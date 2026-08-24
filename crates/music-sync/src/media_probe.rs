//! Bounded `ffprobe` adapter for read-only media inspection.

use std::collections::BTreeMap;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use serde::Deserialize;
use thiserror::Error;

const MAX_CAPTURE_BYTES: u64 = 1024 * 1024;
const MAX_EXECUTABLE_BUSY_RETRIES: usize = 10;

/// Read-only boundary for extracting structural media properties.
pub trait MediaProbe {
    /// Inspects one local file without changing it.
    fn probe(&self, path: &Path) -> Result<MediaProperties, MediaProbeError>;
}

/// Structural and tag-presence information extracted from a media file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaProperties {
    /// Codec name of the first audio stream.
    pub codec: String,
    /// Container duration when ffprobe provides a finite non-negative value.
    pub duration_ms: Option<u64>,
    /// Audio sample rate when available and valid.
    pub sample_rate_hz: Option<u32>,
    /// Audio channel count when available.
    pub channels: Option<u32>,
    /// Whether an attached image stream was observed.
    pub has_embedded_artwork: bool,
    /// Presence of useful tags; values are intentionally not retained in summaries.
    pub tags: MediaTagPresence,
}

/// Presence of useful embedded metadata classes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MediaTagPresence {
    /// At least one of title, artist, or album is present and non-empty.
    pub has_basic_tags: bool,
    /// Structural status of a MusicBrainz recording ID tag.
    pub musicbrainz_recording_id: CanonicalTagStatus,
    /// Structural status of an ISRC tag.
    pub isrc: CanonicalTagStatus,
}

impl MediaTagPresence {
    /// Whether at least one structurally valid canonical identifier was observed.
    #[must_use]
    pub const fn has_canonical_identity(self) -> bool {
        self.musicbrainz_recording_id.is_valid() || self.isrc.is_valid()
    }

    /// Whether either canonical tag was present but structurally malformed.
    #[must_use]
    pub const fn has_malformed_canonical_tag(self) -> bool {
        self.musicbrainz_recording_id.is_malformed() || self.isrc.is_malformed()
    }
}

/// Structural status of one embedded canonical-identifier tag.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum CanonicalTagStatus {
    /// The tag was absent or empty.
    #[default]
    Absent,
    /// The tag value matched the identifier's required structure.
    Valid,
    /// A non-empty tag value did not match the required structure.
    Malformed,
}

impl CanonicalTagStatus {
    const fn is_valid(self) -> bool {
        matches!(self, Self::Valid)
    }

    const fn is_malformed(self) -> bool {
        matches!(self, Self::Malformed)
    }
}

/// Subprocess adapter around an `ffprobe` executable.
#[derive(Debug, Clone)]
pub struct Ffprobe {
    executable: PathBuf,
    timeout: Duration,
}

impl Ffprobe {
    /// Creates an adapter with an explicit executable and per-file timeout.
    #[must_use]
    pub fn new(executable: PathBuf, timeout: Duration) -> Self {
        Self {
            executable,
            timeout,
        }
    }
}

impl MediaProbe for Ffprobe {
    fn probe(&self, path: &Path) -> Result<MediaProperties, MediaProbeError> {
        let mut command = Command::new(&self.executable);
        command
            .args([
                "-v",
                "error",
                "-show_entries",
                "format=duration:format_tags=title,artist,album,track,disc,MusicBrainz_Recording_Id,MusicBrainz_Track_Id,ISRC:stream=index,codec_type,codec_name,sample_rate,channels:stream_disposition=attached_pic",
                "-of",
                "json",
                "-i",
            ])
            .arg(path)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child =
            spawn_with_busy_retry(&mut command).map_err(|source| MediaProbeError::Spawn {
                executable: self.executable.clone(),
                source,
            })?;

        let stdout = child.stdout.take().ok_or(MediaProbeError::MissingPipe)?;
        let stderr = child.stderr.take().ok_or(MediaProbeError::MissingPipe)?;
        let stdout_reader = thread::spawn(move || read_bounded(stdout));
        let stderr_reader = thread::spawn(move || read_bounded(stderr));

        let deadline = Instant::now()
            .checked_add(self.timeout)
            .ok_or(MediaProbeError::InvalidTimeout)?;
        let status = loop {
            if let Some(status) = child.try_wait().map_err(MediaProbeError::Wait)? {
                break status;
            }
            let now = Instant::now();
            if now >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                join_reader(stdout_reader)?;
                join_reader(stderr_reader)?;
                return Err(MediaProbeError::Timeout(self.timeout));
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
            return Err(MediaProbeError::Failed {
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

fn read_bounded(reader: impl Read) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader.take(MAX_CAPTURE_BYTES + 1).read_to_end(&mut bytes)?;
    Ok(bytes)
}

fn join_reader(
    handle: thread::JoinHandle<io::Result<Vec<u8>>>,
) -> Result<Vec<u8>, MediaProbeError> {
    let bytes = handle.join().map_err(|_| MediaProbeError::OutputThread)??;
    if bytes.len() as u64 > MAX_CAPTURE_BYTES {
        return Err(MediaProbeError::OutputTooLarge(MAX_CAPTURE_BYTES));
    }
    Ok(bytes)
}

fn parse_output(bytes: &[u8]) -> Result<MediaProperties, MediaProbeError> {
    let output: ProbeOutput = serde_json::from_slice(bytes).map_err(MediaProbeError::Json)?;
    let audio = output
        .streams
        .iter()
        .find(|stream| stream.codec_type.as_deref() == Some("audio"))
        .ok_or(MediaProbeError::NoAudioStream)?;
    let codec = audio
        .codec_name
        .clone()
        .filter(|codec| !codec.is_empty())
        .ok_or(MediaProbeError::MissingCodec)?;
    let format = output.format.unwrap_or_default();
    let normalized_tags = format
        .tags
        .keys()
        .filter(|key| {
            format
                .tags
                .get(*key)
                .is_some_and(|value| !value.trim().is_empty())
        })
        .map(|key| normalize_tag_key(key))
        .collect::<Vec<_>>();
    let has_basic_tags = normalized_tags
        .iter()
        .any(|key| matches!(key.as_str(), "title" | "artist" | "album"));
    let musicbrainz_recording_id = canonical_tag_status(
        &format.tags,
        &["musicbrainzrecordingid", "musicbrainztrackid"],
        is_musicbrainz_recording_id,
    );
    let isrc = canonical_tag_status(&format.tags, &["isrc"], is_isrc);

    Ok(MediaProperties {
        codec,
        duration_ms: parse_duration_ms(format.duration.as_deref()),
        sample_rate_hz: audio
            .sample_rate
            .as_deref()
            .and_then(|rate| rate.parse().ok()),
        channels: audio.channels,
        has_embedded_artwork: output.streams.iter().any(|stream| {
            stream.codec_type.as_deref() == Some("video")
                && stream.disposition.attached_pic == Some(1)
        }),
        tags: MediaTagPresence {
            has_basic_tags,
            musicbrainz_recording_id,
            isrc,
        },
    })
}

fn canonical_tag_status(
    tags: &BTreeMap<String, String>,
    expected_keys: &[&str],
    validator: fn(&str) -> bool,
) -> CanonicalTagStatus {
    let mut observed = false;
    for (key, value) in tags {
        if !expected_keys.contains(&normalize_tag_key(key).as_str()) || value.trim().is_empty() {
            continue;
        }
        observed = true;
        if !validator(value.trim()) {
            return CanonicalTagStatus::Malformed;
        }
    }
    if observed {
        CanonicalTagStatus::Valid
    } else {
        CanonicalTagStatus::Absent
    }
}

fn is_musicbrainz_recording_id(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(index, byte)| match index {
            8 | 13 | 18 | 23 => byte == b'-',
            _ => byte.is_ascii_hexdigit(),
        })
}

fn is_isrc(value: &str) -> bool {
    let bytes = value.as_bytes();
    match bytes.len() {
        12 => {
            bytes[..2].iter().all(u8::is_ascii_alphabetic)
                && bytes[2..5].iter().all(u8::is_ascii_alphanumeric)
                && bytes[5..].iter().all(u8::is_ascii_digit)
        }
        15 => {
            bytes[..2].iter().all(u8::is_ascii_alphabetic)
                && bytes[2] == b'-'
                && bytes[3..6].iter().all(u8::is_ascii_alphanumeric)
                && bytes[6] == b'-'
                && bytes[7..9].iter().all(u8::is_ascii_digit)
                && bytes[9] == b'-'
                && bytes[10..].iter().all(u8::is_ascii_digit)
        }
        _ => false,
    }
}

fn normalize_tag_key(key: &str) -> String {
    key.chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn parse_duration_ms(duration: Option<&str>) -> Option<u64> {
    let seconds = duration?.parse::<f64>().ok()?;
    if !seconds.is_finite() || seconds.is_sign_negative() {
        return None;
    }
    Some((seconds * 1000.0).round() as u64)
}

#[derive(Debug, Default, Deserialize)]
struct ProbeOutput {
    #[serde(default)]
    streams: Vec<ProbeStream>,
    format: Option<ProbeFormat>,
}

#[derive(Debug, Default, Deserialize)]
struct ProbeStream {
    codec_type: Option<String>,
    codec_name: Option<String>,
    sample_rate: Option<String>,
    channels: Option<u32>,
    #[serde(default)]
    disposition: ProbeDisposition,
}

#[derive(Debug, Default, Deserialize)]
struct ProbeDisposition {
    attached_pic: Option<u8>,
}

#[derive(Debug, Default, Deserialize)]
struct ProbeFormat {
    duration: Option<String>,
    #[serde(default)]
    tags: BTreeMap<String, String>,
}

/// Failure to inspect one media file through the ffprobe boundary.
#[derive(Debug, Error)]
pub enum MediaProbeError {
    /// The subprocess could not be started.
    #[error("failed to start {executable}: {source}")]
    Spawn {
        /// Executable that was requested.
        executable: PathBuf,
        /// Underlying process error.
        source: io::Error,
    },
    /// Waiting for the subprocess failed.
    #[error("failed while waiting for ffprobe: {0}")]
    Wait(io::Error),
    /// Captured output could not be read.
    #[error("failed to read ffprobe output: {0}")]
    Read(#[from] io::Error),
    /// A subprocess pipe was unexpectedly unavailable.
    #[error("ffprobe output pipe was unavailable")]
    MissingPipe,
    /// An output-reader thread failed unexpectedly.
    #[error("ffprobe output reader stopped unexpectedly")]
    OutputThread,
    /// Captured output exceeded the safety bound.
    #[error("ffprobe output exceeded {0} bytes")]
    OutputTooLarge(u64),
    /// The subprocess exceeded its per-file deadline.
    #[error("ffprobe exceeded its per-file timeout of {0:?}")]
    Timeout(Duration),
    /// The configured timeout could not be represented by a monotonic deadline.
    #[error("ffprobe timeout is too large")]
    InvalidTimeout,
    /// ffprobe returned a failure status.
    #[error("ffprobe failed with status {code:?}: {stderr}")]
    Failed {
        /// Process exit code, if available.
        code: Option<i32>,
        /// Bounded stderr text.
        stderr: String,
    },
    /// ffprobe returned malformed JSON.
    #[error("ffprobe returned invalid JSON: {0}")]
    Json(serde_json::Error),
    /// No audio stream was present.
    #[error("ffprobe found no audio stream")]
    NoAudioStream,
    /// The audio stream omitted a codec name.
    #[error("ffprobe audio stream omitted its codec")]
    MissingCodec,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_audio_artwork_duration_and_tag_presence() -> Result<(), MediaProbeError> {
        let properties = parse_output(
            br#"{
                "streams": [
                    {"codec_type":"audio","codec_name":"aac","sample_rate":"44100","channels":2},
                    {"codec_type":"video","codec_name":"mjpeg","disposition":{"attached_pic":1}}
                ],
                "format": {
                    "duration":"12.345",
                    "tags":{"title":"Track","ARTIST":"Artist","MusicBrainz_Recording_Id":"f59c5520-5f46-4d2c-b2c4-822eabf53419"}
                }
            }"#,
        )?;
        assert_eq!(properties.codec, "aac");
        assert_eq!(properties.duration_ms, Some(12_345));
        assert_eq!(properties.sample_rate_hz, Some(44_100));
        assert_eq!(properties.channels, Some(2));
        assert!(properties.has_embedded_artwork);
        assert!(properties.tags.has_basic_tags);
        assert!(properties.tags.has_canonical_identity());
        assert_eq!(
            properties.tags.musicbrainz_recording_id,
            CanonicalTagStatus::Valid
        );
        assert_eq!(properties.tags.isrc, CanonicalTagStatus::Absent);
        Ok(())
    }

    #[test]
    fn classifies_valid_display_isrc_and_malformed_mbid() -> Result<(), MediaProbeError> {
        let properties = parse_output(
            br#"{
                "streams":[{"codec_type":"audio","codec_name":"flac"}],
                "format":{"tags":{"ISRC":"US-ABC-24-12345","musicbrainz recording id":"not-a-uuid"}}
            }"#,
        )?;

        assert_eq!(properties.tags.isrc, CanonicalTagStatus::Valid);
        assert_eq!(
            properties.tags.musicbrainz_recording_id,
            CanonicalTagStatus::Malformed
        );
        assert!(properties.tags.has_canonical_identity());
        assert!(properties.tags.has_malformed_canonical_tag());
        Ok(())
    }

    #[test]
    fn recognizes_musicbrainz_track_id_as_recording_id_alias() -> Result<(), MediaProbeError> {
        let properties = parse_output(
            br#"{
                "streams":[{"codec_type":"audio","codec_name":"opus"}],
                "format":{"tags":{"MUSICBRAINZ_TRACKID":"f59c5520-5f46-4d2c-b2c4-822eabf53419"}}
            }"#,
        )?;

        assert_eq!(
            properties.tags.musicbrainz_recording_id,
            CanonicalTagStatus::Valid
        );
        Ok(())
    }
}
