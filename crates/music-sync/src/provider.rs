//! Provider-neutral source identities and enumeration boundary.

use serde::Serialize;
use serde_json::Value;

/// Durable identifier of one configured source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct SourceId(pub i64);

/// One configured provider source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ConfiguredSource {
    /// Durable music-sync source ID.
    pub id: SourceId,
    /// Provider adapter name.
    pub provider: String,
    /// Original source URL.
    pub url: String,
    /// Optional user-facing name.
    pub name: Option<String>,
    /// Whether future sync runs should enumerate this source.
    pub active: bool,
}

/// Result of idempotently adding or reactivating a source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct AddSourceResult {
    /// Durable source ID.
    pub id: SourceId,
    /// Whether a new row was inserted.
    pub inserted: bool,
    /// Whether an existing inactive source was reactivated.
    pub reactivated: bool,
}

/// Validates the deliberately narrow first-provider URL policy.
#[must_use]
pub fn is_supported_youtube_url(url: &str) -> bool {
    [
        "https://www.youtube.com/",
        "https://youtube.com/",
        "https://music.youtube.com/",
        "https://youtu.be/",
    ]
    .iter()
    .any(|prefix| url.starts_with(prefix))
}

/// Extracts a conservative provider-owned video ID from a supported YouTube URL.
#[must_use]
pub fn youtube_video_id(url: &str) -> Option<String> {
    if !is_supported_youtube_url(url) {
        return None;
    }
    let candidate = if let Some(value) = url.strip_prefix("https://youtu.be/") {
        value.split(['?', '#', '/']).next()
    } else {
        url.split_once('?').and_then(|(_, query)| {
            query.split('&').find_map(|part| {
                part.strip_prefix("v=")
                    .map(|value| value.split('#').next().unwrap_or_default())
            })
        })
    }?;
    (!candidate.is_empty()
        && candidate.len() <= 64
        && candidate
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_')))
    .then(|| candidate.to_owned())
}

/// One remote provider object; this is not a canonical recording identity.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ProviderItem {
    /// Provider-owned stable item identifier.
    pub provider_item_id: String,
    /// Provider URL retained for audit and later acquisition.
    pub url: String,
    /// Provider-supplied title, if present.
    pub title: Option<String>,
    /// Provider-supplied duration in milliseconds, if finite and non-negative.
    pub duration_ms: Option<u64>,
    /// Complete provider response for later audit and adapter evolution.
    pub raw_metadata: Value,
}

/// Snapshot returned by enumerating one configured source.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SourceSnapshot {
    /// Provider implementation name.
    pub provider: String,
    /// Provider collection ID for playlists, absent for a single-item source.
    pub provider_collection_id: Option<String>,
    /// Provider collection title when available.
    pub title: Option<String>,
    /// Items in provider order.
    pub items: Vec<ProviderItem>,
}

/// Conservative provider failure class.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderFailureKind {
    /// The item is explicitly private, deleted, or otherwise permanently unavailable.
    PermanentlyUnavailable,
    /// Authentication, cookies, or tokens are required or invalid.
    Authentication,
    /// The provider has rate-limited the request.
    RateLimited,
    /// The adapter exceeded its deadline.
    Timeout,
    /// Provider output changed or was malformed.
    Extraction,
    /// Unknown/infrastructure failures conservatively treated as transient.
    Transient,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_only_supported_https_youtube_hosts() {
        assert!(is_supported_youtube_url(
            "https://www.youtube.com/watch?v=abc"
        ));
        assert!(is_supported_youtube_url(
            "https://music.youtube.com/playlist?list=abc"
        ));
        assert!(is_supported_youtube_url("https://youtu.be/abc"));
        assert!(!is_supported_youtube_url(
            "http://www.youtube.com/watch?v=abc"
        ));
        assert_eq!(
            youtube_video_id("https://www.youtube.com/watch?v=Abc_12-x&list=test").as_deref(),
            Some("Abc_12-x")
        );
        assert_eq!(
            youtube_video_id("https://youtu.be/Abc_12-x?t=4").as_deref(),
            Some("Abc_12-x")
        );
        assert!(!is_supported_youtube_url("https://example.com/watch?v=abc"));
        assert!(!is_supported_youtube_url(
            "https://youtube.com.example.com/watch?v=abc"
        ));
    }
}
