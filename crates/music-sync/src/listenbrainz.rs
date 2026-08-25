//! Bounded read-only ListenBrainz collaborative-filtering recommendations.

use std::time::Duration;

use serde::Deserialize;
use thiserror::Error;

use crate::discovery::{
    DiscoveryLane, Recommendation, RecommendationError, RecommendationProvider,
};

const MAX_RESPONSE_BYTES: u64 = 4 * 1024 * 1024;

/// Read-only ListenBrainz recording-recommendation adapter.
pub struct ListenBrainz {
    agent: ureq::Agent,
    base_url: String,
    user_name: String,
    exploration_millionths: u32,
    wildcard_millionths: u32,
}

impl ListenBrainz {
    /// Creates a production adapter for the official public API.
    pub fn new(
        user_name: &str,
        user_agent: &str,
        timeout: Duration,
        exploration_ratio: f64,
        wildcard_ratio: f64,
    ) -> Result<Self, ListenBrainzError> {
        Self::with_endpoint(
            "https://api.listenbrainz.org",
            user_name,
            user_agent,
            timeout,
            exploration_ratio,
            wildcard_ratio,
            true,
        )
    }
    /// Creates an explicit endpoint for controlled fixtures or mirrors.
    pub fn with_endpoint(
        base_url: &str,
        user_name: &str,
        user_agent: &str,
        timeout: Duration,
        exploration_ratio: f64,
        wildcard_ratio: f64,
        https_only: bool,
    ) -> Result<Self, ListenBrainzError> {
        if user_name.trim().is_empty() {
            return Err(ListenBrainzError::MissingUser);
        }
        if user_agent.trim().is_empty() {
            return Err(ListenBrainzError::MissingUserAgent);
        }
        if !(0.0..=1.0).contains(&exploration_ratio)
            || !(0.0..=1.0).contains(&wildcard_ratio)
            || exploration_ratio + wildcard_ratio > 1.0
        {
            return Err(ListenBrainzError::InvalidRatios);
        }
        let base_url = base_url.trim_end_matches('/');
        if base_url.is_empty() {
            return Err(ListenBrainzError::InvalidEndpoint);
        }
        let config = ureq::Agent::config_builder()
            .timeout_global(Some(timeout))
            .https_only(https_only)
            .user_agent(user_agent)
            .build();
        Ok(Self {
            agent: config.into(),
            base_url: base_url.into(),
            user_name: user_name.into(),
            exploration_millionths: (exploration_ratio * 1_000_000.0).round() as u32,
            wildcard_millionths: (wildcard_ratio * 1_000_000.0).round() as u32,
        })
    }
    fn fetch(&self, maximum: usize) -> Result<Vec<Recommendation>, ListenBrainzError> {
        let url = format!(
            "{}/1/cf/recommendation/user/{}/recording",
            self.base_url,
            percent_encode(&self.user_name)
        );
        let mut response = self
            .agent
            .get(url)
            .query("count", maximum.to_string())
            .query("offset", "0")
            .call()
            .map_err(ListenBrainzError::Http)?;
        if response.status().as_u16() == 204 {
            return Ok(Vec::new());
        }
        let bytes = response
            .body_mut()
            .with_config()
            .limit(MAX_RESPONSE_BYTES)
            .read_to_vec()
            .map_err(ListenBrainzError::Http)?;
        let response: Response = serde_json::from_slice(&bytes).map_err(ListenBrainzError::Json)?;
        let mut values = response.payload.mbids;
        if values.len() > maximum {
            values.truncate(maximum);
        }
        let maximum_score = values
            .iter()
            .map(|value| value.score)
            .filter(|value| value.is_finite() && *value > 0.0)
            .fold(0.0_f64, f64::max);
        if maximum_score <= 0.0 {
            return Ok(Vec::new());
        }
        let count = values.len().max(1) as u64;
        let adjacent_end = 1_000_000_u64
            .saturating_sub(u64::from(self.exploration_millionths))
            .saturating_sub(u64::from(self.wildcard_millionths));
        let exploration_end = 1_000_000_u64.saturating_sub(u64::from(self.wildcard_millionths));
        Ok(values
            .into_iter()
            .enumerate()
            .filter(|(_, value)| value.score.is_finite() && value.score > 0.0)
            .map(|(index, value)| {
                let rank = (index as u64) * 1_000_000 / count;
                let lane = if rank < adjacent_end {
                    DiscoveryLane::Adjacent
                } else if rank < exploration_end {
                    DiscoveryLane::Exploration
                } else {
                    DiscoveryLane::Wildcard
                };
                Recommendation {
                    recording_mbid: value.recording_mbid,
                    artist_mbid: None,
                    lane,
                    provider_score_millionths: ((value.score / maximum_score) * 1_000_000.0)
                        .round()
                        .clamp(0.0, 1_000_000.0)
                        as u32,
                    seed_weight_millionths: 500_000,
                    reasons: vec![format!(
                        "ListenBrainz collaborative-filtering rank {}",
                        index + 1
                    )],
                }
            })
            .collect())
    }
}
impl RecommendationProvider for ListenBrainz {
    fn recommend(&self, maximum: usize) -> Result<Vec<Recommendation>, RecommendationError> {
        self.fetch(maximum)
            .map_err(|error| RecommendationError::Failure(error.to_string()))
    }
    fn name(&self) -> &'static str {
        "listenbrainz"
    }
}

fn percent_encode(value: &str) -> String {
    value
        .bytes()
        .flat_map(|byte| {
            if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
                vec![byte as char]
            } else {
                format!("%{byte:02X}").chars().collect()
            }
        })
        .collect()
}
#[derive(Debug, Deserialize)]
struct Response {
    payload: Payload,
}
#[derive(Debug, Deserialize)]
struct Payload {
    #[serde(default)]
    mbids: Vec<Recording>,
}
#[derive(Debug, Deserialize)]
struct Recording {
    recording_mbid: String,
    score: f64,
}

/// ListenBrainz request or response failure.
#[derive(Debug, Error)]
pub enum ListenBrainzError {
    /// User name is required.
    #[error("ListenBrainz user name must not be empty")]
    MissingUser,
    /// Identifying User-Agent is required.
    #[error("ListenBrainz User-Agent must not be empty")]
    MissingUserAgent,
    /// Lane ratios are invalid.
    #[error("ListenBrainz discovery ratios are invalid")]
    InvalidRatios,
    /// Endpoint is empty.
    #[error("invalid ListenBrainz endpoint")]
    InvalidEndpoint,
    /// HTTP request/read failed.
    #[error("ListenBrainz HTTP failure: {0}")]
    Http(ureq::Error),
    /// JSON response was malformed.
    #[error("invalid ListenBrainz JSON: {0}")]
    Json(serde_json::Error),
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn percent_encoding_is_path_safe() {
        assert_eq!(percent_encode("a b/c"), "a%20b%2Fc");
    }
}
