//! Bounded Chromaprint fingerprint extraction through `fpcalc`.

use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::persistence::{ArtifactFingerprintEvidence, Database, DatabaseError};

const MAX_OUTPUT_BYTES: u64 = 4 * 1024 * 1024;
const MAX_FINGERPRINT_VALUES: usize = 100_000;
const MAX_EXECUTABLE_BUSY_RETRIES: usize = 10;

/// Read-only boundary for deriving perceptual audio evidence.
pub trait Fingerprinter {
    /// Derives a bounded raw Chromaprint fingerprint from one local media file.
    fn fingerprint(&self, path: &Path) -> Result<RawFingerprint, FingerprintError>;
}

/// Raw algorithm-2 Chromaprint evidence suitable for deterministic comparison.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawFingerprint {
    /// Rounded media duration reported by fpcalc.
    pub duration_ms: u64,
    /// Uncompressed unsigned fingerprint values.
    pub values: Vec<u32>,
}

/// Bounded subprocess adapter around `fpcalc`.
#[derive(Debug, Clone)]
pub struct Fpcalc {
    executable: PathBuf,
    timeout: Duration,
    maximum_audio_seconds: u32,
}

impl Fpcalc {
    /// Creates an adapter with explicit executable, deadline, and audio bound.
    #[must_use]
    pub fn new(executable: PathBuf, timeout: Duration, maximum_audio_seconds: u32) -> Self {
        Self {
            executable,
            timeout,
            maximum_audio_seconds,
        }
    }
}

impl Fingerprinter for Fpcalc {
    fn fingerprint(&self, path: &Path) -> Result<RawFingerprint, FingerprintError> {
        if self.maximum_audio_seconds == 0 {
            return Err(FingerprintError::InvalidAudioLimit);
        }
        let mut command = Command::new(&self.executable);
        command
            .args([
                "-raw",
                "-json",
                "-algorithm",
                "2",
                "-length",
                &self.maximum_audio_seconds.to_string(),
                "--",
            ])
            .arg(path)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child =
            spawn_with_busy_retry(&mut command).map_err(|source| FingerprintError::Spawn {
                executable: self.executable.clone(),
                source,
            })?;
        let stdout = child.stdout.take().ok_or(FingerprintError::MissingPipe)?;
        let stderr = child.stderr.take().ok_or(FingerprintError::MissingPipe)?;
        let stdout_reader = thread::spawn(move || read_bounded(stdout));
        let stderr_reader = thread::spawn(move || read_bounded(stderr));
        let deadline = Instant::now()
            .checked_add(self.timeout)
            .ok_or(FingerprintError::InvalidTimeout)?;
        let status = loop {
            if let Some(status) = child.try_wait().map_err(FingerprintError::Wait)? {
                break status;
            }
            let now = Instant::now();
            if now >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                join_reader(stdout_reader)?;
                join_reader(stderr_reader)?;
                return Err(FingerprintError::Timeout(self.timeout));
            }
            thread::sleep(
                deadline
                    .saturating_duration_since(now)
                    .min(Duration::from_millis(10)),
            );
        };
        let stdout = join_reader(stdout_reader)?;
        let stderr = join_reader(stderr_reader)?;
        if !status.success() {
            return Err(FingerprintError::Failed {
                code: status.code(),
                stderr: String::from_utf8_lossy(&stderr).trim().to_owned(),
            });
        }
        parse_output(&stdout)
    }
}

fn spawn_with_busy_retry(command: &mut Command) -> io::Result<std::process::Child> {
    for retry in 0..=MAX_EXECUTABLE_BUSY_RETRIES {
        match command.spawn() {
            Err(error)
                if error.kind() == io::ErrorKind::ExecutableFileBusy
                    && retry < MAX_EXECUTABLE_BUSY_RETRIES =>
            {
                thread::sleep(Duration::from_millis(5));
            }
            result => return result,
        }
    }
    unreachable!("bounded spawn loop always returns on its final attempt")
}

/// Reconciles raw fingerprint evidence for a bounded stable healthy-artifact set.
pub fn reconcile_artifact_fingerprints(
    database: &mut Database,
    library_directory: &Path,
    fingerprinter: &dyn Fingerprinter,
    maximum_artifacts: usize,
    maximum_audio_seconds: u32,
) -> Result<FingerprintReport, FingerprintReconciliationError> {
    if maximum_artifacts == 0 || maximum_audio_seconds == 0 {
        return Err(FingerprintReconciliationError::InvalidLimit);
    }
    let library = library_directory.canonicalize().map_err(|source| {
        FingerprintReconciliationError::Library {
            path: library_directory.to_path_buf(),
            source,
        }
    })?;
    let candidates =
        database.artifact_fingerprint_candidates(maximum_artifacts, maximum_audio_seconds)?;
    let mut report = FingerprintReport {
        selected: candidates.len() as u64,
        ..FingerprintReport::default()
    };
    for candidate in candidates {
        if candidate.prior_max_seconds == Some(i64::from(maximum_audio_seconds))
            && candidate.prior_fingerprint_json.is_some()
        {
            report.unchanged += 1;
            continue;
        }
        let path = match safe_regular_path(&candidate.path, &library) {
            Ok(path) => path,
            Err(message) => {
                database.defer_artifact_fingerprint(
                    candidate.artifact_id,
                    maximum_audio_seconds,
                    &message,
                )?;
                report.failures.push(FingerprintFailure {
                    artifact_id: candidate.artifact_id,
                    path: candidate.path,
                    message,
                });
                continue;
            }
        };
        let fingerprint = match fingerprinter.fingerprint(&path) {
            Ok(fingerprint) => fingerprint,
            Err(error) => {
                let message = error.to_string();
                database.defer_artifact_fingerprint(
                    candidate.artifact_id,
                    maximum_audio_seconds,
                    &message,
                )?;
                report.failures.push(FingerprintFailure {
                    artifact_id: candidate.artifact_id,
                    path: candidate.path,
                    message,
                });
                continue;
            }
        };
        let evidence = ArtifactFingerprintEvidence {
            artifact_id: candidate.artifact_id,
            max_seconds: maximum_audio_seconds,
            duration_ms: i64::try_from(fingerprint.duration_ms)
                .map_err(|_| FingerprintReconciliationError::ValueRange)?,
            fingerprint_json: serde_json::to_string(&fingerprint.values)
                .map_err(FingerprintReconciliationError::Json)?,
            value_count: i64::try_from(fingerprint.values.len())
                .map_err(|_| FingerprintReconciliationError::ValueRange)?,
        };
        if database.record_artifact_fingerprint(&evidence)? {
            report.recorded += 1;
        } else {
            report.unchanged += 1;
        }
        database.clear_artifact_fingerprint_deferral(candidate.artifact_id)?;
    }
    Ok(report)
}

fn safe_regular_path(path: &Path, library: &Path) -> Result<PathBuf, String> {
    if !path.is_absolute() || !path.starts_with(library) {
        return Err("registered artifact is outside the configured library".into());
    }
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|error| format!("failed to inspect registered artifact: {error}"))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err("registered artifact is not a regular non-symlink file".into());
    }
    let canonical = path
        .canonicalize()
        .map_err(|error| format!("failed to resolve registered artifact: {error}"))?;
    if !canonical.starts_with(library) {
        return Err("registered artifact resolves outside the configured library".into());
    }
    Ok(canonical)
}

/// Aggregate bounded fingerprint reconciliation effects.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct FingerprintReport {
    /// Healthy artifacts selected in stable ID order.
    pub selected: u64,
    /// New or refreshed fingerprint rows.
    pub recorded: u64,
    /// Evidence already present at the requested extraction bound.
    pub unchanged: u64,
    /// Isolated artifacts with insufficient fingerprint evidence.
    pub failures: Vec<FingerprintFailure>,
}

/// One isolated fingerprint failure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FingerprintFailure {
    /// Durable artifact ID.
    pub artifact_id: i64,
    /// Registered artifact path.
    pub path: PathBuf,
    /// Bounded failure diagnostic.
    pub message: String,
}

/// Fatal bounded fingerprint reconciliation failure.
#[derive(Debug, Error)]
pub enum FingerprintReconciliationError {
    /// Work and audio bounds must be non-zero.
    #[error("fingerprint limits must be greater than zero")]
    InvalidLimit,
    /// Configured library could not be resolved.
    #[error("failed to access library {path}: {source}")]
    Library {
        /// Configured library path.
        path: PathBuf,
        /// Underlying filesystem error.
        source: io::Error,
    },
    /// Durable state access failed.
    #[error(transparent)]
    Database(#[from] DatabaseError),
    /// Raw evidence serialization failed.
    #[error("failed to serialize raw fingerprint: {0}")]
    Json(serde_json::Error),
    /// Evidence could not fit the durable SQLite representation.
    #[error("fingerprint evidence exceeds SQLite range")]
    ValueRange,
}

#[derive(Deserialize)]
struct FpcalcOutput {
    duration: f64,
    fingerprint: Vec<u32>,
}

fn parse_output(bytes: &[u8]) -> Result<RawFingerprint, FingerprintError> {
    let output: FpcalcOutput = serde_json::from_slice(bytes).map_err(FingerprintError::Json)?;
    if !output.duration.is_finite()
        || output.duration < 0.0
        || output.duration > u64::MAX as f64 / 1000.0
    {
        return Err(FingerprintError::InvalidDuration);
    }
    if output.fingerprint.is_empty() {
        return Err(FingerprintError::Empty);
    }
    if output.fingerprint.len() > MAX_FINGERPRINT_VALUES {
        return Err(FingerprintError::TooManyValues(output.fingerprint.len()));
    }
    Ok(RawFingerprint {
        duration_ms: (output.duration * 1000.0).round() as u64,
        values: output.fingerprint,
    })
}

fn read_bounded(reader: impl Read) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader.take(MAX_OUTPUT_BYTES + 1).read_to_end(&mut bytes)?;
    Ok(bytes)
}

fn join_reader(
    handle: thread::JoinHandle<io::Result<Vec<u8>>>,
) -> Result<Vec<u8>, FingerprintError> {
    let bytes = handle
        .join()
        .map_err(|_| FingerprintError::OutputThread)??;
    if bytes.len() as u64 > MAX_OUTPUT_BYTES {
        return Err(FingerprintError::OutputTooLarge(MAX_OUTPUT_BYTES));
    }
    Ok(bytes)
}

/// Failure to derive bounded perceptual evidence.
#[derive(Debug, Error)]
pub enum FingerprintError {
    /// Fingerprint audio limit must be non-zero.
    #[error("fpcalc audio limit must be greater than zero")]
    InvalidAudioLimit,
    /// Subprocess could not start.
    #[error("failed to start {executable}: {source}")]
    Spawn {
        /// Configured executable path.
        executable: PathBuf,
        /// Underlying process error.
        source: io::Error,
    },
    /// Waiting for fpcalc failed.
    #[error("failed while waiting for fpcalc: {0}")]
    Wait(io::Error),
    /// Captured output could not be read.
    #[error("failed to read fpcalc output: {0}")]
    Read(#[from] io::Error),
    /// Expected subprocess pipe was absent.
    #[error("fpcalc output pipe was unavailable")]
    MissingPipe,
    /// Output-reader thread stopped unexpectedly.
    #[error("fpcalc output reader stopped unexpectedly")]
    OutputThread,
    /// Captured output exceeded its bound.
    #[error("fpcalc output exceeded {0} bytes")]
    OutputTooLarge(u64),
    /// Deadline could not be represented.
    #[error("fpcalc timeout is too large")]
    InvalidTimeout,
    /// Subprocess exceeded its deadline.
    #[error("fpcalc exceeded its timeout of {0:?}")]
    Timeout(Duration),
    /// Subprocess returned failure.
    #[error("fpcalc failed with status {code:?}: {stderr}")]
    Failed {
        /// Exit code when available.
        code: Option<i32>,
        /// Bounded stderr diagnostic.
        stderr: String,
    },
    /// JSON output was malformed.
    #[error("fpcalc returned invalid JSON: {0}")]
    Json(serde_json::Error),
    /// Duration was invalid.
    #[error("fpcalc returned an invalid duration")]
    InvalidDuration,
    /// Fingerprint contained no evidence.
    #[error("fpcalc returned an empty fingerprint")]
    Empty,
    /// Fingerprint value count exceeded its semantic bound.
    #[error("fpcalc returned {0} fingerprint values")]
    TooManyValues(usize),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_bounded_raw_json() -> Result<(), FingerprintError> {
        let result = parse_output(br#"{"duration":3.25,"fingerprint":[1,2,4294967295]}"#)?;
        assert_eq!(result.duration_ms, 3_250);
        assert_eq!(result.values, [1, 2, u32::MAX]);
        Ok(())
    }

    #[test]
    fn rejects_empty_and_invalid_duration() {
        assert!(matches!(
            parse_output(br#"{"duration":1.0,"fingerprint":[]}"#),
            Err(FingerprintError::Empty)
        ));
        assert!(matches!(
            parse_output(br#"{"duration":-1.0,"fingerprint":[1]}"#),
            Err(FingerprintError::InvalidDuration)
        ));
    }
}
