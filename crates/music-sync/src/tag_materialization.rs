//! Source-preserving canonical metadata materialization through ffmpeg stream copy.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use serde::Serialize;
use thiserror::Error;

use crate::acquisition::{ValidatedStagedMedia, validate_staged_media};
use crate::content_hash::ContentHasher;
use crate::media_probe::MediaProbe;
use crate::persistence::{
    Database, DatabaseError, MetadataMaterializationCandidate, MetadataMaterializationIntent,
};

const MAX_DIAGNOSTIC_BYTES: u64 = 1024 * 1024;

/// Bounded ffmpeg stream-copy boundary for canonical container metadata.
pub trait MetadataRemuxer {
    /// Writes one incomplete derived file without replacing its source.
    fn remux(
        &self,
        source: &Path,
        destination: &Path,
        metadata: &CanonicalTagSnapshot,
        artwork: Option<&CanonicalArtworkInput>,
    ) -> Result<(), MetadataRemuxError>;
}

/// Exact selected canonical fields passed to the remux boundary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CanonicalTagSnapshot {
    /// Canonical title.
    pub title: String,
    /// Ordered canonical artist credit.
    pub artist: String,
    /// Explicit canonical/provider album; absent is not fabricated.
    pub album: Option<String>,
    /// Explicit provider/canonical genres; never inferred from title text.
    pub genres: Vec<String>,
    /// Auditable artist display provenance.
    pub artist_provenance: Option<String>,
    /// Release date at available precision.
    pub date: Option<String>,
    /// Strong MusicBrainz recording identity.
    pub musicbrainz_recording_id: Option<String>,
    /// Normalized ISRC.
    pub isrc: Option<String>,
}

/// Selected canonical release artwork supplied to container-specific ffmpeg mapping.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanonicalArtworkInput {
    /// Validated cached image path.
    pub path: PathBuf,
    /// Magic-byte-derived image MIME type.
    pub mime_type: String,
}

/// Blocking ffmpeg adapter constrained to stream copy and bounded diagnostics.
#[derive(Debug, Clone)]
pub struct FfmpegMetadataRemuxer {
    executable: PathBuf,
    timeout: Duration,
}

impl FfmpegMetadataRemuxer {
    /// Creates an adapter with explicit executable and deadline.
    #[must_use]
    pub const fn new(executable: PathBuf, timeout: Duration) -> Self {
        Self {
            executable,
            timeout,
        }
    }
}

impl MetadataRemuxer for FfmpegMetadataRemuxer {
    fn remux(
        &self,
        source: &Path,
        destination: &Path,
        metadata: &CanonicalTagSnapshot,
        artwork: Option<&CanonicalArtworkInput>,
    ) -> Result<(), MetadataRemuxError> {
        if destination.exists() {
            return Err(MetadataRemuxError::ExistingDestination(destination.into()));
        }
        let mut command = Command::new(&self.executable);
        command
            .args(["-nostdin", "-v", "error", "-n", "-i"])
            .arg(source);
        let extension = source
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        let mut ffmetadata_path = None;
        let mut metadata_in_file = false;
        if let Some(artwork) = artwork {
            if matches!(extension.as_str(), "opus" | "ogg" | "oga") {
                let picture = metadata_block_picture(artwork)?;
                let path = destination.with_extension("music-sync.ffmetadata");
                write_ffmetadata(&path, metadata, &picture)?;
                command.args(["-f", "ffmetadata", "-i"]).arg(&path).args([
                    "-map",
                    "0:a:0",
                    "-c",
                    "copy",
                    "-map_metadata",
                    "1",
                ]);
                ffmetadata_path = Some(path);
                metadata_in_file = true;
            } else {
                command.arg("-i").arg(&artwork.path).args([
                    "-map",
                    "0:a:0",
                    "-map",
                    "1:v:0",
                    "-c:a",
                    "copy",
                    "-c:v",
                    "copy",
                    "-disposition:v:0",
                    "attached_pic",
                    "-map_metadata",
                    "0",
                ]);
            }
        } else {
            command.args(["-map", "0", "-c", "copy", "-map_metadata", "0"]);
        }
        if !metadata_in_file {
            command
                .arg("-metadata")
                .arg(format!("title={}", metadata.title))
                .arg("-metadata")
                .arg(format!("artist={}", metadata.artist));
            if let Some(value) = &metadata.album {
                command.arg("-metadata").arg(format!("album={value}"));
            }
            if !metadata.genres.is_empty() {
                command
                    .arg("-metadata")
                    .arg(format!("genre={}", metadata.genres.join("; ")));
            }
            if let Some(value) = &metadata.date {
                command.arg("-metadata").arg(format!("date={value}"));
            }
            if let Some(value) = &metadata.musicbrainz_recording_id {
                command
                    .arg("-metadata")
                    .arg(format!("MUSICBRAINZ_TRACKID={value}"));
            }
            if let Some(value) = &metadata.isrc {
                command.arg("-metadata").arg(format!("ISRC={value}"));
            }
        }
        command
            .arg(destination)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        let mut child = command
            .spawn()
            .map_err(|source| MetadataRemuxError::Spawn {
                executable: self.executable.clone(),
                source,
            })?;
        let stderr = child.stderr.take().ok_or(MetadataRemuxError::MissingPipe)?;
        let reader = thread::spawn(move || read_bounded(stderr));
        let deadline = Instant::now()
            .checked_add(self.timeout)
            .ok_or(MetadataRemuxError::InvalidTimeout)?;
        let status = loop {
            if let Some(status) = child.try_wait().map_err(MetadataRemuxError::Wait)? {
                break status;
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                let _ = join_reader(reader)?;
                remove_owned_metadata(ffmetadata_path.as_deref())?;
                return Err(MetadataRemuxError::Timeout(self.timeout));
            }
            thread::sleep(Duration::from_millis(10));
        };
        let stderr = join_reader(reader)?;
        remove_owned_metadata(ffmetadata_path.as_deref())?;
        if !status.success() {
            return Err(MetadataRemuxError::Failed {
                code: status.code(),
                stderr: String::from_utf8_lossy(&stderr).trim().into(),
            });
        }
        Ok(())
    }
}

fn write_ffmetadata(
    path: &Path,
    metadata: &CanonicalTagSnapshot,
    picture: &str,
) -> Result<(), MetadataRemuxError> {
    if path.exists() {
        fs::remove_file(path).map_err(|source| MetadataRemuxError::ArtworkRead {
            path: path.into(),
            source,
        })?;
    }
    let escape = |value: &str| {
        value
            .replace('\\', "\\\\")
            .replace('=', "\\=")
            .replace(';', "\\;")
            .replace('#', "\\#")
            .replace('\n', "\\n")
    };
    let mut text = format!(
        ";FFMETADATA1\ntitle={}\nartist={}\nMETADATA_BLOCK_PICTURE={}\n",
        escape(&metadata.title),
        escape(&metadata.artist),
        picture
    );
    if let Some(value) = &metadata.album {
        text.push_str(&format!("album={}\n", escape(value)));
    }
    if !metadata.genres.is_empty() {
        text.push_str(&format!("genre={}\n", escape(&metadata.genres.join("; "))));
    }
    if let Some(value) = &metadata.date {
        text.push_str(&format!("date={}\n", escape(value)));
    }
    if let Some(value) = &metadata.musicbrainz_recording_id {
        text.push_str(&format!("MUSICBRAINZ_TRACKID={}\n", escape(value)));
    }
    if let Some(value) = &metadata.isrc {
        text.push_str(&format!("ISRC={}\n", escape(value)));
    }
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|source| MetadataRemuxError::ArtworkRead {
            path: path.into(),
            source,
        })?;
    file.write_all(text.as_bytes())
        .and_then(|()| file.sync_all())
        .map_err(|source| MetadataRemuxError::ArtworkRead {
            path: path.into(),
            source,
        })
}

fn remove_owned_metadata(path: Option<&Path>) -> Result<(), MetadataRemuxError> {
    if let Some(path) = path {
        fs::remove_file(path).map_err(|source| MetadataRemuxError::ArtworkRead {
            path: path.into(),
            source,
        })?;
    }
    Ok(())
}

/// Executes a bounded stable set of source-preserving canonical remuxes.
pub fn materialize_canonical_tags(
    database: &mut Database,
    remuxer: &dyn MetadataRemuxer,
    probe: &dyn MediaProbe,
    hasher: &dyn ContentHasher,
    state_directory: &Path,
    maximum_recordings: usize,
) -> Result<MetadataMaterializationReport, MetadataMaterializationError> {
    if maximum_recordings == 0 {
        return Err(MetadataMaterializationError::InvalidLimit);
    }
    let candidates = database.metadata_materialization_candidates(maximum_recordings)?;
    let mut report = MetadataMaterializationReport {
        selected: candidates.len() as u64,
        ..Default::default()
    };
    for candidate in candidates {
        match process_one(
            database,
            remuxer,
            probe,
            hasher,
            state_directory,
            &candidate,
        ) {
            Ok(inserted) => {
                report.committed += u64::from(inserted);
                report.recovered += u64::from(!inserted);
            }
            Err(error) => {
                database
                    .defer_metadata_materialization(candidate.recording_id, &error.to_string())?;
                report.deferred += 1;
                report.failures.push(MetadataMaterializationFailure {
                    recording_id: candidate.recording_id,
                    message: error.to_string(),
                });
            }
        }
    }
    Ok(report)
}

fn process_one(
    database: &mut Database,
    remuxer: &dyn MetadataRemuxer,
    probe: &dyn MediaProbe,
    hasher: &dyn ContentHasher,
    state_directory: &Path,
    candidate: &MetadataMaterializationCandidate,
) -> Result<bool, MetadataMaterializationError> {
    let intent = if candidate.prepared {
        database
            .prepared_metadata_materialization(candidate.recording_id)?
            .ok_or(MetadataMaterializationError::MissingPreparedIntent(
                candidate.recording_id,
            ))?
    } else {
        let source_hash = hasher.hash(&candidate.source_path)?;
        if hex_sha256(source_hash.sha256) != candidate.source_sha256 {
            return Err(MetadataMaterializationError::SourceChanged(
                candidate.source_path.clone(),
            ));
        }
        let history_path = history_path(state_directory, candidate)?;
        preserve_source(
            &candidate.source_path,
            &history_path,
            &candidate.source_sha256,
            hasher,
        )?;
        let staged_path = staged_path(candidate)?;
        if staged_path.exists() && !candidate.staging_reserved {
            return Err(MetadataMaterializationError::UnknownStaging(staged_path));
        }
        database.reserve_metadata_materialization_staging(candidate.recording_id, &staged_path)?;
        let snapshot = CanonicalTagSnapshot {
            title: candidate.title.clone(),
            artist: candidate.artist_credit.clone(),
            album: candidate.release_title.clone(),
            genres: serde_json::from_str(&candidate.genres_json)?,
            artist_provenance: candidate.artist_provenance.clone(),
            date: candidate.release_date.clone(),
            musicbrainz_recording_id: candidate.musicbrainz_recording_id.clone(),
            isrc: candidate.isrc.clone(),
        };
        let artwork = canonical_artwork(state_directory, candidate)?;
        let existing_validated = if staged_path.exists() {
            match validate_staged_media(&staged_path, probe, hasher) {
                Ok(validated) if validate_stream_copy(candidate, &validated).is_ok() => {
                    Some(validated)
                }
                Ok(_) | Err(_) => {
                    fs::remove_file(&staged_path).map_err(|source| {
                        MetadataMaterializationError::Io {
                            path: staged_path.clone(),
                            source,
                        }
                    })?;
                    None
                }
            }
        } else {
            None
        };
        if existing_validated.is_none() {
            remuxer.remux(
                &candidate.source_path,
                &staged_path,
                &snapshot,
                artwork.as_ref(),
            )?;
        }
        let validated = match existing_validated {
            Some(validated) => validated,
            None => validate_staged_media(&staged_path, probe, hasher)?,
        };
        validate_stream_copy(candidate, &validated)?;
        let intent = MetadataMaterializationIntent {
            history_path,
            staged_path,
            validated,
            canonical_snapshot_json: serde_json::to_string(&snapshot)?,
        };
        database.prepare_metadata_materialization(candidate, &intent)?;
        intent
    };
    preserve_source(
        &candidate.source_path,
        &intent.history_path,
        &candidate.source_sha256,
        hasher,
    )?;
    commit_visible(candidate, &intent, hasher)?;
    Ok(database
        .commit_metadata_materialization(candidate.recording_id)?
        .inserted)
}

fn canonical_artwork(
    state: &Path,
    candidate: &MetadataMaterializationCandidate,
) -> Result<Option<CanonicalArtworkInput>, MetadataMaterializationError> {
    let (Some(relative), Some(mime)) = (
        &candidate.artwork_relative_path,
        &candidate.artwork_mime_type,
    ) else {
        return Ok(None);
    };
    let requested = state.join(relative);
    let path = requested
        .canonicalize()
        .map_err(|source| MetadataMaterializationError::Io {
            path: requested.clone(),
            source,
        })?;
    let root = state
        .canonicalize()
        .map_err(|source| MetadataMaterializationError::Io {
            path: state.into(),
            source,
        })?;
    if !path.starts_with(&root) {
        return Err(MetadataMaterializationError::ArtworkEscaped(path));
    }
    Ok(Some(CanonicalArtworkInput {
        path,
        mime_type: mime.clone(),
    }))
}

fn metadata_block_picture(artwork: &CanonicalArtworkInput) -> Result<String, MetadataRemuxError> {
    let data = fs::read(&artwork.path).map_err(|source| MetadataRemuxError::ArtworkRead {
        path: artwork.path.clone(),
        source,
    })?;
    if data.is_empty() || data.len() > 20 * 1024 * 1024 {
        return Err(MetadataRemuxError::InvalidArtwork);
    }
    let mime = artwork.mime_type.as_bytes();
    let mime_len = u32::try_from(mime.len()).map_err(|_| MetadataRemuxError::InvalidArtwork)?;
    let data_len = u32::try_from(data.len()).map_err(|_| MetadataRemuxError::InvalidArtwork)?;
    let mut block = Vec::with_capacity(data.len() + 40);
    block.extend_from_slice(&3_u32.to_be_bytes());
    block.extend_from_slice(&mime_len.to_be_bytes());
    block.extend_from_slice(mime);
    block.extend_from_slice(&0_u32.to_be_bytes());
    for _ in 0..4 {
        block.extend_from_slice(&0_u32.to_be_bytes());
    }
    block.extend_from_slice(&data_len.to_be_bytes());
    block.extend_from_slice(&data);
    Ok(base64_encode(&block))
}

fn base64_encode(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut result = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let a = chunk[0];
        let b = *chunk.get(1).unwrap_or(&0);
        let c = *chunk.get(2).unwrap_or(&0);
        result.push(TABLE[(a >> 2) as usize] as char);
        result.push(TABLE[(((a & 3) << 4) | (b >> 4)) as usize] as char);
        result.push(if chunk.len() > 1 {
            TABLE[(((b & 15) << 2) | (c >> 6)) as usize] as char
        } else {
            '='
        });
        result.push(if chunk.len() > 2 {
            TABLE[(c & 63) as usize] as char
        } else {
            '='
        });
    }
    result
}

fn validate_stream_copy(
    source: &MetadataMaterializationCandidate,
    result: &ValidatedStagedMedia,
) -> Result<(), MetadataMaterializationError> {
    if source.codec.as_deref() != Some(&result.codec)
        || source.sample_rate_hz != result.sample_rate_hz.map(i64::from)
        || source.channels != result.channels.map(i64::from)
    {
        return Err(MetadataMaterializationError::AudioPropertiesChanged);
    }
    if let (Some(left), Some(right)) = (
        source.duration_ms,
        result.duration_ms.and_then(|v| i64::try_from(v).ok()),
    ) && (left - right).abs() > 1000
    {
        return Err(MetadataMaterializationError::AudioPropertiesChanged);
    }
    Ok(())
}

fn history_path(
    state: &Path,
    candidate: &MetadataMaterializationCandidate,
) -> Result<PathBuf, MetadataMaterializationError> {
    let extension = candidate
        .source_path
        .extension()
        .and_then(|v| v.to_str())
        .ok_or_else(|| MetadataMaterializationError::InvalidPath(candidate.source_path.clone()))?;
    Ok(state
        .join("artifact-history")
        .join(&candidate.source_sha256[..2])
        .join(format!("{}.{}", candidate.source_sha256, extension)))
}

fn staged_path(
    candidate: &MetadataMaterializationCandidate,
) -> Result<PathBuf, MetadataMaterializationError> {
    let parent = candidate
        .source_path
        .parent()
        .ok_or_else(|| MetadataMaterializationError::InvalidPath(candidate.source_path.clone()))?;
    let stem = candidate
        .source_path
        .file_stem()
        .and_then(|v| v.to_str())
        .ok_or_else(|| MetadataMaterializationError::InvalidPath(candidate.source_path.clone()))?;
    let extension = candidate
        .source_path
        .extension()
        .and_then(|v| v.to_str())
        .ok_or_else(|| MetadataMaterializationError::InvalidPath(candidate.source_path.clone()))?;
    Ok(parent.join(format!(
        ".{stem}.music-sync-tags-{}.{}",
        candidate.recording_id, extension
    )))
}

fn preserve_source(
    source: &Path,
    history: &Path,
    expected: &str,
    hasher: &dyn ContentHasher,
) -> Result<(), MetadataMaterializationError> {
    if history.exists() {
        return verify_hash(history, expected, hasher);
    }
    let parent = history
        .parent()
        .ok_or_else(|| MetadataMaterializationError::InvalidPath(history.into()))?;
    fs::create_dir_all(parent).map_err(|source| MetadataMaterializationError::Io {
        path: parent.into(),
        source,
    })?;
    let temporary = history.with_extension("music-sync-copy");
    if temporary.exists() && verify_hash(&temporary, expected, hasher).is_err() {
        fs::remove_file(&temporary).map_err(|source| MetadataMaterializationError::Io {
            path: temporary.clone(),
            source,
        })?;
    }
    if !temporary.exists() {
        let mut input = File::open(source).map_err(|error| MetadataMaterializationError::Io {
            path: source.into(),
            source: error,
        })?;
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|source| MetadataMaterializationError::Io {
                path: temporary.clone(),
                source,
            })?;
        io::copy(&mut input, &mut output)
            .and_then(|_| output.sync_all())
            .map_err(|source| MetadataMaterializationError::Io {
                path: temporary.clone(),
                source,
            })?;
    }
    verify_hash(&temporary, expected, hasher)?;
    match fs::hard_link(&temporary, history) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            verify_hash(history, expected, hasher)?
        }
        Err(source) => {
            return Err(MetadataMaterializationError::Io {
                path: history.into(),
                source,
            });
        }
    }
    fs::remove_file(&temporary).map_err(|source| MetadataMaterializationError::Io {
        path: temporary,
        source,
    })?;
    Ok(())
}

fn hex_sha256(bytes: [u8; 32]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
fn verify_hash(
    path: &Path,
    expected: &str,
    hasher: &dyn ContentHasher,
) -> Result<(), MetadataMaterializationError> {
    if hex_sha256(hasher.hash(path)?.sha256) == expected {
        Ok(())
    } else {
        Err(MetadataMaterializationError::HashMismatch(path.into()))
    }
}

fn commit_visible(
    candidate: &MetadataMaterializationCandidate,
    intent: &MetadataMaterializationIntent,
    hasher: &dyn ContentHasher,
) -> Result<(), MetadataMaterializationError> {
    let current = hex_sha256(hasher.hash(&candidate.source_path)?.sha256);
    if current == intent.validated.sha256 {
        return Ok(());
    }
    if current != candidate.source_sha256 {
        return Err(MetadataMaterializationError::SourceChanged(
            candidate.source_path.clone(),
        ));
    }
    verify_hash(&intent.staged_path, &intent.validated.sha256, hasher)?;
    fs::rename(&intent.staged_path, &candidate.source_path).map_err(|source| {
        MetadataMaterializationError::Io {
            path: candidate.source_path.clone(),
            source,
        }
    })?;
    verify_hash(&candidate.source_path, &intent.validated.sha256, hasher)
}

fn read_bounded(mut reader: impl Read) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader
        .by_ref()
        .take(MAX_DIAGNOSTIC_BYTES + 1)
        .read_to_end(&mut bytes)?;
    Ok(bytes)
}
fn join_reader(
    handle: thread::JoinHandle<io::Result<Vec<u8>>>,
) -> Result<Vec<u8>, MetadataRemuxError> {
    let bytes = handle
        .join()
        .map_err(|_| MetadataRemuxError::OutputThread)??;
    if bytes.len() as u64 > MAX_DIAGNOSTIC_BYTES {
        return Err(MetadataRemuxError::OutputTooLarge);
    }
    Ok(bytes)
}

/// Aggregate effects of one bounded materialization pass.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize)]
pub struct MetadataMaterializationReport {
    /// Selected candidates.
    pub selected: u64,
    /// Newly committed outputs.
    pub committed: u64,
    /// Recovered prepared commits.
    pub recovered: u64,
    /// Deferred failures.
    pub deferred: u64,
    /// Per-recording failures.
    pub failures: Vec<MetadataMaterializationFailure>,
}
/// One isolated materialization failure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MetadataMaterializationFailure {
    /// Durable recording row.
    pub recording_id: i64,
    /// Auditable message.
    pub message: String,
}

/// Bounded ffmpeg failure.
#[derive(Debug, Error)]
pub enum MetadataRemuxError {
    /// Output already exists and is never overwritten by ffmpeg.
    #[error("remux destination already exists: {0}")]
    ExistingDestination(PathBuf),
    /// Subprocess spawn failed.
    #[error("failed to start {executable}: {source}")]
    Spawn {
        /// Executable path.
        executable: PathBuf,
        /// Underlying error.
        source: io::Error,
    },
    /// Pipe was unavailable.
    #[error("ffmpeg diagnostic pipe unavailable")]
    MissingPipe,
    /// Wait failed.
    #[error("failed while waiting for ffmpeg: {0}")]
    Wait(io::Error),
    /// Deadline overflowed.
    #[error("ffmpeg timeout is too large")]
    InvalidTimeout,
    /// Deadline elapsed.
    #[error("ffmpeg exceeded timeout {0:?}")]
    Timeout(Duration),
    /// Reader thread failed.
    #[error("ffmpeg diagnostic reader failed")]
    OutputThread,
    /// Diagnostic exceeded limit.
    #[error("ffmpeg diagnostic exceeded its byte limit")]
    OutputTooLarge,
    /// Subprocess returned failure.
    #[error("ffmpeg failed with status {code:?}: {stderr}")]
    Failed {
        /// Exit code.
        code: Option<i32>,
        /// Bounded diagnostic.
        stderr: String,
    },
    /// Diagnostic read failed.
    #[error("failed to read ffmpeg diagnostic: {0}")]
    Read(#[from] io::Error),
    /// Selected cached artwork could not be read.
    #[error("failed to read canonical artwork {path}: {source}")]
    ArtworkRead {
        /// Artwork path.
        path: PathBuf,
        /// Underlying error.
        source: io::Error,
    },
    /// Selected artwork exceeded supported constraints.
    #[error("canonical artwork is empty or exceeds its byte limit")]
    InvalidArtwork,
}

/// Materialization, validation, or commit failure.
#[derive(Debug, Error)]
pub enum MetadataMaterializationError {
    /// Limit must be positive.
    #[error("maximum recordings must be greater than zero")]
    InvalidLimit,
    /// Database failure.
    #[error(transparent)]
    Database(#[from] DatabaseError),
    /// Remux failure.
    #[error(transparent)]
    Remux(#[from] MetadataRemuxError),
    /// Hash failure.
    #[error(transparent)]
    Hash(#[from] crate::content_hash::ContentHashError),
    /// Validation failure.
    #[error(transparent)]
    Validation(#[from] crate::acquisition::StagedMediaValidationError),
    /// Snapshot serialization failed.
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    /// Source bytes changed.
    #[error("source artifact changed before commit: {0}")]
    SourceChanged(PathBuf),
    /// Stream-copy output changed audio properties.
    #[error("stream-copy output changed codec, sample rate, channels, or duration")]
    AudioPropertiesChanged,
    /// Path shape is unsafe.
    #[error("invalid materialization path: {0}")]
    InvalidPath(PathBuf),
    /// Preserved or staged bytes contradict their expected digest.
    #[error("materialization hash mismatch: {0}")]
    HashMismatch(PathBuf),
    /// Durable prepared row was missing.
    #[error("prepared materialization missing for recording {0}")]
    MissingPreparedIntent(i64),
    /// A hidden path existed before music-sync durably reserved it.
    #[error("unknown existing metadata staging file is preserved: {0}")]
    UnknownStaging(PathBuf),
    /// Canonical artwork resolved outside application state.
    #[error("canonical artwork escaped application state: {0}")]
    ArtworkEscaped(PathBuf),
    /// Filesystem effect failed.
    #[error("materialization filesystem failure at {path}: {source}")]
    Io {
        /// Affected path.
        path: PathBuf,
        /// Underlying error.
        source: io::Error,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content_hash::{ContentHasher, Sha256FileHasher};

    #[test]
    fn visible_commit_recovers_only_the_prepared_result_hash()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let source = directory.path().join("track.opus");
        let staged = directory.path().join(".track.tags.opus");
        fs::write(&source, b"source")?;
        fs::write(&staged, b"result")?;
        let source_hash = Sha256FileHasher.hash(&source)?;
        let result_hash = Sha256FileHasher.hash(&staged)?;
        let hex = |bytes: [u8; 32]| bytes.iter().map(|byte| format!("{byte:02x}")).collect();
        let candidate = MetadataMaterializationCandidate {
            recording_id: 1,
            source_artifact_id: 1,
            source_path: source.clone(),
            source_sha256: hex(source_hash.sha256),
            codec: Some("opus".into()),
            duration_ms: Some(1_000),
            sample_rate_hz: Some(48_000),
            channels: Some(2),
            title: "Title".into(),
            artist_credit: "Artist".into(),
            release_title: Some("Album".into()),
            release_date: None,
            genres_json: "[]".into(),
            artist_provenance: Some("artist".into()),
            musicbrainz_recording_id: None,
            isrc: None,
            prepared: true,
            artwork_relative_path: None,
            artwork_mime_type: None,
            staging_reserved: false,
        };
        let intent = MetadataMaterializationIntent {
            history_path: directory.path().join("history.opus"),
            staged_path: staged,
            validated: ValidatedStagedMedia {
                path: directory.path().join(".track.tags.opus"),
                sha256: hex(result_hash.sha256),
                bytes: result_hash.bytes_hashed,
                codec: "opus".into(),
                duration_ms: Some(1_000),
                sample_rate_hz: Some(48_000),
                channels: Some(2),
                musicbrainz_recording_id: None,
                isrc: None,
            },
            canonical_snapshot_json: "{}".into(),
        };
        commit_visible(&candidate, &intent, &Sha256FileHasher)?;
        assert_eq!(fs::read(&source)?, b"result");
        commit_visible(&candidate, &intent, &Sha256FileHasher)?;
        assert_eq!(fs::read(&source)?, b"result");
        Ok(())
    }

    #[test]
    fn base64_encoder_handles_padding_boundaries() {
        assert_eq!(base64_encode(b"a"), "YQ==");
        assert_eq!(base64_encode(b"ab"), "YWI=");
        assert_eq!(base64_encode(b"abc"), "YWJj");
    }
}
