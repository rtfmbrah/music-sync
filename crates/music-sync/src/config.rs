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
    /// Autonomous discovery policy.
    #[serde(default)]
    pub discovery: DiscoveryConfig,
    /// Complete headless service-cycle policy and external adapter locations.
    #[serde(default)]
    pub service: ServiceConfig,
}

/// Headless autonomous service configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ServiceConfig {
    /// Enables the complete enrichment/discovery service cycle.
    pub enabled: bool,
    /// Identifying HTTP User-Agent with operator contact information.
    pub user_agent: Option<String>,
    /// Explicit yt-dlp adapter executable.
    pub yt_dlp: PathBuf,
    /// Optional Netscape-format YouTube cookie file kept outside tracked config.
    pub yt_dlp_cookie_file: Option<PathBuf>,
    /// Delay between provider extraction requests delegated to yt-dlp.
    pub yt_dlp_sleep_requests_seconds: u64,
    /// Minimum randomized delay before each media download.
    pub yt_dlp_min_sleep_seconds: u64,
    /// Maximum randomized delay before each media download.
    pub yt_dlp_max_sleep_seconds: u64,
    /// Explicit ffprobe executable.
    pub ffprobe: PathBuf,
    /// Explicit ffmpeg executable.
    pub ffmpeg: PathBuf,
    /// Explicit fpcalc executable.
    pub fpcalc: PathBuf,
    /// MusicBrainz ws/2 endpoint.
    pub musicbrainz_endpoint: String,
    /// Cover Art Archive endpoint.
    pub cover_art_endpoint: String,
    /// LRCLIB API endpoint.
    pub lyrics_endpoint: String,
    /// ListenBrainz API endpoint.
    pub listenbrainz_endpoint: String,
    /// AcoustID fingerprint lookup endpoint.
    pub acoustid_endpoint: String,
    /// Per-boundary global HTTP deadline in seconds.
    pub http_timeout_seconds: u64,
    /// Per-provider yt-dlp enumeration deadline in seconds.
    pub source_timeout_seconds: u64,
    /// Per-download yt-dlp deadline in seconds.
    pub download_timeout_seconds: u64,
    /// Per-file ffprobe deadline in seconds.
    pub probe_timeout_seconds: u64,
    /// Per-file ffmpeg deadline in seconds.
    pub remux_timeout_seconds: u64,
    /// Per-file fpcalc deadline in seconds.
    pub fingerprint_timeout_seconds: u64,
    /// Maximum items selected by each ordinary bounded phase.
    pub phase_item_limit: usize,
    /// Maximum audio seconds consumed by each fingerprint.
    pub fingerprint_audio_seconds: u32,
}

impl Default for ServiceConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            user_agent: None,
            yt_dlp: "yt-dlp".into(),
            yt_dlp_cookie_file: None,
            yt_dlp_sleep_requests_seconds: 1,
            yt_dlp_min_sleep_seconds: 5,
            yt_dlp_max_sleep_seconds: 15,
            ffprobe: "ffprobe".into(),
            ffmpeg: "ffmpeg".into(),
            fpcalc: "fpcalc".into(),
            musicbrainz_endpoint: "https://musicbrainz.org/ws/2".into(),
            cover_art_endpoint: "https://coverartarchive.org".into(),
            lyrics_endpoint: "https://lrclib.net/api".into(),
            listenbrainz_endpoint: "https://api.listenbrainz.org".into(),
            acoustid_endpoint: "https://api.acoustid.org/v2/lookup".into(),
            http_timeout_seconds: 30,
            source_timeout_seconds: 60,
            download_timeout_seconds: 600,
            probe_timeout_seconds: 30,
            remux_timeout_seconds: 120,
            fingerprint_timeout_seconds: 60,
            phase_item_limit: 100,
            fingerprint_audio_seconds: 120,
        }
    }
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

/// Configurable limits for autonomous discovery.
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
    /// Public ListenBrainz user whose recommendations are read.
    pub listenbrainz_user: Option<String>,
    /// Read-only Navidrome/Subsonic API base URL for favorite seeds.
    pub navidrome_url: Option<String>,
    /// Navidrome user paired with token/salt secrets from the process environment.
    pub navidrome_user: Option<String>,
    /// Searches YouTube when canonical MusicBrainz URL evidence is absent.
    pub youtube_search_fallback: bool,
    /// Maximum yt-dlp results staged for one recommendation.
    pub youtube_search_max_candidates: usize,
    /// Minimum AcoustID confidence represented as a value from zero to one.
    pub acoustid_minimum_score: f64,
    /// Maximum audio seconds supplied to the AcoustID Chromaprint extractor.
    pub acoustid_fingerprint_audio_seconds: u32,
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
            listenbrainz_user: None,
            navidrome_url: None,
            navidrome_user: None,
            youtube_search_fallback: false,
            youtube_search_max_candidates: 3,
            acoustid_minimum_score: 0.95,
            acoustid_fingerprint_audio_seconds: 900,
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
        if self.service.enabled {
            if self
                .service
                .user_agent
                .as_deref()
                .is_none_or(|value| value.trim().is_empty())
            {
                return Err(ConfigError::Invalid(
                    "enabled service requires an identifying user agent".into(),
                ));
            }
            if self.service.phase_item_limit == 0
                || self.service.fingerprint_audio_seconds == 0
                || [
                    self.service.http_timeout_seconds,
                    self.service.source_timeout_seconds,
                    self.service.download_timeout_seconds,
                    self.service.probe_timeout_seconds,
                    self.service.remux_timeout_seconds,
                    self.service.fingerprint_timeout_seconds,
                ]
                .contains(&0)
            {
                return Err(ConfigError::Invalid(
                    "enabled service limits and deadlines must be greater than zero".into(),
                ));
            }
            if [
                &self.service.yt_dlp,
                &self.service.ffprobe,
                &self.service.ffmpeg,
                &self.service.fpcalc,
            ]
            .iter()
            .any(|path| path.as_os_str().is_empty())
            {
                return Err(ConfigError::Invalid(
                    "enabled service tool paths must not be empty".into(),
                ));
            }
            if self
                .service
                .yt_dlp_cookie_file
                .as_ref()
                .is_some_and(|path| path.as_os_str().is_empty())
            {
                return Err(ConfigError::Invalid(
                    "configured yt-dlp cookie path must not be empty".into(),
                ));
            }
            if self.service.yt_dlp_sleep_requests_seconds == 0
                || self.service.yt_dlp_min_sleep_seconds == 0
                || self.service.yt_dlp_max_sleep_seconds < self.service.yt_dlp_min_sleep_seconds
            {
                return Err(ConfigError::Invalid(
                    "enabled service yt-dlp pacing must be positive with maximum sleep at least minimum sleep".into(),
                ));
            }
        }
        if self.discovery.target_new_tracks_per_day > self.discovery.max_new_tracks_per_day {
            return Err(ConfigError::Invalid(
                "discovery target cannot exceed the daily maximum".into(),
            ));
        }
        if self.discovery.enabled {
            if self.discovery.max_new_tracks_per_day == 0
                || self.discovery.max_tracks_per_artist_per_day == 0
            {
                return Err(ConfigError::Invalid(
                    "enabled discovery budgets must be greater than zero".into(),
                ));
            }
            if self
                .discovery
                .listenbrainz_user
                .as_deref()
                .is_none_or(|user| user.trim().is_empty())
            {
                return Err(ConfigError::Invalid(
                    "enabled discovery requires a ListenBrainz user".into(),
                ));
            }
            if self.discovery.navidrome_url.is_some() != self.discovery.navidrome_user.is_some() {
                return Err(ConfigError::Invalid(
                    "Navidrome discovery URL and user must be configured together".into(),
                ));
            }
            if self.discovery.youtube_search_fallback
                && (self.discovery.youtube_search_max_candidates == 0
                    || self.discovery.acoustid_fingerprint_audio_seconds == 0)
            {
                return Err(ConfigError::Invalid(
                    "enabled discovery YouTube search requires positive candidate and fingerprint limits".into(),
                ));
            }
        }
        if !(self.discovery.acoustid_minimum_score.is_finite()
            && 0.0 < self.discovery.acoustid_minimum_score
            && self.discovery.acoustid_minimum_score <= 1.0)
        {
            return Err(ConfigError::Invalid(
                "AcoustID minimum score must be greater than zero and at most one".into(),
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
            service: ServiceConfig::default(),
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

    #[test]
    fn rejects_invalid_service_provider_pacing() {
        let mut config = valid_config();
        config.service.enabled = true;
        config.service.user_agent = Some("fixture@example.invalid".into());
        config.service.yt_dlp_min_sleep_seconds = 20;
        config.service.yt_dlp_max_sleep_seconds = 10;
        assert!(matches!(config.validate(), Err(ConfigError::Invalid(_))));
    }
}
