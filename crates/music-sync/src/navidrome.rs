//! Read-only Navidrome taste signals through the public Subsonic API.

use std::time::Duration;

use serde::Deserialize;
use thiserror::Error;

const MAX_RESPONSE_BYTES: u64 = 4 * 1024 * 1024;

/// Fetches starred recordings without accessing Navidrome's private database.
pub struct NavidromeFavorites {
    agent: ureq::Agent,
    base_url: String,
    user: String,
    token: String,
    salt: String,
}

impl NavidromeFavorites {
    /// Creates a production HTTPS adapter using a precomputed Subsonic token and salt.
    pub fn new(
        base_url: &str,
        user: &str,
        token: &str,
        salt: &str,
        timeout: Duration,
    ) -> Result<Self, NavidromeError> {
        Self::with_endpoint(base_url, user, token, salt, timeout, true)
    }

    /// Creates an explicit endpoint; plaintext is only intended for loopback fixtures.
    pub fn with_endpoint(
        base_url: &str,
        user: &str,
        token: &str,
        salt: &str,
        timeout: Duration,
        https_only: bool,
    ) -> Result<Self, NavidromeError> {
        if user.trim().is_empty() || token.trim().is_empty() || salt.len() < 6 {
            return Err(NavidromeError::MissingCredentials);
        }
        let base_url = base_url.trim_end_matches('/');
        if base_url.is_empty() {
            return Err(NavidromeError::InvalidEndpoint);
        }
        let config = ureq::Agent::config_builder()
            .timeout_global(Some(timeout))
            .https_only(https_only)
            .user_agent("music-sync/0.1")
            .build();
        Ok(Self {
            agent: config.into(),
            base_url: base_url.into(),
            user: user.into(),
            token: token.into(),
            salt: salt.into(),
        })
    }

    /// Returns only valid canonical recording MBIDs from starred songs.
    pub fn recording_mbids(&self) -> Result<Vec<String>, NavidromeError> {
        let url = format!("{}/rest/getStarred2.view", self.base_url);
        let mut response = self
            .agent
            .get(url)
            .query("u", &self.user)
            .query("t", &self.token)
            .query("s", &self.salt)
            .query("v", "1.16.1")
            .query("c", "music-sync")
            .query("f", "json")
            .call()
            .map_err(NavidromeError::Http)?;
        let bytes = response
            .body_mut()
            .with_config()
            .limit(MAX_RESPONSE_BYTES)
            .read_to_vec()
            .map_err(NavidromeError::Http)?;
        let envelope: Envelope = serde_json::from_slice(&bytes).map_err(NavidromeError::Json)?;
        if envelope.response.status != "ok" {
            return Err(NavidromeError::Api(envelope.response.error.map_or_else(
                || "unknown Subsonic error".into(),
                |error| error.message,
            )));
        }
        let mut values = envelope
            .response
            .starred
            .map_or_else(Vec::new, |starred| starred.songs)
            .into_iter()
            .filter_map(|song| song.recording_mbid)
            .filter(|value| is_uuid_shape(value))
            .collect::<Vec<_>>();
        values.sort();
        values.dedup();
        Ok(values)
    }
}

fn is_uuid_shape(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(index, byte)| {
            if matches!(index, 8 | 13 | 18 | 23) {
                byte == b'-'
            } else {
                byte.is_ascii_hexdigit()
            }
        })
}

#[derive(Debug, Deserialize)]
struct Envelope {
    #[serde(rename = "subsonic-response")]
    response: SubsonicResponse,
}

#[derive(Debug, Deserialize)]
struct SubsonicResponse {
    status: String,
    #[serde(rename = "starred2")]
    starred: Option<Starred>,
    error: Option<ApiError>,
}

#[derive(Debug, Deserialize)]
struct Starred {
    #[serde(default, rename = "song")]
    songs: Vec<Song>,
}

#[derive(Debug, Deserialize)]
struct Song {
    #[serde(default, rename = "musicBrainzId", alias = "musicBrainzRecordingId")]
    recording_mbid: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ApiError {
    message: String,
}

/// Navidrome request or response failure.
#[derive(Debug, Error)]
pub enum NavidromeError {
    /// Credentials are absent or structurally unsafe.
    #[error("Navidrome user, token, and a salt of at least six characters are required")]
    MissingCredentials,
    /// Endpoint is empty.
    #[error("invalid Navidrome endpoint")]
    InvalidEndpoint,
    /// HTTP request/read failed.
    #[error("Navidrome HTTP failure: {0}")]
    Http(ureq::Error),
    /// JSON response was malformed.
    #[error("invalid Navidrome JSON: {0}")]
    Json(serde_json::Error),
    /// Subsonic returned a protocol-level error.
    #[error("Navidrome API failure: {0}")]
    Api(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_only_uuid_shaped_recording_ids() {
        assert!(is_uuid_shape("11111111-2222-3333-4444-555555555555"));
        assert!(!is_uuid_shape("not-a-recording"));
    }
}
