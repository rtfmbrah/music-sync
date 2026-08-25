//! Bounded `yt-dlp` adapter for YouTube source enumeration.

use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use serde_json::Value;
use thiserror::Error;

use crate::provider::{ProviderFailureKind, ProviderItem, SourceSnapshot};

const MAX_OUTPUT_BYTES: usize = 16 * 1024 * 1024;

/// YouTube enumeration adapter backed by `yt-dlp`.
#[derive(Debug, Clone)]
pub struct YtDlp {
    executable: PathBuf,
    timeout: Duration,
    cookie_file: Option<PathBuf>,
    sleep_requests_seconds: Option<u64>,
    download_sleep_seconds: Option<(u64, u64)>,
}

impl YtDlp {
    /// Creates an adapter with an explicit executable and enumeration deadline.
    #[must_use]
    pub fn new(executable: PathBuf, timeout: Duration) -> Self {
        Self {
            executable,
            timeout,
            cookie_file: None,
            sleep_requests_seconds: None,
            download_sleep_seconds: None,
        }
    }

    /// Configures an optional Netscape-format cookie file for authenticated provider access.
    #[must_use]
    pub fn with_cookie_file(mut self, cookie_file: Option<PathBuf>) -> Self {
        self.cookie_file = cookie_file;
        self
    }

    /// Configures yt-dlp's own extraction and randomized pre-download pacing.
    #[must_use]
    pub fn with_pacing(
        mut self,
        sleep_requests_seconds: u64,
        minimum_download_sleep_seconds: u64,
        maximum_download_sleep_seconds: u64,
    ) -> Self {
        self.sleep_requests_seconds = Some(sleep_requests_seconds);
        self.download_sleep_seconds = Some((
            minimum_download_sleep_seconds,
            maximum_download_sleep_seconds,
        ));
        self
    }

    /// Enumerates a single video or playlist without downloading media.
    pub fn enumerate(&self, url: &str) -> Result<SourceSnapshot, YtDlpError> {
        let mut command = Command::new(&self.executable);
        self.apply_authentication(&mut command);
        self.apply_request_pacing(&mut command);
        command.args([
            "--flat-playlist",
            "--dump-single-json",
            "--no-warnings",
            "--",
            url,
        ]);
        let stdout = self.execute(&mut command)?;
        parse_snapshot(&stdout)
    }

    /// Downloads one provider item into an existing job-specific staging directory.
    ///
    /// The returned file remains staging evidence. This method never writes to or
    /// commits within the managed library.
    pub fn download(&self, url: &str, staging: &Path) -> Result<DownloadedMedia, YtDlpError> {
        let staging = staging
            .canonicalize()
            .map_err(|source| YtDlpError::Staging {
                path: staging.to_path_buf(),
                source,
            })?;
        if !staging.is_dir() {
            return Err(YtDlpError::StagingNotDirectory(staging));
        }
        let mut command = Command::new(&self.executable);
        self.apply_authentication(&mut command);
        self.apply_request_pacing(&mut command);
        if let Some((minimum, maximum)) = self.download_sleep_seconds {
            command
                .arg("--sleep-interval")
                .arg(minimum.to_string())
                .arg("--max-sleep-interval")
                .arg(maximum.to_string());
        }
        command
            .args([
                "--no-playlist",
                "--no-progress",
                "--no-warnings",
                "--quiet",
                "--no-overwrites",
                "--paths",
            ])
            .arg(&staging)
            .args([
                "--output",
                "media.%(ext)s",
                "--print",
                "after_move:filepath",
                "--",
                url,
            ]);
        let stdout = self.execute(&mut command)?;
        validate_download_output(&stdout, &staging)
    }

    fn apply_authentication(&self, command: &mut Command) {
        if let Some(cookie_file) = &self.cookie_file {
            command.arg("--cookies").arg(cookie_file);
        }
    }

    fn apply_request_pacing(&self, command: &mut Command) {
        if let Some(seconds) = self.sleep_requests_seconds {
            command.arg("--sleep-requests").arg(seconds.to_string());
        }
    }

    fn execute(&self, command: &mut Command) -> Result<Vec<u8>, YtDlpError> {
        let mut child = command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(YtDlpError::Spawn)?;
        let stdout = child.stdout.take().ok_or(YtDlpError::MissingPipe)?;
        let stderr = child.stderr.take().ok_or(YtDlpError::MissingPipe)?;
        let stdout_reader = std::thread::spawn(move || read_limited(stdout));
        let stderr_reader = std::thread::spawn(move || read_limited(stderr));
        let deadline = std::time::Instant::now()
            .checked_add(self.timeout)
            .ok_or(YtDlpError::InvalidTimeout)?;
        loop {
            if let Some(status) = child.try_wait().map_err(YtDlpError::Wait)? {
                let stdout = join_reader(stdout_reader)?;
                let stderr = join_reader(stderr_reader)?;
                if !status.success() {
                    let stderr = String::from_utf8_lossy(&stderr).trim().to_owned();
                    return Err(YtDlpError::Provider {
                        kind: classify_failure(&stderr),
                        message: stderr,
                    });
                }
                return Ok(stdout);
            }
            if std::time::Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                let _ = join_reader(stdout_reader);
                let _ = join_reader(stderr_reader);
                return Err(YtDlpError::Provider {
                    kind: ProviderFailureKind::Timeout,
                    message: format!("yt-dlp exceeded its deadline of {:?}", self.timeout),
                });
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

fn validate_download_output(bytes: &[u8], staging: &Path) -> Result<DownloadedMedia, YtDlpError> {
    let output = std::str::from_utf8(bytes).map_err(YtDlpError::DownloadOutputUtf8)?;
    let paths = output
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect::<Vec<_>>();
    if paths.len() != 1 {
        return Err(YtDlpError::DownloadOutputCount(paths.len()));
    }
    let reported = PathBuf::from(paths[0]);
    if !reported.is_absolute() {
        return Err(YtDlpError::DownloadPathNotAbsolute(reported));
    }
    let path = reported
        .canonicalize()
        .map_err(|source| YtDlpError::DownloadedFile {
            path: reported.clone(),
            source,
        })?;
    if path.parent() != Some(staging) {
        return Err(YtDlpError::DownloadPathEscaped {
            staging: staging.to_path_buf(),
            path,
        });
    }
    let metadata = path
        .metadata()
        .map_err(|source| YtDlpError::DownloadedFile {
            path: path.clone(),
            source,
        })?;
    if !metadata.is_file() {
        return Err(YtDlpError::DownloadNotFile(path));
    }
    if metadata.len() == 0 {
        return Err(YtDlpError::DownloadEmpty(path));
    }
    Ok(DownloadedMedia {
        path,
        bytes: metadata.len(),
    })
}

/// One non-empty regular media file retained in acquisition staging.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct DownloadedMedia {
    /// Canonical path contained directly inside the job staging directory.
    pub path: PathBuf,
    /// Exact file length observed after yt-dlp exited successfully.
    pub bytes: u64,
}

fn read_limited(reader: impl Read) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader
        .take((MAX_OUTPUT_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    Ok(bytes)
}

fn join_reader(
    handle: std::thread::JoinHandle<io::Result<Vec<u8>>>,
) -> Result<Vec<u8>, YtDlpError> {
    let bytes = handle.join().map_err(|_| YtDlpError::OutputThread)??;
    if bytes.len() > MAX_OUTPUT_BYTES {
        return Err(YtDlpError::OutputTooLarge);
    }
    Ok(bytes)
}

fn parse_snapshot(bytes: &[u8]) -> Result<SourceSnapshot, YtDlpError> {
    let root: Value = serde_json::from_slice(bytes).map_err(YtDlpError::Json)?;
    let entries = root.get("entries").and_then(Value::as_array);
    let items = match entries {
        Some(entries) => entries
            .iter()
            .map(parse_item)
            .collect::<Result<Vec<_>, _>>()?,
        None => vec![parse_item(&root)?],
    };
    Ok(SourceSnapshot {
        provider: "youtube".into(),
        provider_collection_id: entries.and_then(|_| string_field(&root, "id")),
        title: string_field(&root, "title"),
        items,
    })
}

fn parse_item(value: &Value) -> Result<ProviderItem, YtDlpError> {
    let provider_item_id = string_field(value, "id").ok_or(YtDlpError::MissingField("id"))?;
    let url = string_field(value, "webpage_url")
        .or_else(|| string_field(value, "url"))
        .ok_or(YtDlpError::MissingField("url"))?;
    let duration_ms = value
        .get("duration")
        .and_then(Value::as_f64)
        .and_then(|seconds| {
            (seconds.is_finite() && !seconds.is_sign_negative())
                .then(|| (seconds * 1000.0).round() as u64)
        });
    Ok(ProviderItem {
        provider_item_id,
        url,
        title: string_field(value, "title"),
        duration_ms,
        raw_metadata: value.clone(),
    })
}

fn string_field(value: &Value, field: &str) -> Option<String> {
    value
        .get(field)?
        .as_str()
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

fn classify_failure(stderr: &str) -> ProviderFailureKind {
    let normalized = stderr.to_ascii_lowercase();
    if normalized.contains("private video")
        || normalized.contains("video unavailable")
        || normalized.contains("deleted")
    {
        ProviderFailureKind::PermanentlyUnavailable
    } else if normalized.contains("429") || normalized.contains("too many requests") {
        ProviderFailureKind::RateLimited
    } else if normalized.contains("sign in")
        || normalized.contains("cookie")
        || normalized.contains("token")
    {
        ProviderFailureKind::Authentication
    } else {
        ProviderFailureKind::Transient
    }
}

/// Failure to enumerate a source.
#[derive(Debug, Error)]
pub enum YtDlpError {
    /// The executable could not be started.
    #[error("failed to start yt-dlp: {0}")]
    Spawn(io::Error),
    /// Waiting for the subprocess failed.
    #[error("failed while waiting for yt-dlp: {0}")]
    Wait(io::Error),
    /// A subprocess output pipe was unexpectedly unavailable.
    #[error("yt-dlp output pipe was unavailable")]
    MissingPipe,
    /// An output reader thread stopped unexpectedly.
    #[error("yt-dlp output reader stopped unexpectedly")]
    OutputThread,
    /// Captured output could not be read.
    #[error("failed to read yt-dlp output: {0}")]
    Read(#[from] io::Error),
    /// Captured output exceeded the safety bound.
    #[error("yt-dlp output exceeded the 16 MiB limit")]
    OutputTooLarge,
    /// Provider JSON was malformed.
    #[error("yt-dlp returned malformed JSON: {0}")]
    Json(serde_json::Error),
    /// A required provider field was absent.
    #[error("yt-dlp output omitted required field {0}")]
    MissingField(&'static str),
    /// yt-dlp returned a classified provider failure.
    #[error("provider failure ({kind:?}): {message}")]
    Provider {
        /// Conservative failure class.
        kind: ProviderFailureKind,
        /// Bounded provider diagnostic.
        message: String,
    },
    /// The configured deadline cannot be represented by the monotonic clock.
    #[error("yt-dlp timeout is too large")]
    InvalidTimeout,
    /// The requested staging directory could not be resolved.
    #[error("failed to access acquisition staging directory {path}: {source}")]
    Staging {
        /// Requested staging path.
        path: PathBuf,
        /// Underlying filesystem error.
        source: io::Error,
    },
    /// The resolved staging path was not a directory.
    #[error("acquisition staging path is not a directory: {0}")]
    StagingNotDirectory(PathBuf),
    /// yt-dlp download output was not UTF-8.
    #[error("yt-dlp download path output was not valid UTF-8: {0}")]
    DownloadOutputUtf8(std::str::Utf8Error),
    /// yt-dlp did not report exactly one downloaded path.
    #[error("yt-dlp reported {0} download paths; expected exactly one")]
    DownloadOutputCount(usize),
    /// yt-dlp reported a relative path that cannot be safely scoped.
    #[error("yt-dlp reported a non-absolute download path: {0}")]
    DownloadPathNotAbsolute(PathBuf),
    /// The reported download path could not be inspected.
    #[error("failed to inspect yt-dlp download {path}: {source}")]
    DownloadedFile {
        /// Reported media path.
        path: PathBuf,
        /// Underlying filesystem error.
        source: io::Error,
    },
    /// The reported download resolved outside its job staging directory.
    #[error("yt-dlp download escaped staging {staging}: {path}")]
    DownloadPathEscaped {
        /// Canonical staging directory.
        staging: PathBuf,
        /// Canonical reported media path.
        path: PathBuf,
    },
    /// The reported download was not a regular file.
    #[error("yt-dlp download is not a regular file: {0}")]
    DownloadNotFile(PathBuf),
    /// The reported media file was empty.
    #[error("yt-dlp download is empty: {0}")]
    DownloadEmpty(PathBuf),
}

impl YtDlpError {
    /// Returns the conservative domain classification for this adapter failure.
    #[must_use]
    pub const fn kind(&self) -> ProviderFailureKind {
        match self {
            Self::Provider { kind, .. } => *kind,
            Self::Json(_)
            | Self::MissingField(_)
            | Self::MissingPipe
            | Self::OutputThread
            | Self::DownloadOutputUtf8(_)
            | Self::DownloadOutputCount(_)
            | Self::DownloadPathNotAbsolute(_)
            | Self::DownloadPathEscaped { .. } => ProviderFailureKind::Extraction,
            Self::Spawn(_)
            | Self::Wait(_)
            | Self::Read(_)
            | Self::OutputTooLarge
            | Self::InvalidTimeout
            | Self::Staging { .. }
            | Self::StagingNotDirectory(_)
            | Self::DownloadedFile { .. }
            | Self::DownloadNotFile(_)
            | Self::DownloadEmpty(_) => ProviderFailureKind::Transient,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_stored_video_and_playlist_fixtures() -> Result<(), YtDlpError> {
        let video = parse_snapshot(include_bytes!("../tests/fixtures/yt-dlp/video.json"))?;
        let playlist = parse_snapshot(include_bytes!("../tests/fixtures/yt-dlp/playlist.json"))?;
        assert_eq!(video.items.len(), 1);
        assert_eq!(video.items[0].duration_ms, Some(123_500));
        assert_eq!(playlist.provider_collection_id.as_deref(), Some("pl001"));
        assert_eq!(playlist.items.len(), 2);
        Ok(())
    }

    #[test]
    fn classifies_stored_provider_failures_conservatively() {
        assert_eq!(
            classify_failure(include_str!("../tests/fixtures/yt-dlp/private.stderr")),
            ProviderFailureKind::PermanentlyUnavailable
        );
        assert_eq!(
            classify_failure(include_str!("../tests/fixtures/yt-dlp/rate-limit.stderr")),
            ProviderFailureKind::RateLimited
        );
        assert_eq!(
            classify_failure("unexpected extractor breakage"),
            ProviderFailureKind::Transient
        );
    }
}
