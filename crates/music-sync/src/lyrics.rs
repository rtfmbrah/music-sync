//! Bounded lyrics enrichment and crash-safe adjacent sidecar materialization.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::Deserialize;
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::persistence::{
    Database, DatabaseError, LyricsResolutionState, LyricsWorkCandidate, ResolvedLyrics,
};

const MAX_RESPONSE_BYTES: u64 = 2 * 1024 * 1024;
const MAX_LYRICS_BYTES: usize = 2 * 1024 * 1024;

/// Exact canonical signature supplied to a lyrics provider.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct LyricsSignature {
    /// Selected canonical recording title.
    pub track_name: String,
    /// Selected canonical artist credit.
    pub artist_name: String,
    /// Selected canonical release title.
    pub album_name: String,
    /// Probed duration rounded to whole seconds.
    pub duration_seconds: i64,
}

/// External boundary for exact-signature lyrics lookup.
pub trait LyricsProvider {
    /// Resolves one signature without treating returned text as identity evidence.
    fn resolve(&self, signature: &LyricsSignature) -> Result<LyricsLookup, LyricsProviderError>;
}

/// Bounded provider outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LyricsLookup {
    /// Provider has no current record for the signature.
    Unavailable {
        /// Exact bounded response, empty for HTTP 404.
        raw_response_json: String,
    },
    /// Provider explicitly identifies an instrumental recording.
    Instrumental {
        /// Provider record identity.
        provider_entity_id: String,
        /// Exact bounded response JSON.
        raw_response_json: String,
    },
    /// Validated synchronized or plain text candidate.
    Found {
        /// Provider record identity.
        provider_entity_id: String,
        /// Synchronized or plain selection.
        kind: String,
        /// Validated lyrics text.
        content: String,
        /// Exact bounded response JSON.
        raw_response_json: String,
    },
}

/// Blocking sequential LRCLIB `/api/get` adapter.
pub struct Lrclib {
    agent: ureq::Agent,
    base_url: String,
    minimum_interval: Duration,
    previous_request: Mutex<Option<Instant>>,
}

impl Lrclib {
    /// Creates a production adapter with responsible sequential pacing.
    pub fn new(user_agent: &str, timeout: Duration) -> Result<Self, LyricsProviderError> {
        Self::with_endpoint(
            "https://lrclib.net/api",
            user_agent,
            timeout,
            Duration::from_millis(300),
            true,
        )
    }

    /// Creates an explicitly configured endpoint for controlled fixtures or mirrors.
    pub fn with_endpoint(
        base_url: &str,
        user_agent: &str,
        timeout: Duration,
        minimum_interval: Duration,
        https_only: bool,
    ) -> Result<Self, LyricsProviderError> {
        if user_agent.trim().is_empty() {
            return Err(LyricsProviderError::MissingUserAgent);
        }
        let base_url = base_url.trim_end_matches('/');
        if base_url.is_empty() {
            return Err(LyricsProviderError::InvalidEndpoint);
        }
        let config = ureq::Agent::config_builder()
            .timeout_global(Some(timeout))
            .https_only(https_only)
            .user_agent(user_agent)
            .build();
        Ok(Self {
            agent: config.into(),
            base_url: base_url.into(),
            minimum_interval,
            previous_request: Mutex::new(None),
        })
    }
}

impl LyricsProvider for Lrclib {
    fn resolve(&self, signature: &LyricsSignature) -> Result<LyricsLookup, LyricsProviderError> {
        let mut previous = self
            .previous_request
            .lock()
            .map_err(|_| LyricsProviderError::PacingState)?;
        if let Some(at) = *previous {
            std::thread::sleep(self.minimum_interval.saturating_sub(at.elapsed()));
        }
        *previous = Some(Instant::now());
        drop(previous);
        let duration = signature.duration_seconds.to_string();
        let response = self
            .agent
            .get(format!("{}/get", self.base_url))
            .query("track_name", &signature.track_name)
            .query("artist_name", &signature.artist_name)
            .query("album_name", &signature.album_name)
            .query("duration", &duration)
            .call();
        let mut response = match response {
            Ok(response) => response,
            Err(ureq::Error::StatusCode(404)) => {
                return Ok(LyricsLookup::Unavailable {
                    raw_response_json: String::new(),
                });
            }
            Err(error) => return Err(LyricsProviderError::Http(error)),
        };
        let bytes = response
            .body_mut()
            .with_config()
            .limit(MAX_RESPONSE_BYTES)
            .read_to_vec()
            .map_err(LyricsProviderError::Http)?;
        let raw = std::str::from_utf8(&bytes)
            .map_err(LyricsProviderError::Utf8)?
            .to_owned();
        let record: LrclibResponse =
            serde_json::from_slice(&bytes).map_err(LyricsProviderError::Json)?;
        validate_signature(signature, &record)?;
        if record.instrumental {
            return Ok(LyricsLookup::Instrumental {
                provider_entity_id: record.id.to_string(),
                raw_response_json: raw,
            });
        }
        if let Some(content) = validate_content(record.synced_lyrics.as_deref(), true)? {
            return Ok(LyricsLookup::Found {
                provider_entity_id: record.id.to_string(),
                kind: "synchronized".into(),
                content,
                raw_response_json: raw,
            });
        }
        if let Some(content) = validate_content(record.plain_lyrics.as_deref(), false)? {
            return Ok(LyricsLookup::Found {
                provider_entity_id: record.id.to_string(),
                kind: "plain".into(),
                content,
                raw_response_json: raw,
            });
        }
        Err(LyricsProviderError::MissingLyrics)
    }
}

/// Resolves and materializes a bounded stable set of adjacent sidecars.
pub fn resolve_lyrics(
    database: &mut Database,
    provider: &dyn LyricsProvider,
    maximum_recordings: usize,
) -> Result<LyricsReport, LyricsError> {
    if maximum_recordings == 0 {
        return Err(LyricsError::InvalidLimit);
    }
    let candidates = database.lyrics_work_candidates(maximum_recordings)?;
    let mut report = LyricsReport {
        selected: candidates.len() as u64,
        ..LyricsReport::default()
    };
    for work in candidates {
        if let Err(error) = process_one(database, provider, &work, &mut report) {
            database.record_lyrics_resolution_state(
                work.recording_id,
                LyricsResolutionState::Deferred,
                &error.to_string(),
                "",
            )?;
            report.deferred += 1;
            report.failures.push(LyricsFailure {
                recording_id: work.recording_id,
                message: error.to_string(),
            });
        }
    }
    Ok(report)
}

fn process_one(
    database: &mut Database,
    provider: &dyn LyricsProvider,
    work: &LyricsWorkCandidate,
    report: &mut LyricsReport,
) -> Result<(), LyricsError> {
    let (observation_id, content) =
        if let (Some(id), Some(content)) = (work.observation_id, work.selected_content.clone()) {
            (id, content)
        } else {
            let signature = LyricsSignature {
                track_name: work.title.clone(),
                artist_name: work.artist_credit.clone(),
                album_name: work.release_title.clone(),
                duration_seconds: (work.duration_ms + 500) / 1000,
            };
            let signature_json = serde_json::to_string(&signature)?;
            match provider.resolve(&signature)? {
                LyricsLookup::Unavailable { raw_response_json } => {
                    database.record_lyrics_resolution_state(
                        work.recording_id,
                        LyricsResolutionState::Unavailable,
                        "LRCLIB returned no matching lyrics",
                        &raw_response_json,
                    )?;
                    report.unavailable += 1;
                    return Ok(());
                }
                LyricsLookup::Instrumental {
                    provider_entity_id,
                    raw_response_json,
                } => {
                    database.record_lyrics_resolution_state(
                        work.recording_id,
                        LyricsResolutionState::Instrumental,
                        &format!("LRCLIB record {provider_entity_id} is instrumental"),
                        &raw_response_json,
                    )?;
                    report.instrumental += 1;
                    return Ok(());
                }
                LyricsLookup::Found {
                    provider_entity_id,
                    kind,
                    content,
                    raw_response_json,
                } => {
                    let lyrics = ResolvedLyrics {
                        provider_entity_id,
                        kind,
                        content: content.clone(),
                        signature_json,
                        raw_response_json,
                    };
                    (
                        database.record_resolved_lyrics(work.recording_id, &lyrics)?,
                        content,
                    )
                }
            }
        };
    let path = sidecar_path(&work.artifact_path)?;
    let bytes = normalized_bytes(&content)?;
    let sha256 = format!("{:x}", Sha256::digest(&bytes));
    if !work.output_prepared && path.exists() {
        return Err(LyricsError::UnknownSidecar(path));
    }
    let inserted = database.prepare_lyrics_output(
        work,
        observation_id,
        &path,
        &sha256,
        i64::try_from(bytes.len()).map_err(|_| LyricsError::InvalidContent)?,
    )?;
    if inserted {
        create_sidecar_no_clobber(&path, work.recording_id, &bytes)?;
    } else {
        verify_exact(&path, &bytes)?;
    }
    database.commit_lyrics_output(work.recording_id)?;
    report.resolved += 1;
    report.sidecars_committed += 1;
    Ok(())
}

fn sidecar_path(audio: &Path) -> Result<PathBuf, LyricsError> {
    if !audio.is_absolute() || audio.file_stem().is_none() {
        return Err(LyricsError::InvalidArtifactPath(audio.into()));
    }
    let mut path = audio.to_path_buf();
    path.set_extension("lrc");
    Ok(path)
}

fn normalized_bytes(content: &str) -> Result<Vec<u8>, LyricsError> {
    if content.contains('\0') {
        return Err(LyricsError::InvalidContent);
    }
    let normalized = content.replace("\r\n", "\n").replace('\r', "\n");
    let normalized = format!("{}\n", normalized.trim_end());
    if normalized.trim().is_empty() || normalized.len() > MAX_LYRICS_BYTES {
        return Err(LyricsError::InvalidContent);
    }
    Ok(normalized.into_bytes())
}

fn create_sidecar_no_clobber(
    path: &Path,
    recording_id: i64,
    bytes: &[u8],
) -> Result<(), LyricsError> {
    let file_name = path
        .file_name()
        .and_then(|v| v.to_str())
        .ok_or_else(|| LyricsError::InvalidArtifactPath(path.into()))?;
    let temporary = path.with_file_name(format!(".{file_name}.music-sync-{recording_id}.tmp"));
    match OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
    {
        Ok(mut file) => {
            file.write_all(bytes)
                .and_then(|()| file.sync_all())
                .map_err(|source| LyricsError::Io {
                    path: temporary.clone(),
                    source,
                })?;
        }
        Err(source) if source.kind() == std::io::ErrorKind::AlreadyExists => {
            verify_exact(&temporary, bytes)?
        }
        Err(source) => {
            return Err(LyricsError::Io {
                path: temporary,
                source,
            });
        }
    }
    fs::hard_link(&temporary, path).map_err(|source| LyricsError::Io {
        path: path.into(),
        source,
    })?;
    fs::remove_file(&temporary).map_err(|source| LyricsError::Io {
        path: temporary,
        source,
    })?;
    Ok(())
}

fn verify_exact(path: &Path, expected: &[u8]) -> Result<(), LyricsError> {
    let actual = fs::read(path).map_err(|source| LyricsError::Io {
        path: path.into(),
        source,
    })?;
    if actual == expected {
        Ok(())
    } else {
        Err(LyricsError::ExistingSidecarMismatch(path.into()))
    }
}

fn validate_signature(
    signature: &LyricsSignature,
    response: &LrclibResponse,
) -> Result<(), LyricsProviderError> {
    let same = |a: &str, b: &str| a.trim().to_lowercase() == b.trim().to_lowercase();
    if !same(&signature.track_name, &response.track_name)
        || !same(&signature.artist_name, &response.artist_name)
        || !same(&signature.album_name, &response.album_name)
        || (signature.duration_seconds - response.duration.round() as i64).abs() > 2
    {
        return Err(LyricsProviderError::SignatureMismatch);
    }
    Ok(())
}

fn validate_content(
    content: Option<&str>,
    synchronized: bool,
) -> Result<Option<String>, LyricsProviderError> {
    let Some(content) = content.map(str::trim).filter(|v| !v.is_empty()) else {
        return Ok(None);
    };
    if content.len() > MAX_LYRICS_BYTES || content.contains('\0') {
        return Err(LyricsProviderError::InvalidLyrics);
    }
    if synchronized
        && !content
            .lines()
            .any(|line| line.starts_with('[') && line.get(3..4) == Some(":"))
    {
        return Err(LyricsProviderError::InvalidLyrics);
    }
    Ok(Some(content.into()))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LrclibResponse {
    id: u64,
    track_name: String,
    artist_name: String,
    album_name: String,
    duration: f64,
    instrumental: bool,
    plain_lyrics: Option<String>,
    synced_lyrics: Option<String>,
}

/// Aggregate effects of one bounded lyrics pass.
#[derive(Debug, Default, Clone, PartialEq, Eq, serde::Serialize)]
pub struct LyricsReport {
    /// Work candidates selected in stable order.
    pub selected: u64,
    /// Recordings with committed lyrics this pass.
    pub resolved: u64,
    /// Provider-confirmed instrumental recordings.
    pub instrumental: u64,
    /// Recordings without a provider result.
    pub unavailable: u64,
    /// Isolated retryable failures.
    pub deferred: u64,
    /// Newly or recoverably committed sidecars.
    pub sidecars_committed: u64,
    /// Per-recording failures.
    pub failures: Vec<LyricsFailure>,
}

/// One isolated lyrics failure.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct LyricsFailure {
    /// Durable recording row.
    pub recording_id: i64,
    /// Auditable failure message.
    pub message: String,
}

/// LRCLIB boundary failure.
#[derive(Debug, Error)]
pub enum LyricsProviderError {
    /// Required identifying header is empty.
    #[error("LRCLIB User-Agent must not be empty")]
    MissingUserAgent,
    /// Endpoint is empty or invalid.
    #[error("invalid LRCLIB endpoint")]
    InvalidEndpoint,
    /// Sequential pacing mutex was poisoned.
    #[error("lyrics pacing state is unavailable")]
    PacingState,
    /// Request or bounded response read failed.
    #[error("LRCLIB HTTP failure: {0}")]
    Http(ureq::Error),
    /// Response was not UTF-8.
    #[error("LRCLIB response is not UTF-8: {0}")]
    Utf8(std::str::Utf8Error),
    /// Response JSON was malformed.
    #[error("invalid LRCLIB JSON: {0}")]
    Json(serde_json::Error),
    /// Returned signature contradicted canonical input.
    #[error("LRCLIB result contradicts the canonical request signature")]
    SignatureMismatch,
    /// Returned lyrics violated content constraints.
    #[error("LRCLIB result contains invalid lyrics")]
    InvalidLyrics,
    /// Non-instrumental response had no lyrics text.
    #[error("LRCLIB result contains neither synchronized nor plain lyrics")]
    MissingLyrics,
}

/// Lyrics workflow or sidecar failure.
#[derive(Debug, Error)]
pub enum LyricsError {
    /// Work limit must be positive.
    #[error("maximum recordings must be greater than zero")]
    InvalidLimit,
    /// Durable state operation failed.
    #[error(transparent)]
    Database(#[from] DatabaseError),
    /// Provider operation failed.
    #[error(transparent)]
    Provider(#[from] LyricsProviderError),
    /// Signature serialization failed.
    #[error("failed to serialize lyrics signature: {0}")]
    Json(#[from] serde_json::Error),
    /// Managed audio cannot yield a safe adjacent path.
    #[error("invalid managed artifact path: {0}")]
    InvalidArtifactPath(PathBuf),
    /// Selected content failed normalization constraints.
    #[error("lyrics content is empty, unsafe, or exceeds the byte limit")]
    InvalidContent,
    /// A pre-existing unowned sidecar was preserved.
    #[error("unknown existing sidecar is preserved: {0}")]
    UnknownSidecar(PathBuf),
    /// Recoverable owned output contradicted intent.
    #[error("existing owned sidecar bytes do not match durable intent: {0}")]
    ExistingSidecarMismatch(PathBuf),
    /// Filesystem operation failed.
    #[error("sidecar filesystem failure at {path}: {source}")]
    Io {
        /// Affected path.
        path: PathBuf,
        /// Underlying filesystem failure.
        source: std::io::Error,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn synchronized_text_outranks_plain_and_requires_timestamps()
    -> Result<(), Box<dyn std::error::Error>> {
        assert_eq!(
            validate_content(Some("[00:01.20] Line"), true)?,
            Some("[00:01.20] Line".into())
        );
        assert!(validate_content(Some("Line only"), true).is_err());
        assert_eq!(
            validate_content(Some("Line only"), false)?,
            Some("Line only".into())
        );
        Ok(())
    }

    #[test]
    fn normalizes_line_endings_and_appends_one_newline() -> Result<(), Box<dyn std::error::Error>> {
        assert_eq!(normalized_bytes("a\r\nb\r\n")?, b"a\nb\n");
        Ok(())
    }

    #[test]
    fn no_clobber_commit_preserves_existing_sidecar() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("track.lrc");
        fs::write(&path, b"unknown lyrics")?;
        assert!(create_sidecar_no_clobber(&path, 7, b"selected lyrics\n").is_err());
        assert_eq!(fs::read(path)?, b"unknown lyrics");
        Ok(())
    }
}
