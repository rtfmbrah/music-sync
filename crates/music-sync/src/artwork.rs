//! Bounded release-artwork resolution and immutable content-addressed caching.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Deserialize;
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::persistence::{
    ArtworkBlob, ArtworkResolutionState, Database, DatabaseError, ResolvedArtwork,
};

const MAX_INDEX_BYTES: u64 = 2 * 1024 * 1024;
const MAX_IMAGE_BYTES: u64 = 20 * 1024 * 1024;

/// External boundary for fetching one deterministic release image.
pub trait ReleaseArtworkProvider {
    /// Returns a selected image, or `None` when the release has no usable art.
    fn fetch(&self, release_mbid: &str) -> Result<ArtworkLookup, ArtworkProviderError>;
}

/// Bounded provider result retained for durable audit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtworkLookup {
    /// Deterministically selected image when present.
    pub selected: Option<FetchedArtwork>,
    /// Exact bounded provider index JSON.
    pub raw_response_json: String,
}

/// Selected provider image bytes and provenance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchedArtwork {
    /// Provider-owned image identity.
    pub source_image_id: String,
    /// Exact URL used for the download.
    pub source_url: String,
    /// Normalized image role.
    pub role: String,
    /// Provider approval flag.
    pub approved: bool,
    /// Bounded response bytes, not yet trusted until magic validation.
    pub bytes: Vec<u8>,
}

/// Blocking bounded Cover Art Archive adapter.
pub struct CoverArtArchive {
    agent: ureq::Agent,
    base_url: String,
    allow_loopback_http: bool,
}

impl CoverArtArchive {
    /// Creates a production adapter for the official HTTPS endpoint.
    pub fn new(user_agent: &str, timeout: Duration) -> Result<Self, ArtworkProviderError> {
        Self::with_endpoint("https://coverartarchive.org", user_agent, timeout, true)
    }

    /// Creates an explicitly configured endpoint for controlled fixtures or mirrors.
    pub fn with_endpoint(
        base_url: &str,
        user_agent: &str,
        timeout: Duration,
        https_only: bool,
    ) -> Result<Self, ArtworkProviderError> {
        if user_agent.trim().is_empty() {
            return Err(ArtworkProviderError::MissingUserAgent);
        }
        let base_url = base_url.trim_end_matches('/');
        if base_url.is_empty() {
            return Err(ArtworkProviderError::InvalidEndpoint);
        }
        let config = ureq::Agent::config_builder()
            .timeout_global(Some(timeout))
            .https_only(https_only)
            .user_agent(user_agent)
            .build();
        Ok(Self {
            agent: config.into(),
            base_url: base_url.into(),
            allow_loopback_http: !https_only && is_loopback_url(base_url),
        })
    }

    fn bounded_get(&self, url: &str, limit: u64) -> Result<Vec<u8>, ArtworkProviderError> {
        let mut response = self
            .agent
            .get(url)
            .call()
            .map_err(ArtworkProviderError::Http)?;
        response
            .body_mut()
            .with_config()
            .limit(limit)
            .read_to_vec()
            .map_err(ArtworkProviderError::Http)
    }
}

impl ReleaseArtworkProvider for CoverArtArchive {
    fn fetch(&self, release_mbid: &str) -> Result<ArtworkLookup, ArtworkProviderError> {
        if !is_mbid(release_mbid) {
            return Err(ArtworkProviderError::InvalidReleaseMbid);
        }
        let index_url = format!("{}/release/{release_mbid}", self.base_url);
        let index = match self.bounded_get(&index_url, MAX_INDEX_BYTES) {
            Ok(index) => index,
            Err(ArtworkProviderError::Http(ureq::Error::StatusCode(404))) => {
                return Ok(ArtworkLookup {
                    selected: None,
                    raw_response_json: String::new(),
                });
            }
            Err(error) => return Err(error),
        };
        let raw_response_json = std::str::from_utf8(&index)
            .map_err(ArtworkProviderError::Utf8)?
            .to_owned();
        let response: IndexResponse =
            serde_json::from_slice(&index).map_err(ArtworkProviderError::Json)?;
        let Some(image) = select_image(response.images) else {
            return Ok(ArtworkLookup {
                selected: None,
                raw_response_json,
            });
        };
        let source_url = secure_image_url(image.preferred_url());
        if !(source_url.starts_with("https://")
            || self.allow_loopback_http && is_loopback_url(&source_url))
        {
            return Err(ArtworkProviderError::InsecureImageUrl);
        }
        let bytes = self.bounded_get(&source_url, MAX_IMAGE_BYTES)?;
        Ok(ArtworkLookup {
            selected: Some(FetchedArtwork {
                source_image_id: image.id,
                source_url,
                role: if image.front {
                    "front"
                } else if image.back {
                    "back"
                } else {
                    "other"
                }
                .into(),
                approved: image.approved,
                bytes,
            }),
            raw_response_json,
        })
    }
}

/// Resolves and caches a stable bounded set of canonical release covers.
pub fn resolve_release_artwork(
    database: &mut Database,
    provider: &dyn ReleaseArtworkProvider,
    state_directory: &Path,
    maximum_releases: usize,
) -> Result<ArtworkResolutionReport, ArtworkResolutionError> {
    if maximum_releases == 0 {
        return Err(ArtworkResolutionError::InvalidLimit);
    }
    let candidates = database.artwork_resolution_candidates(maximum_releases)?;
    let mut report = ArtworkResolutionReport {
        selected: candidates.len() as u64,
        ..ArtworkResolutionReport::default()
    };
    for candidate in candidates {
        let lookup = match provider.fetch(&candidate.musicbrainz_release_id) {
            Ok(lookup) => lookup,
            Err(error) => {
                database.record_artwork_resolution_state(
                    candidate.release_id,
                    ArtworkResolutionState::Deferred,
                    &error.to_string(),
                    "",
                )?;
                report.deferred += 1;
                report.failures.push(ArtworkResolutionFailure {
                    release_id: candidate.release_id,
                    message: error.to_string(),
                });
                continue;
            }
        };
        let Some(fetched) = lookup.selected else {
            database.record_artwork_resolution_state(
                candidate.release_id,
                ArtworkResolutionState::Unavailable,
                "Cover Art Archive returned no usable image",
                &lookup.raw_response_json,
            )?;
            report.unavailable += 1;
            continue;
        };
        let blob = match cache_validated_image(state_directory, &fetched.bytes) {
            Ok(blob) => blob,
            Err(error) => {
                database.record_artwork_resolution_state(
                    candidate.release_id,
                    ArtworkResolutionState::Deferred,
                    &error.to_string(),
                    &lookup.raw_response_json,
                )?;
                report.deferred += 1;
                report.failures.push(ArtworkResolutionFailure {
                    release_id: candidate.release_id,
                    message: error.to_string(),
                });
                continue;
            }
        };
        let artwork = ResolvedArtwork {
            source_image_id: fetched.source_image_id,
            source_url: fetched.source_url,
            role: fetched.role,
            approved: fetched.approved,
        };
        if database.record_resolved_artwork(
            &candidate,
            &artwork,
            &blob,
            &lookup.raw_response_json,
        )? {
            report.resolved += 1;
        }
    }
    Ok(report)
}

fn cache_validated_image(
    state_directory: &Path,
    bytes: &[u8],
) -> Result<ArtworkBlob, ArtworkResolutionError> {
    let (mime_type, extension) = image_type(bytes).ok_or(ArtworkResolutionError::InvalidImage)?;
    let sha256 = format!("{:x}", Sha256::digest(bytes));
    let relative_path = format!("artwork-cache/{}/{}.{}", &sha256[..2], sha256, extension);
    let final_path = state_directory.join(&relative_path);
    let parent = final_path
        .parent()
        .ok_or_else(|| ArtworkResolutionError::InvalidCachePath(final_path.clone()))?;
    fs::create_dir_all(parent).map_err(|source| ArtworkResolutionError::CreateDirectory {
        path: parent.into(),
        source,
    })?;
    match OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&final_path)
    {
        Ok(mut file) => {
            file.write_all(bytes)
                .and_then(|()| file.sync_all())
                .map_err(|source| ArtworkResolutionError::WriteCache {
                    path: final_path.clone(),
                    source,
                })?;
        }
        Err(source) if source.kind() == std::io::ErrorKind::AlreadyExists => {
            let existing =
                fs::read(&final_path).map_err(|source| ArtworkResolutionError::ReadCache {
                    path: final_path.clone(),
                    source,
                })?;
            if existing != bytes {
                return Err(ArtworkResolutionError::CacheCollision(final_path));
            }
        }
        Err(source) => {
            return Err(ArtworkResolutionError::WriteCache {
                path: final_path,
                source,
            });
        }
    }
    let byte_count =
        i64::try_from(bytes.len()).map_err(|_| ArtworkResolutionError::ImageTooLarge)?;
    Ok(ArtworkBlob {
        sha256,
        relative_path,
        mime_type: mime_type.into(),
        byte_count,
    })
}

fn image_type(bytes: &[u8]) -> Option<(&'static str, &'static str)> {
    if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        Some(("image/jpeg", "jpg"))
    } else if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some(("image/png", "png"))
    } else if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some(("image/webp", "webp"))
    } else {
        None
    }
}

fn select_image(mut images: Vec<IndexImage>) -> Option<IndexImage> {
    images.sort_by_key(|image| (!image.front, !image.approved, image.id.clone()));
    images.into_iter().next()
}

fn is_loopback_url(url: &str) -> bool {
    url.starts_with("http://127.0.0.1:") || url.starts_with("http://[::1]:")
}

fn secure_image_url(url: &str) -> String {
    url.strip_prefix("http://coverartarchive.org/").map_or_else(
        || url.to_owned(),
        |path| format!("https://coverartarchive.org/{path}"),
    )
}

fn is_mbid(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(index, byte)| match index {
            8 | 13 | 18 | 23 => byte == b'-',
            _ => byte.is_ascii_hexdigit(),
        })
}

#[derive(Debug, Deserialize)]
struct IndexResponse {
    #[serde(default)]
    images: Vec<IndexImage>,
}

#[derive(Debug, Deserialize)]
struct IndexImage {
    #[serde(deserialize_with = "deserialize_image_id")]
    id: String,
    image: String,
    #[serde(default)]
    front: bool,
    #[serde(default)]
    back: bool,
    #[serde(default)]
    approved: bool,
    #[serde(default)]
    thumbnails: Thumbnails,
}

impl IndexImage {
    fn preferred_url(&self) -> &str {
        self.thumbnails
            .size_1200
            .as_deref()
            .or(self.thumbnails.size_500.as_deref())
            .or(self.thumbnails.large.as_deref())
            .unwrap_or(&self.image)
    }
}

fn deserialize_image_id<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    match value {
        serde_json::Value::String(id) if !id.is_empty() => Ok(id),
        serde_json::Value::Number(id) => Ok(id.to_string()),
        _ => Err(serde::de::Error::custom(
            "image id must be a string or integer",
        )),
    }
}

#[derive(Debug, Default, Deserialize)]
struct Thumbnails {
    large: Option<String>,
    #[serde(rename = "1200")]
    size_1200: Option<String>,
    #[serde(rename = "500")]
    size_500: Option<String>,
}

/// Aggregate effects of one bounded artwork pass.
#[derive(Debug, Default, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ArtworkResolutionReport {
    /// Candidate releases selected in stable order.
    pub selected: u64,
    /// Newly selected cached release images.
    pub resolved: u64,
    /// Releases with no usable provider image.
    pub unavailable: u64,
    /// Releases deferred after ordinary failures.
    pub deferred: u64,
    /// Per-release provider or cache failures.
    pub failures: Vec<ArtworkResolutionFailure>,
}

/// One non-fatal release-artwork failure.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ArtworkResolutionFailure {
    /// Durable release row ID.
    pub release_id: i64,
    /// Auditable failure message.
    pub message: String,
}

/// Cover Art Archive boundary failure.
#[derive(Debug, Error)]
pub enum ArtworkProviderError {
    /// A meaningful operator-identifying User-Agent is required.
    #[error("Cover Art Archive User-Agent must not be empty")]
    MissingUserAgent,
    /// Provider endpoint is empty or invalid.
    #[error("invalid Cover Art Archive endpoint")]
    InvalidEndpoint,
    /// Release identity is not an MBID.
    #[error("invalid MusicBrainz release MBID")]
    InvalidReleaseMbid,
    /// Provider returned an insecure non-loopback download URL.
    #[error("Cover Art Archive returned an insecure image URL")]
    InsecureImageUrl,
    /// HTTP request or bounded response read failed.
    #[error("Cover Art Archive HTTP failure: {0}")]
    Http(ureq::Error),
    /// Provider response was not UTF-8 JSON text.
    #[error("Cover Art Archive response is not UTF-8: {0}")]
    Utf8(std::str::Utf8Error),
    /// Provider JSON was malformed.
    #[error("invalid Cover Art Archive JSON: {0}")]
    Json(serde_json::Error),
}

/// Artwork workflow or immutable-cache failure.
#[derive(Debug, Error)]
pub enum ArtworkResolutionError {
    /// Work limits must be positive.
    #[error("maximum releases must be greater than zero")]
    InvalidLimit,
    /// SQLite persistence failed.
    #[error(transparent)]
    Database(#[from] DatabaseError),
    /// Downloaded bytes are not an accepted image type.
    #[error("downloaded artwork has invalid JPEG, PNG, or WebP magic bytes")]
    InvalidImage,
    /// Image size cannot be represented durably.
    #[error("downloaded artwork is too large")]
    ImageTooLarge,
    /// Cache target has no valid parent.
    #[error("invalid artwork cache path: {0}")]
    InvalidCachePath(PathBuf),
    /// Cache directory creation failed.
    #[error("failed to create artwork cache directory {path}: {source}")]
    CreateDirectory {
        /// Directory that could not be created.
        path: PathBuf,
        /// Underlying filesystem failure.
        source: std::io::Error,
    },
    /// Immutable cache write failed.
    #[error("failed to write artwork cache file {path}: {source}")]
    WriteCache {
        /// Immutable target path.
        path: PathBuf,
        /// Underlying filesystem failure.
        source: std::io::Error,
    },
    /// Existing cache verification failed.
    #[error("failed to read artwork cache file {path}: {source}")]
    ReadCache {
        /// Existing immutable target path.
        path: PathBuf,
        /// Underlying filesystem failure.
        source: std::io::Error,
    },
    /// Existing bytes contradict their content-addressed path.
    #[error("artwork cache collision at {0}")]
    CacheCollision(PathBuf),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selects_front_then_approved_then_stable_id() -> Result<(), Box<dyn std::error::Error>> {
        let json = br#"{"images":[
          {"id":3,"image":"https://e/3","front":false,"approved":true},
          {"id":2,"image":"https://e/2","front":true,"approved":false},
          {"id":1,"image":"https://e/1","front":true,"approved":true}
        ]}"#;
        let response: IndexResponse = serde_json::from_slice(json)?;
        assert_eq!(
            select_image(response.images).ok_or("missing image")?.id,
            "1"
        );
        Ok(())
    }

    #[test]
    fn recognizes_only_supported_magic_bytes() {
        assert_eq!(image_type(b"\xff\xd8\xffmore"), Some(("image/jpeg", "jpg")));
        assert_eq!(
            image_type(b"\x89PNG\r\n\x1a\nmore"),
            Some(("image/png", "png"))
        );
        assert_eq!(
            image_type(b"RIFF0000WEBPmore"),
            Some(("image/webp", "webp"))
        );
        assert_eq!(image_type(b"<html>"), None);
    }
}
