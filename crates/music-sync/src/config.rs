//! Application configuration and validation.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Non-secret application configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppConfig {
    /// Directory containing the SQLite database and durable application state.
    pub state_directory: PathBuf,
    /// Root directory containing managed audio.
    pub library_directory: PathBuf,
    /// Directory where Navidrome-compatible playlists will be materialized.
    pub playlist_directory: PathBuf,
    /// Bounded concurrency settings for external work.
    #[serde(default)]
    pub concurrency: ConcurrencyConfig,
    /// Autonomous discovery policy. Discovery is not implemented yet.
    #[serde(default)]
    pub discovery: DiscoveryConfig,
}

/// Bounded concurrency limits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ConcurrencyConfig {
    /// Maximum concurrent provider requests.
    pub provider_requests: usize,
    /// Maximum concurrent acquisitions.
    pub downloads: usize,
    /// Maximum concurrent metadata requests.
    pub metadata_requests: usize,
}

impl Default for ConcurrencyConfig {
    fn default() -> Self {
        Self {
            provider_requests: 2,
            downloads: 1,
            metadata_requests: 4,
        }
    }
}

/// Configurable limits for future autonomous discovery.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DiscoveryConfig {
    /// Whether autonomous discovery is enabled.
    pub enabled: bool,
    /// Desired daily growth rate.
    pub target_new_tracks_per_day: u32,
    /// Hard daily growth limit.
    pub max_new_tracks_per_day: u32,
    /// Daily per-artist diversity limit.
    pub max_tracks_per_artist_per_day: u32,
    /// Fraction reserved for adjacent recommendations.
    pub exploration_ratio: f64,
    /// Fraction reserved for distant recommendations.
    pub wildcard_ratio: f64,
    /// Storage guardrail for future acquisitions.
    pub minimum_free_disk_gb: u64,
}

impl Default for DiscoveryConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            target_new_tracks_per_day: 8,
            max_new_tracks_per_day: 15,
            max_tracks_per_artist_per_day: 2,
            exploration_ratio: 0.20,
            wildcard_ratio: 0.05,
            minimum_free_disk_gb: 20,
        }
    }
}

impl AppConfig {
    /// Loads and validates TOML configuration from `path`.
    pub fn from_file(path: &Path) -> Result<Self, ConfigError> {
        let input = fs::read_to_string(path).map_err(|source| ConfigError::Read {
            path: path.to_path_buf(),
            source,
        })?;
        let config = toml::from_str::<Self>(&input).map_err(|source| ConfigError::Parse {
            path: path.to_path_buf(),
            source,
        })?;
        config.validate()?;
        Ok(config)
    }

    /// Validates cross-field constraints without touching the filesystem.
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.state_directory.as_os_str().is_empty()
            || self.library_directory.as_os_str().is_empty()
            || self.playlist_directory.as_os_str().is_empty()
        {
            return Err(ConfigError::Invalid(
                "configured directories must not be empty".into(),
            ));
        }
        if self.concurrency.provider_requests == 0
            || self.concurrency.downloads == 0
            || self.concurrency.metadata_requests == 0
        {
            return Err(ConfigError::Invalid(
                "concurrency limits must be greater than zero".into(),
            ));
        }
        if self.discovery.target_new_tracks_per_day > self.discovery.max_new_tracks_per_day {
            return Err(ConfigError::Invalid(
                "discovery target cannot exceed the daily maximum".into(),
            ));
        }
        let exploration = self.discovery.exploration_ratio;
        let wildcard = self.discovery.wildcard_ratio;
        if !(0.0..=1.0).contains(&exploration)
            || !(0.0..=1.0).contains(&wildcard)
            || exploration + wildcard > 1.0
        {
            return Err(ConfigError::Invalid(
                "discovery ratios must be between zero and one and sum to at most one".into(),
            ));
        }
        Ok(())
    }

    /// Returns the default SQLite database path within the state directory.
    #[must_use]
    pub fn database_path(&self) -> PathBuf {
        self.state_directory.join("music-sync.sqlite3")
    }
}

/// Configuration loading or validation failure.
#[derive(Debug, Error)]
pub enum ConfigError {
    /// The configuration file could not be read.
    #[error("failed to read configuration {path}: {source}")]
    Read {
        /// Requested configuration path.
        path: PathBuf,
        /// Underlying I/O error.
        source: std::io::Error,
    },
    /// TOML syntax or shape was invalid.
    #[error("failed to parse configuration {path}: {source}")]
    Parse {
        /// Requested configuration path.
        path: PathBuf,
        /// Underlying TOML error.
        source: toml::de::Error,
    },
    /// Values conflict with application constraints.
    #[error("invalid configuration: {0}")]
    Invalid(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_config() -> AppConfig {
        AppConfig {
            state_directory: PathBuf::from("state"),
            library_directory: PathBuf::from("music"),
            playlist_directory: PathBuf::from("playlists"),
            concurrency: ConcurrencyConfig::default(),
            discovery: DiscoveryConfig::default(),
        }
    }

    #[test]
    fn rejects_zero_concurrency() {
        let mut config = valid_config();
        config.concurrency.downloads = 0;
        assert!(matches!(config.validate(), Err(ConfigError::Invalid(_))));
    }

    #[test]
    fn rejects_discovery_ratios_over_one() {
        let mut config = valid_config();
        config.discovery.exploration_ratio = 0.8;
        config.discovery.wildcard_ratio = 0.3;
        assert!(matches!(config.validate(), Err(ConfigError::Invalid(_))));
    }
}
