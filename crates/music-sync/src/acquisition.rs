//! Crash-safe acquisition staging primitives.

use std::fs::{self, File};
use std::path::{Path, PathBuf};

use thiserror::Error;

use crate::content_hash::{ContentHashError, ContentHasher};
use crate::media_probe::{MediaProbe, MediaProbeError};
use crate::persistence::{AcquisitionWork, ArtifactPersistenceResult, Database, DatabaseError};
use crate::yt_dlp::{YtDlp, YtDlpError};

/// Stable job-specific staging directory outside the managed library.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcquisitionStaging {
    root: PathBuf,
}

impl AcquisitionStaging {
    /// Creates a staging layout rooted below music-sync application state.
    #[must_use]
    pub fn new(state_directory: &Path) -> Self {
        Self {
            root: state_directory.join("acquisition-staging"),
        }
    }

    /// Creates and returns the stable directory for a durable acquisition job.
    ///
    /// Repeated preparation preserves every existing staged file so recovery can
    /// inspect or resume partial work rather than silently discarding evidence.
    pub fn prepare(&self, job_id: i64) -> Result<PathBuf, AcquisitionStagingError> {
        if job_id <= 0 {
            return Err(AcquisitionStagingError::InvalidJobId(job_id));
        }
        let path = self.root.join(format!("job-{job_id}"));
        fs::create_dir_all(&path).map_err(|source| AcquisitionStagingError::Create {
            path: path.clone(),
            source,
        })?;
        Ok(path)
    }
}

/// Structurally valid, exact-byte evidence for one unchanged staged media file.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ValidatedStagedMedia {
    /// Validated staging path.
    pub path: PathBuf,
    /// Lowercase SHA-256 digest of the exact staged bytes.
    pub sha256: String,
    /// Exact stable file length.
    pub bytes: u64,
    /// Codec of the first audio stream reported by the probe.
    pub codec: String,
    /// Container duration when available.
    pub duration_ms: Option<u64>,
    /// Audio sample rate when available.
    pub sample_rate_hz: Option<u32>,
    /// Audio channel count when available.
    pub channels: Option<u32>,
}

/// Validates one staged file without modifying or moving it.
pub fn validate_staged_media(
    path: &Path,
    probe: &dyn MediaProbe,
    hasher: &dyn ContentHasher,
) -> Result<ValidatedStagedMedia, StagedMediaValidationError> {
    let path = path
        .canonicalize()
        .map_err(|source| StagedMediaValidationError::Inspect {
            path: path.to_path_buf(),
            source,
        })?;
    let before = path
        .metadata()
        .map_err(|source| StagedMediaValidationError::Inspect {
            path: path.clone(),
            source,
        })?;
    if !before.is_file() {
        return Err(StagedMediaValidationError::NotFile(path));
    }
    if before.len() == 0 {
        return Err(StagedMediaValidationError::Empty(path));
    }
    let properties = probe
        .probe(&path)
        .map_err(StagedMediaValidationError::Probe)?;
    let hash = hasher
        .hash(&path)
        .map_err(StagedMediaValidationError::Hash)?;
    let after = path
        .metadata()
        .map_err(|source| StagedMediaValidationError::Inspect {
            path: path.clone(),
            source,
        })?;
    if before.len() != hash.bytes_hashed || hash.bytes_hashed != after.len() {
        return Err(StagedMediaValidationError::SizeChanged {
            before: before.len(),
            hashed: hash.bytes_hashed,
            after: after.len(),
        });
    }
    Ok(ValidatedStagedMedia {
        path,
        sha256: hex_sha256(hash.sha256),
        bytes: hash.bytes_hashed,
        codec: properties.codec,
        duration_ms: properties.duration_ms,
        sample_rate_hz: properties.sample_rate_hz,
        channels: properties.channels,
    })
}

/// Derives a conservative managed destination from provider identity and extension.
pub fn artifact_destination(
    library_directory: &Path,
    provider: &str,
    provider_item_id: &str,
    staged_path: &Path,
) -> Result<PathBuf, ArtifactCommitError> {
    let library =
        library_directory
            .canonicalize()
            .map_err(|source| ArtifactCommitError::Library {
                path: library_directory.to_path_buf(),
                source,
            })?;
    if !library.is_dir() {
        return Err(ArtifactCommitError::LibraryNotDirectory(library));
    }
    if !is_safe_component(provider) {
        return Err(ArtifactCommitError::UnsafeProvider(provider.into()));
    }
    if !is_safe_component(provider_item_id) {
        return Err(ArtifactCommitError::UnsafeProviderItemId(
            provider_item_id.into(),
        ));
    }
    let extension = staged_path
        .extension()
        .and_then(|extension| extension.to_str())
        .filter(|extension| {
            !extension.is_empty()
                && extension.len() <= 10
                && extension
                    .chars()
                    .all(|character| character.is_ascii_alphanumeric())
        })
        .ok_or_else(|| ArtifactCommitError::UnsafeExtension(staged_path.to_path_buf()))?;
    Ok(library.join(provider).join(format!(
        "{provider_item_id}.{}",
        extension.to_ascii_lowercase()
    )))
}

fn is_safe_component(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
}

/// Atomically creates a no-clobber final link to validated staged bytes.
///
/// Staging and library storage must share a filesystem. An existing exact-byte path
/// is reported as recovered prepared work; different bytes are always rejected.
fn commit_staged_file(
    validated: &ValidatedStagedMedia,
    final_path: &Path,
    hasher: &dyn ContentHasher,
    allow_prepared_recovery: bool,
) -> Result<ArtifactCommitEffect, ArtifactCommitError> {
    let staged = validated
        .path
        .canonicalize()
        .map_err(|source| ArtifactCommitError::Staged {
            path: validated.path.clone(),
            source,
        })?;
    File::open(&staged)
        .and_then(|file| file.sync_all())
        .map_err(|source| ArtifactCommitError::Sync {
            path: staged.clone(),
            source,
        })?;
    let parent = final_path
        .parent()
        .ok_or_else(|| ArtifactCommitError::MissingParent(final_path.to_path_buf()))?;
    fs::create_dir_all(parent).map_err(|source| ArtifactCommitError::CreateDirectory {
        path: parent.to_path_buf(),
        source,
    })?;
    let canonical_parent =
        parent
            .canonicalize()
            .map_err(|source| ArtifactCommitError::CreateDirectory {
                path: parent.to_path_buf(),
                source,
            })?;
    if canonical_parent != parent {
        return Err(ArtifactCommitError::DestinationEscaped {
            requested: parent.to_path_buf(),
            resolved: canonical_parent,
        });
    }
    match fs::hard_link(&staged, final_path) {
        Ok(()) => {
            File::open(parent)
                .and_then(|directory| directory.sync_all())
                .map_err(|source| ArtifactCommitError::Sync {
                    path: parent.to_path_buf(),
                    source,
                })?;
            Ok(ArtifactCommitEffect::Created)
        }
        Err(source) if source.kind() == std::io::ErrorKind::AlreadyExists => {
            if !allow_prepared_recovery {
                return Err(ArtifactCommitError::ExistingUnowned(
                    final_path.to_path_buf(),
                ));
            }
            let existing = hasher
                .hash(final_path)
                .map_err(ArtifactCommitError::ExistingHash)?;
            if existing.bytes_hashed == validated.bytes
                && hex_sha256(existing.sha256) == validated.sha256
            {
                Ok(ArtifactCommitEffect::RecoveredExisting)
            } else {
                Err(ArtifactCommitError::ExistingMismatch(
                    final_path.to_path_buf(),
                ))
            }
        }
        Err(source) => Err(ArtifactCommitError::Link {
            staged,
            final_path: final_path.to_path_buf(),
            source,
        }),
    }
}

/// Persists intent, performs the no-clobber filesystem commit, then finalizes SQLite.
pub fn commit_validated_acquisition(
    database: &mut Database,
    work: &AcquisitionWork,
    validated: &ValidatedStagedMedia,
    library_directory: &Path,
    hasher: &dyn ContentHasher,
) -> Result<AcquisitionCommitResult, AcquisitionCommitError> {
    let final_path = artifact_destination(
        library_directory,
        &work.provider,
        &work.provider_item_id,
        &validated.path,
    )?;
    let newly_prepared =
        database.prepare_acquisition_commit(work.job_id, validated, &final_path)?;
    let filesystem = commit_staged_file(validated, &final_path, hasher, !newly_prepared)?;
    let database = database.finalize_acquisition_commit(work.job_id)?;
    Ok(AcquisitionCommitResult {
        final_path,
        filesystem,
        database,
    })
}

/// Claims and processes at most one acquisition through every safe boundary.
pub fn run_one_acquisition(
    database: &mut Database,
    state_directory: &Path,
    library_directory: &Path,
    downloader: &YtDlp,
    probe: &dyn MediaProbe,
    hasher: &dyn ContentHasher,
) -> Result<AcquisitionRunOutcome, AcquisitionRunError> {
    let Some(work) = database.claim_next_acquisition()? else {
        return Ok(AcquisitionRunOutcome::Idle);
    };
    run_claimed_acquisition(
        database,
        work,
        state_directory,
        library_directory,
        downloader,
        probe,
        hasher,
    )
}

fn run_claimed_acquisition(
    database: &mut Database,
    work: AcquisitionWork,
    state_directory: &Path,
    library_directory: &Path,
    downloader: &YtDlp,
    probe: &dyn MediaProbe,
    hasher: &dyn ContentHasher,
) -> Result<AcquisitionRunOutcome, AcquisitionRunError> {
    let result = (|| {
        let staging = AcquisitionStaging::new(state_directory).prepare(work.job_id)?;
        let downloaded = downloader.download(&work.original_url, &staging)?;
        let validated = validate_staged_media(&downloaded.path, probe, hasher)?;
        if downloaded.bytes != validated.bytes {
            return Err(AcquisitionRunError::DownloadSizeChanged {
                downloaded: downloaded.bytes,
                validated: validated.bytes,
            });
        }
        let commit =
            commit_validated_acquisition(database, &work, &validated, library_directory, hasher)?;
        Ok(AcquisitionRunOutcome::Committed {
            work: work.clone(),
            commit,
        })
    })();
    match result {
        Ok(outcome) => Ok(outcome),
        Err(error) => {
            let message = error.to_string();
            database
                .defer_acquisition(work.job_id, &message)
                .map_err(|database| AcquisitionRunError::DeferFailed { message, database })?;
            Err(error)
        }
    }
}

/// Processes a bounded snapshot of runnable jobs, attempting each at most once.
pub fn run_pending_acquisitions(
    database: &mut Database,
    state_directory: &Path,
    library_directory: &Path,
    downloader: &YtDlp,
    probe: &dyn MediaProbe,
    hasher: &dyn ContentHasher,
    maximum_jobs: usize,
) -> Result<AcquisitionBatchReport, AcquisitionRunError> {
    let job_ids = database.runnable_acquisition_job_ids(maximum_jobs)?;
    let mut report = AcquisitionBatchReport {
        selected: job_ids.len() as u64,
        ..AcquisitionBatchReport::default()
    };
    for job_id in job_ids {
        let Some(work) = database.claim_acquisition(job_id)? else {
            report.skipped += 1;
            continue;
        };
        match run_claimed_acquisition(
            database,
            work,
            state_directory,
            library_directory,
            downloader,
            probe,
            hasher,
        ) {
            Ok(AcquisitionRunOutcome::Committed { .. }) => report.committed += 1,
            Ok(AcquisitionRunOutcome::Idle) => report.skipped += 1,
            Err(error) if error.is_database_failure() => return Err(error),
            Err(error) => report.failures.push(AcquisitionBatchFailure {
                job_id,
                message: error.to_string(),
            }),
        }
    }
    Ok(report)
}

/// Result of processing at most one durable acquisition job.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum AcquisitionRunOutcome {
    /// No pending or deferred acquisition was available.
    Idle,
    /// One artifact was validated, committed, and persisted successfully.
    Committed {
        /// Claimed provider work.
        work: AcquisitionWork,
        /// Filesystem and SQLite commit effects.
        commit: AcquisitionCommitResult,
    },
}

/// Aggregate effects of one bounded acquisition batch.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct AcquisitionBatchReport {
    /// Runnable job IDs snapshotted at batch start.
    pub selected: u64,
    /// Jobs committed successfully.
    pub committed: u64,
    /// Jobs no longer runnable when their turn arrived.
    pub skipped: u64,
    /// Ordinary per-job failures deferred without stopping the batch.
    pub failures: Vec<AcquisitionBatchFailure>,
}

/// One isolated job failure within a completed batch.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct AcquisitionBatchFailure {
    /// Durable failed job ID.
    pub job_id: i64,
    /// Persisted user-facing failure diagnostic.
    pub message: String,
}

/// Completed filesystem and database effects for one validated acquisition.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct AcquisitionCommitResult {
    /// Final managed artifact path.
    pub final_path: PathBuf,
    /// Whether the no-clobber link was newly created or recovered.
    pub filesystem: ArtifactCommitEffect,
    /// Recording/artifact persistence result.
    pub database: ArtifactPersistenceResult,
}

/// Observable filesystem effect of a no-clobber artifact commit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactCommitEffect {
    /// A new final hard link was atomically created.
    Created,
    /// A prior prepared attempt already created the same exact-byte final file.
    RecoveredExisting,
}

fn hex_sha256(bytes: [u8; 32]) -> String {
    use std::fmt::Write;

    bytes
        .iter()
        .fold(String::with_capacity(64), |mut output, byte| {
            let _ = write!(output, "{byte:02x}");
            output
        })
}

/// Failure to prepare job-specific acquisition staging.
#[derive(Debug, Error)]
pub enum AcquisitionStagingError {
    /// Durable job identifiers must be positive.
    #[error("acquisition job ID must be greater than zero: {0}")]
    InvalidJobId(i64),
    /// The staging directory could not be created.
    #[error("failed to create acquisition staging directory {path}: {source}")]
    Create {
        /// Requested staging directory.
        path: PathBuf,
        /// Underlying filesystem error.
        source: std::io::Error,
    },
}

/// Failure to validate an untrusted staged media file.
#[derive(Debug, Error)]
pub enum StagedMediaValidationError {
    /// Staged filesystem metadata could not be read.
    #[error("failed to inspect staged media {path}: {source}")]
    Inspect {
        /// Staged path.
        path: PathBuf,
        /// Underlying filesystem error.
        source: std::io::Error,
    },
    /// The staged path was not a regular file.
    #[error("staged media is not a regular file: {0}")]
    NotFile(PathBuf),
    /// The staged path was empty.
    #[error("staged media is empty: {0}")]
    Empty(PathBuf),
    /// Structural probing failed.
    #[error("staged media probe failed: {0}")]
    Probe(MediaProbeError),
    /// Exact-byte hashing failed.
    #[error("staged media hash failed: {0}")]
    Hash(ContentHashError),
    /// File size changed or did not match bytes consumed while validating.
    #[error(
        "staged media size changed during validation: before={before}, hashed={hashed}, after={after}"
    )]
    SizeChanged {
        /// Size before probing and hashing.
        before: u64,
        /// Bytes consumed by the hasher.
        hashed: u64,
        /// Size after hashing.
        after: u64,
    },
}

/// Failure to derive or atomically create a managed artifact destination.
#[derive(Debug, Error)]
pub enum ArtifactCommitError {
    /// The configured library root could not be resolved.
    #[error("failed to access managed library {path}: {source}")]
    Library {
        /// Configured library path.
        path: PathBuf,
        /// Underlying filesystem error.
        source: std::io::Error,
    },
    /// The configured library root was not a directory.
    #[error("managed library path is not a directory: {0}")]
    LibraryNotDirectory(PathBuf),
    /// Provider name was unsafe as a path component.
    #[error("provider cannot be represented as a safe path component: {0}")]
    UnsafeProvider(String),
    /// Provider item ID was unsafe as a path component.
    #[error("provider item ID cannot be represented as a safe path component: {0}")]
    UnsafeProviderItemId(String),
    /// Staged extension was absent or unsafe.
    #[error("staged media extension is missing or unsafe: {0}")]
    UnsafeExtension(PathBuf),
    /// Staged media could not be resolved.
    #[error("failed to access staged media {path}: {source}")]
    Staged {
        /// Staged path.
        path: PathBuf,
        /// Underlying filesystem error.
        source: std::io::Error,
    },
    /// Final path had no parent directory.
    #[error("final artifact path has no parent: {0}")]
    MissingParent(PathBuf),
    /// A managed destination directory could not be created.
    #[error("failed to create managed artifact directory {path}: {source}")]
    CreateDirectory {
        /// Requested directory.
        path: PathBuf,
        /// Underlying filesystem error.
        source: std::io::Error,
    },
    /// A destination parent resolved through a symlink or other path indirection.
    #[error("managed artifact directory escaped its requested path {requested}: {resolved}")]
    DestinationEscaped {
        /// Lexical destination parent below the canonical library root.
        requested: PathBuf,
        /// Canonical destination parent.
        resolved: PathBuf,
    },
    /// Staged bytes or the destination directory could not be synchronized.
    #[error("failed to synchronize artifact path {path}: {source}")]
    Sync {
        /// File or directory path.
        path: PathBuf,
        /// Underlying filesystem error.
        source: std::io::Error,
    },
    /// The same-filesystem no-clobber link failed.
    #[error("failed to atomically link staged media {staged} to {final_path}: {source}")]
    Link {
        /// Canonical staged file.
        staged: PathBuf,
        /// Intended final path.
        final_path: PathBuf,
        /// Underlying filesystem error.
        source: std::io::Error,
    },
    /// Existing destination bytes could not be hashed for recovery comparison.
    #[error("failed to hash existing destination: {0}")]
    ExistingHash(ContentHashError),
    /// A destination existed before this job established recoverable commit intent.
    #[error("existing artifact destination is not owned by this prepared commit: {0}")]
    ExistingUnowned(PathBuf),
    /// Existing destination bytes differed and were preserved untouched.
    #[error("existing artifact destination has different bytes and was preserved: {0}")]
    ExistingMismatch(PathBuf),
}

/// Failure across prepared intent, no-clobber filesystem commit, or final SQLite state.
#[derive(Debug, Error)]
pub enum AcquisitionCommitError {
    /// Destination derivation or filesystem commit failed.
    #[error(transparent)]
    Filesystem(#[from] ArtifactCommitError),
    /// Prepared or finalized SQLite state failed transactionally.
    #[error(transparent)]
    Database(#[from] DatabaseError),
}

/// Failure while running one claimed acquisition through safe staging and commit.
#[derive(Debug, Error)]
pub enum AcquisitionRunError {
    /// SQLite claim or state transition failed.
    #[error(transparent)]
    Database(#[from] DatabaseError),
    /// Job staging could not be prepared.
    #[error(transparent)]
    Staging(#[from] AcquisitionStagingError),
    /// yt-dlp failed or returned unsafe staged output.
    #[error(transparent)]
    Download(#[from] YtDlpError),
    /// Structural or exact-byte validation failed.
    #[error(transparent)]
    Validation(#[from] StagedMediaValidationError),
    /// Download and validation observed different byte counts.
    #[error(
        "staged download size changed before commit: downloaded={downloaded}, validated={validated}"
    )]
    DownloadSizeChanged {
        /// Bytes observed after yt-dlp exited.
        downloaded: u64,
        /// Bytes consumed by validation.
        validated: u64,
    },
    /// Prepared no-clobber artifact commit failed.
    #[error(transparent)]
    Commit(#[from] AcquisitionCommitError),
    /// Recording the original failure as deferred also failed.
    #[error("acquisition failed ({message}) and deferring the job failed: {database}")]
    DeferFailed {
        /// Original bounded failure diagnostic.
        message: String,
        /// SQLite deferral failure.
        database: DatabaseError,
    },
}

impl AcquisitionRunError {
    fn is_database_failure(&self) -> bool {
        matches!(
            self,
            Self::Database(_)
                | Self::DeferFailed { .. }
                | Self::Commit(AcquisitionCommitError::Database(_))
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content_hash::{ContentHash, Sha256FileHasher};
    use crate::media_probe::{MediaProperties, MediaTagPresence};
    use crate::provider::{ProviderItem, SourceSnapshot};
    use serde_json::json;

    struct FixtureProbe {
        fails: bool,
    }

    impl MediaProbe for FixtureProbe {
        fn probe(&self, _path: &Path) -> Result<MediaProperties, MediaProbeError> {
            if self.fails {
                return Err(MediaProbeError::NoAudioStream);
            }
            Ok(MediaProperties {
                codec: "opus".into(),
                duration_ms: Some(1_234),
                sample_rate_hz: Some(48_000),
                channels: Some(2),
                has_embedded_artwork: false,
                tags: MediaTagPresence::default(),
            })
        }
    }

    struct WrongLengthHasher;

    impl ContentHasher for WrongLengthHasher {
        fn hash(&self, _path: &Path) -> Result<ContentHash, ContentHashError> {
            Ok(ContentHash {
                sha256: [0; 32],
                bytes_hashed: 1,
            })
        }
    }

    fn claimed_work() -> Result<(Database, AcquisitionWork), Box<dyn std::error::Error>> {
        let mut database = Database::open_in_memory()?;
        let source = database.add_source("https://youtu.be/fixture", None)?;
        database.reconcile_source_snapshot(
            source.id,
            &SourceSnapshot {
                provider: "youtube".into(),
                provider_collection_id: None,
                title: Some("Fixture".into()),
                items: vec![ProviderItem {
                    provider_item_id: "fixture_id".into(),
                    url: "https://youtu.be/fixture".into(),
                    title: Some("Fixture".into()),
                    duration_ms: Some(1_234),
                    raw_metadata: json!({ "id": "fixture_id" }),
                }],
            },
        )?;
        let work = database
            .claim_next_acquisition()?
            .ok_or("missing acquisition work")?;
        Ok((database, work))
    }

    fn validated_fixture(
        directory: &Path,
    ) -> Result<ValidatedStagedMedia, Box<dyn std::error::Error>> {
        let path = directory.join("media.opus");
        fs::write(&path, b"validated audio")?;
        Ok(validate_staged_media(
            &path,
            &FixtureProbe { fails: false },
            &Sha256FileHasher,
        )?)
    }

    #[test]
    fn repeated_prepare_preserves_staged_evidence() -> Result<(), Box<dyn std::error::Error>> {
        let state = tempfile::tempdir()?;
        let staging = AcquisitionStaging::new(state.path());
        let first = staging.prepare(42)?;
        fs::write(first.join("partial.media"), b"preserve")?;

        let repeated = staging.prepare(42)?;

        assert_eq!(repeated, first);
        assert_eq!(fs::read(repeated.join("partial.media"))?, b"preserve");
        Ok(())
    }

    #[test]
    fn rejects_non_positive_job_ids() {
        let staging = AcquisitionStaging::new(Path::new("state"));
        assert!(matches!(
            staging.prepare(0),
            Err(AcquisitionStagingError::InvalidJobId(0))
        ));
    }

    #[test]
    fn validation_combines_audio_properties_and_exact_hash()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("media.opus");
        fs::write(&path, b"abc")?;

        let validated =
            validate_staged_media(&path, &FixtureProbe { fails: false }, &Sha256FileHasher)?;

        assert_eq!(validated.path, path);
        assert_eq!(validated.bytes, 3);
        assert_eq!(validated.codec, "opus");
        assert_eq!(validated.duration_ms, Some(1_234));
        assert_eq!(
            validated.sha256,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        Ok(())
    }

    #[test]
    fn validation_rejects_probe_failure_and_hash_size_mismatch()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("media.opus");
        fs::write(&path, b"abc")?;

        let probe_failure =
            validate_staged_media(&path, &FixtureProbe { fails: true }, &Sha256FileHasher);
        assert!(matches!(
            probe_failure,
            Err(StagedMediaValidationError::Probe(
                MediaProbeError::NoAudioStream
            ))
        ));

        let mismatch =
            validate_staged_media(&path, &FixtureProbe { fails: false }, &WrongLengthHasher);
        assert!(matches!(
            mismatch,
            Err(StagedMediaValidationError::SizeChanged {
                before: 3,
                hashed: 1,
                after: 3
            })
        ));
        assert_eq!(fs::read(path)?, b"abc");
        Ok(())
    }

    #[test]
    fn validated_acquisition_commits_without_clobber_and_repeats_idempotently()
    -> Result<(), Box<dyn std::error::Error>> {
        let state = tempfile::tempdir()?;
        let library = tempfile::tempdir_in(state.path().parent().ok_or("missing parent")?)?;
        let validated = validated_fixture(state.path())?;
        let (mut database, work) = claimed_work()?;

        let first = commit_validated_acquisition(
            &mut database,
            &work,
            &validated,
            library.path(),
            &Sha256FileHasher,
        )?;
        let repeated = commit_validated_acquisition(
            &mut database,
            &work,
            &validated,
            library.path(),
            &Sha256FileHasher,
        )?;

        assert_eq!(first.filesystem, ArtifactCommitEffect::Created);
        assert!(first.database.inserted);
        assert_eq!(fs::read(&first.final_path)?, b"validated audio");
        assert_eq!(repeated.filesystem, ArtifactCommitEffect::RecoveredExisting);
        assert!(!repeated.database.inserted);
        assert_eq!(repeated.database.artifact_id, first.database.artifact_id);
        assert_eq!(fs::read(&validated.path)?, b"validated audio");
        Ok(())
    }

    #[test]
    fn prepared_filesystem_commit_recovers_after_interruption()
    -> Result<(), Box<dyn std::error::Error>> {
        let state = tempfile::tempdir()?;
        let library = tempfile::tempdir_in(state.path().parent().ok_or("missing parent")?)?;
        let validated = validated_fixture(state.path())?;
        let (mut database, work) = claimed_work()?;
        let final_path = artifact_destination(
            library.path(),
            &work.provider,
            &work.provider_item_id,
            &validated.path,
        )?;
        assert!(database.prepare_acquisition_commit(work.job_id, &validated, &final_path)?);
        assert_eq!(
            commit_staged_file(&validated, &final_path, &Sha256FileHasher, false)?,
            ArtifactCommitEffect::Created
        );

        assert_eq!(database.recover_interrupted_acquisitions()?, 1);
        assert!(database.retry_deferred_acquisition(work.job_id)?);
        let retry = database
            .claim_next_acquisition()?
            .ok_or("missing recovered work")?;
        let recovered = commit_validated_acquisition(
            &mut database,
            &retry,
            &validated,
            library.path(),
            &Sha256FileHasher,
        )?;

        assert_eq!(
            recovered.filesystem,
            ArtifactCommitEffect::RecoveredExisting
        );
        assert!(recovered.database.inserted);
        assert_eq!(fs::read(recovered.final_path)?, b"validated audio");
        Ok(())
    }

    #[test]
    fn different_existing_destination_is_preserved_and_not_persisted()
    -> Result<(), Box<dyn std::error::Error>> {
        let state = tempfile::tempdir()?;
        let library = tempfile::tempdir_in(state.path().parent().ok_or("missing parent")?)?;
        let validated = validated_fixture(state.path())?;
        let (mut database, work) = claimed_work()?;
        let destination = artifact_destination(
            library.path(),
            &work.provider,
            &work.provider_item_id,
            &validated.path,
        )?;
        fs::create_dir_all(destination.parent().ok_or("missing destination parent")?)?;
        assert!(database.prepare_acquisition_commit(work.job_id, &validated, &destination)?);
        fs::write(&destination, b"existing user bytes")?;

        let result = commit_validated_acquisition(
            &mut database,
            &work,
            &validated,
            library.path(),
            &Sha256FileHasher,
        );

        assert!(matches!(
            result,
            Err(AcquisitionCommitError::Filesystem(
                ArtifactCommitError::ExistingMismatch(_)
            ))
        ));
        assert_eq!(fs::read(destination)?, b"existing user bytes");
        assert_eq!(fs::read(validated.path)?, b"validated audio");
        Ok(())
    }

    #[test]
    fn identical_preexisting_destination_is_not_claimed_as_recovery()
    -> Result<(), Box<dyn std::error::Error>> {
        let state = tempfile::tempdir()?;
        let library = tempfile::tempdir_in(state.path().parent().ok_or("missing parent")?)?;
        let validated = validated_fixture(state.path())?;
        let (mut database, work) = claimed_work()?;
        let destination = artifact_destination(
            library.path(),
            &work.provider,
            &work.provider_item_id,
            &validated.path,
        )?;
        fs::create_dir_all(destination.parent().ok_or("missing destination parent")?)?;
        fs::write(&destination, b"validated audio")?;

        let result = commit_validated_acquisition(
            &mut database,
            &work,
            &validated,
            library.path(),
            &Sha256FileHasher,
        );

        assert!(matches!(
            result,
            Err(AcquisitionCommitError::Filesystem(
                ArtifactCommitError::ExistingUnowned(_)
            ))
        ));
        assert_eq!(fs::read(destination)?, b"validated audio");
        Ok(())
    }

    #[test]
    fn destination_rejects_unsafe_identity_and_symlink_escape()
    -> Result<(), Box<dyn std::error::Error>> {
        let state = tempfile::tempdir()?;
        let library = tempfile::tempdir_in(state.path().parent().ok_or("missing parent")?)?;
        let outside = tempfile::tempdir_in(state.path().parent().ok_or("missing parent")?)?;
        let validated = validated_fixture(state.path())?;
        assert!(matches!(
            artifact_destination(library.path(), "youtube", "../escape", &validated.path),
            Err(ArtifactCommitError::UnsafeProviderItemId(_))
        ));

        std::os::unix::fs::symlink(outside.path(), library.path().join("youtube"))?;
        let destination =
            artifact_destination(library.path(), "youtube", "fixture_id", &validated.path)?;
        let escaped = commit_staged_file(&validated, &destination, &Sha256FileHasher, false);
        assert!(matches!(
            escaped,
            Err(ArtifactCommitError::DestinationEscaped { .. })
        ));
        assert!(!outside.path().join("fixture_id.opus").exists());
        Ok(())
    }
}
