//! Explainable canonical discovery with deterministic hard safety budgets.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Serialize;
use thiserror::Error;

use crate::config::DiscoveryConfig;
use crate::persistence::{Database, DatabaseError, DiscoveryPersistenceReport};

/// Recommendation exploration lane with an independent budget.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiscoveryLane {
    /// Close to established taste.
    Adjacent,
    /// Broader but related exploration.
    Exploration,
    /// Deliberately distant wildcard.
    Wildcard,
}

impl DiscoveryLane {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Adjacent => "adjacent",
            Self::Exploration => "exploration",
            Self::Wildcard => "wildcard",
        }
    }
}

/// Provider-neutral canonical recommendation evidence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recommendation {
    /// Exact MusicBrainz recording MBID.
    pub recording_mbid: String,
    /// MusicBrainz artist MBID for diversity accounting.
    pub artist_mbid: Option<String>,
    /// Explicit exploration lane.
    pub lane: DiscoveryLane,
    /// Provider relevance in fixed-point millionths.
    pub provider_score_millionths: u32,
    /// Strength of the seed/signal that produced this candidate.
    pub seed_weight_millionths: u32,
    /// Human-auditable provider reasons.
    pub reasons: Vec<String>,
}

/// Boundary for canonical recommendation providers.
pub trait RecommendationProvider {
    /// Returns a bounded set of canonical candidates without acquisition side effects.
    fn recommend(&self, maximum: usize) -> Result<Vec<Recommendation>, RecommendationError>;
    /// Stable provider name retained in provenance.
    fn name(&self) -> &'static str;
}

/// Read-only free-storage boundary used before approving growth.
pub trait FreeSpaceProbe {
    /// Returns available bytes for the configured library filesystem.
    fn available_bytes(&self, path: &Path) -> Result<u64, FreeSpaceError>;
}

/// Portable `df -Pk` free-space adapter.
#[derive(Debug, Clone)]
pub struct DfFreeSpace {
    executable: PathBuf,
}
impl DfFreeSpace {
    /// Creates an adapter using an explicit `df` executable.
    #[must_use]
    pub fn new(executable: PathBuf) -> Self {
        Self { executable }
    }
}
impl FreeSpaceProbe for DfFreeSpace {
    fn available_bytes(&self, path: &Path) -> Result<u64, FreeSpaceError> {
        let output = Command::new(&self.executable)
            .args(["-P", "-k", "--"])
            .arg(path)
            .output()
            .map_err(|source| FreeSpaceError::Spawn {
                executable: self.executable.clone(),
                source,
            })?;
        if !output.status.success() {
            return Err(FreeSpaceError::Failed(
                String::from_utf8_lossy(&output.stderr).trim().into(),
            ));
        }
        if output.stdout.len() > 64 * 1024 {
            return Err(FreeSpaceError::OutputTooLarge);
        }
        let line = String::from_utf8(output.stdout)
            .map_err(FreeSpaceError::Utf8)?
            .lines()
            .last()
            .ok_or(FreeSpaceError::Malformed)?
            .to_owned();
        let available = line
            .split_whitespace()
            .nth(3)
            .ok_or(FreeSpaceError::Malformed)?
            .parse::<u64>()
            .map_err(|_| FreeSpaceError::Malformed)?;
        available.checked_mul(1024).ok_or(FreeSpaceError::Overflow)
    }
}

/// Generates, scores, deduplicates, and budget-gates one discovery pass.
pub fn run_discovery(
    database: &mut Database,
    provider: &dyn RecommendationProvider,
    space: &dyn FreeSpaceProbe,
    library: &Path,
    config: &DiscoveryConfig,
    maximum_candidates: usize,
) -> Result<DiscoveryReport, DiscoveryError> {
    if !config.enabled {
        return Ok(DiscoveryReport {
            disabled: true,
            ..Default::default()
        });
    }
    if maximum_candidates == 0 {
        return Err(DiscoveryError::InvalidLimit);
    }
    let free_bytes = space.available_bytes(library)?;
    let required_free_bytes = config
        .minimum_free_disk_gb
        .checked_mul(1024 * 1024 * 1024)
        .ok_or(DiscoveryError::StorageOverflow)?;
    let mut recommendations = provider.recommend(maximum_candidates)?;
    for recommendation in &recommendations {
        validate_recommendation(recommendation)?;
    }
    recommendations.sort_by(|left, right| {
        score(right)
            .cmp(&score(left))
            .then_with(|| left.recording_mbid.cmp(&right.recording_mbid))
    });
    recommendations.dedup_by(|left, right| left.recording_mbid == right.recording_mbid);
    let persistence = database.record_discovery_candidates(
        provider.name(),
        &recommendations,
        config,
        free_bytes,
        required_free_bytes,
    )?;
    Ok(DiscoveryReport {
        disabled: false,
        received: recommendations.len() as u64,
        persistence,
    })
}

pub(crate) fn score(value: &Recommendation) -> u32 {
    let provider = u64::from(value.provider_score_millionths);
    let seed = u64::from(value.seed_weight_millionths);
    u32::try_from((provider * 700_000 + seed * 300_000) / 1_000_000)
        .unwrap_or(1_000_000)
        .min(1_000_000)
}
fn validate_recommendation(value: &Recommendation) -> Result<(), DiscoveryError> {
    if !is_mbid(&value.recording_mbid)
        || value.artist_mbid.as_ref().is_some_and(|id| !is_mbid(id))
        || value.provider_score_millionths > 1_000_000
        || value.seed_weight_millionths > 1_000_000
    {
        return Err(DiscoveryError::InvalidRecommendation(
            value.recording_mbid.clone(),
        ));
    }
    Ok(())
}
fn is_mbid(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(i, b)| match i {
            8 | 13 | 18 | 23 => b == b'-',
            _ => b.is_ascii_hexdigit(),
        })
}

/// Aggregate durable outcome of one discovery pass.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize)]
pub struct DiscoveryReport {
    /// Whether the configured hard off-switch skipped all boundaries.
    pub disabled: bool,
    /// Unique valid recommendations received.
    pub received: u64,
    /// Durable decisions and budget effects.
    pub persistence: DiscoveryPersistenceReport,
}

/// Recommendation-provider failure.
#[derive(Debug, Error)]
pub enum RecommendationError {
    /// Provider request or parsing failure.
    #[error("recommendation provider failure: {0}")]
    Failure(String),
}
/// Free-space inspection failure.
#[derive(Debug, Error)]
pub enum FreeSpaceError {
    /// `df` could not start.
    #[error("failed to start {executable}: {source}")]
    Spawn {
        /// Executable.
        executable: PathBuf,
        /// Underlying error.
        source: std::io::Error,
    },
    /// `df` returned failure.
    #[error("df failed: {0}")]
    Failed(String),
    /// Output exceeded its bound.
    #[error("df output exceeded its byte limit")]
    OutputTooLarge,
    /// Output was not UTF-8.
    #[error("df output was not UTF-8: {0}")]
    Utf8(std::string::FromUtf8Error),
    /// Output shape was invalid.
    #[error("df output was malformed")]
    Malformed,
    /// KiB-to-byte conversion overflowed.
    #[error("df byte count overflowed")]
    Overflow,
}
/// Discovery orchestration failure.
#[derive(Debug, Error)]
pub enum DiscoveryError {
    /// Candidate bound must be positive.
    #[error("maximum discovery candidates must be greater than zero")]
    InvalidLimit,
    /// Recommendation violated canonical constraints.
    #[error("invalid canonical recommendation: {0}")]
    InvalidRecommendation(String),
    /// Configured storage conversion overflowed.
    #[error("configured minimum free storage overflowed bytes")]
    StorageOverflow,
    /// Recommendation provider failed.
    #[error(transparent)]
    Provider(#[from] RecommendationError),
    /// Free-space inspection failed.
    #[error(transparent)]
    FreeSpace(#[from] FreeSpaceError),
    /// Durable state failed.
    #[error(transparent)]
    Database(#[from] DatabaseError),
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FixtureProvider(Vec<Recommendation>);
    impl RecommendationProvider for FixtureProvider {
        fn recommend(&self, maximum: usize) -> Result<Vec<Recommendation>, RecommendationError> {
            Ok(self.0.iter().take(maximum).cloned().collect())
        }
        fn name(&self) -> &'static str {
            "fixture"
        }
    }
    struct FixtureSpace(u64);
    impl FreeSpaceProbe for FixtureSpace {
        fn available_bytes(&self, _: &Path) -> Result<u64, FreeSpaceError> {
            Ok(self.0)
        }
    }
    fn recommendation(
        recording: &str,
        artist: Option<&str>,
        lane: DiscoveryLane,
        provider_score_millionths: u32,
    ) -> Recommendation {
        Recommendation {
            recording_mbid: recording.into(),
            artist_mbid: artist.map(Into::into),
            lane,
            provider_score_millionths,
            seed_weight_millionths: 500_000,
            reasons: vec!["fixture signal".into()],
        }
    }
    fn config() -> DiscoveryConfig {
        DiscoveryConfig {
            enabled: true,
            target_new_tracks_per_day: 2,
            max_new_tracks_per_day: 3,
            max_tracks_per_artist_per_day: 1,
            exploration_ratio: 0.34,
            wildcard_ratio: 0.33,
            minimum_free_disk_gb: 1,
            listenbrainz_user: Some("fixture-user".into()),
            navidrome_url: None,
            navidrome_user: None,
            ..DiscoveryConfig::default()
        }
    }

    #[test]
    fn scoring_is_fixed_point_and_deterministic() {
        let value = Recommendation {
            recording_mbid: "11111111-2222-3333-4444-555555555555".into(),
            artist_mbid: None,
            lane: DiscoveryLane::Adjacent,
            provider_score_millionths: 800_000,
            seed_weight_millionths: 400_000,
            reasons: vec![],
        };
        assert_eq!(score(&value), 680_000);
    }

    #[test]
    fn hard_budgets_and_exact_dedup_are_transactional() -> Result<(), Box<dyn std::error::Error>> {
        let mut database = Database::open_in_memory()?;
        let provider = FixtureProvider(vec![
            recommendation(
                "11111111-1111-1111-1111-111111111111",
                Some("aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa"),
                DiscoveryLane::Adjacent,
                900_000,
            ),
            recommendation(
                "22222222-2222-2222-2222-222222222222",
                Some("aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa"),
                DiscoveryLane::Adjacent,
                800_000,
            ),
            recommendation(
                "33333333-3333-3333-3333-333333333333",
                Some("bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb"),
                DiscoveryLane::Exploration,
                700_000,
            ),
            recommendation(
                "44444444-4444-4444-4444-444444444444",
                None,
                DiscoveryLane::Wildcard,
                600_000,
            ),
        ]);
        let first = run_discovery(
            &mut database,
            &provider,
            &FixtureSpace(2 * 1024 * 1024 * 1024),
            Path::new("/library"),
            &config(),
            10,
        )?;
        assert_eq!(first.persistence.approved, 2);
        assert_eq!(first.persistence.budget_rejected, 2);
        let repeat = run_discovery(
            &mut database,
            &provider,
            &FixtureSpace(2 * 1024 * 1024 * 1024),
            Path::new("/library"),
            &config(),
            10,
        )?;
        assert_eq!(repeat.persistence.duplicates, 2);
        assert_eq!(repeat.persistence.approved, 0);
        Ok(())
    }
}
