//! Complete bounded autonomous service-cycle orchestration.

use std::path::Path;

use serde::Serialize;
use thiserror::Error;

use crate::acoustid::AcoustIdProvider;
use crate::acquisition::{AcquisitionBatchReport, run_pending_acquisitions};
use crate::artwork::{ArtworkResolutionReport, ReleaseArtworkProvider, resolve_release_artwork};
use crate::config::DiscoveryConfig;
use crate::content_hash::ContentHasher;
use crate::discovery::{DiscoveryReport, FreeSpaceProbe, RecommendationProvider, run_discovery};
use crate::discovery_routing::{DiscoveryRoutingReport, route_approved_discovery_with_search};
use crate::discovery_search::DiscoverySearchBoundaries;
use crate::fingerprint::{
    CompressedFingerprinter, FingerprintReport, Fingerprinter, reconcile_artifact_fingerprints,
};
use crate::genre::{GenreResolutionReport, resolve_genres};
use crate::health::{ArtifactHealthReport, reconcile_artifact_health};
use crate::lyrics::{LyricsProvider, LyricsReport, resolve_lyrics};
use crate::media_probe::MediaProbe;
use crate::metadata::{MetadataResolutionReport, resolve_canonical_metadata};
use crate::musicbrainz::{CanonicalMetadataProvider, GenreMetadataProvider};
use crate::navidrome::TasteSignalProvider;
use crate::persistence::{
    Database, DatabaseError, DiscoverySeedReport, RepairVerificationDecision, ServicePhaseStatus,
};
use crate::playlist::{PlaylistMaterializationReport, materialize_playlists};
use crate::provider_enrichment::{ProviderMetadataReport, enrich_provider_metadata};
use crate::repair::{
    RepairAssessmentReport, RepairCandidateReport, RepairCommitReport, RepairRunOutcome,
    assess_repair_eligibility, commit_verified_repair, generate_repair_candidates,
    run_one_repair_verification,
};
use crate::sync::{SyncBoundaries, SyncDirectories, SyncLimits, SyncRunReport, run_sync};
use crate::tag_materialization::{
    MetadataMaterializationReport, MetadataRemuxer, materialize_canonical_tags,
};
use crate::yt_dlp::YtDlp;

/// External boundaries used by one complete service cycle.
pub struct ServiceBoundaries<'a> {
    /// Source enumeration adapter.
    pub source_adapter: &'a YtDlp,
    /// Acquisition and repair provider adapter.
    pub acquisition_adapter: &'a YtDlp,
    /// Structural media probe.
    pub probe: &'a dyn MediaProbe,
    /// Exact-byte hash boundary.
    pub hasher: &'a dyn ContentHasher,
    /// Perceptual fingerprint boundary.
    pub fingerprinter: &'a dyn Fingerprinter,
    /// Compressed fingerprint boundary used only by optional AcoustID discovery.
    pub compressed_fingerprinter: &'a dyn CompressedFingerprinter,
    /// Optional external identity provider for discovery search.
    pub acoustid: Option<&'a dyn AcoustIdProvider>,
    /// Canonical MusicBrainz metadata and relationship boundary.
    pub canonical_metadata: &'a dyn CanonicalMetadataProvider,
    /// Identity-verified external genre boundary.
    pub genres: &'a dyn GenreMetadataProvider,
    /// Canonical release-art boundary.
    pub artwork: &'a dyn ReleaseArtworkProvider,
    /// Lyrics boundary.
    pub lyrics: &'a dyn LyricsProvider,
    /// Source-preserving tag remux boundary.
    pub remuxer: &'a dyn MetadataRemuxer,
    /// Optional autonomous recommendation provider.
    pub recommendations: Option<&'a dyn RecommendationProvider>,
    /// Optional read-only Navidrome taste-signal provider.
    pub taste_signals: Option<&'a dyn TasteSignalProvider>,
    /// Free-storage boundary.
    pub free_space: &'a dyn FreeSpaceProbe,
}

/// Filesystem roots used by one complete service cycle.
#[derive(Debug, Clone, Copy)]
pub struct ServiceDirectories<'a> {
    /// Durable state/staging root.
    pub state: &'a Path,
    /// Managed library root.
    pub library: &'a Path,
    /// Owned playlist output root.
    pub playlists: &'a Path,
}

/// Explicit work bounds for one complete service cycle.
#[derive(Debug, Clone, Copy)]
pub struct ServiceLimits {
    /// General per-phase item bound.
    pub items: usize,
    /// Maximum fingerprint audio seconds.
    pub fingerprint_audio_seconds: u32,
}

/// Runs core synchronization, discovery acquisition, health, repair assessment,
/// enrichment, final tags, and final playlists inside one durable service run.
pub fn run_complete_service(
    database: &mut Database,
    service_run_id: i64,
    directories: ServiceDirectories<'_>,
    boundaries: ServiceBoundaries<'_>,
    limits: ServiceLimits,
    discovery: &DiscoveryConfig,
) -> Result<CompleteServiceReport, ServiceError> {
    if limits.items == 0 || limits.fingerprint_audio_seconds == 0 {
        return Err(ServiceError::InvalidLimits);
    }
    let core = run_sync(
        database,
        service_run_id,
        SyncDirectories {
            state: directories.state,
            library: directories.library,
            playlists: directories.playlists,
        },
        SyncBoundaries {
            source_adapter: boundaries.source_adapter,
            acquisition_adapter: boundaries.acquisition_adapter,
            probe: boundaries.probe,
            hasher: boundaries.hasher,
            fingerprinter: boundaries.fingerprinter,
        },
        SyncLimits {
            maximum_jobs: limits.items,
            maximum_fingerprints: limits.items,
            fingerprint_audio_seconds: limits.fingerprint_audio_seconds,
        },
    )
    .map_err(|error| ServiceError::Phase {
        phase: "core_sync".into(),
        message: error.to_string(),
    })?;
    let mut ordinal = 4;
    let seeds = if discovery.enabled {
        if let Some(provider) = boundaries.taste_signals {
            Some(execute_database_phase(
                database,
                service_run_id,
                "taste_signals",
                ordinal,
                |database| {
                    let mbids = provider
                        .recording_mbids()
                        .map_err(|error| error.to_string())?;
                    database
                        .replace_navidrome_seeds(&mbids)
                        .map_err(|error| error.to_string())
                },
                |_| ServicePhaseStatus::Succeeded,
            )?)
        } else {
            None
        }
    } else {
        None
    };
    if seeds.is_some() {
        ordinal += 1;
    }
    let discovery_report = if discovery.enabled {
        let provider = boundaries
            .recommendations
            .ok_or(ServiceError::MissingRecommendations)?;
        let report = execute_database_phase(
            database,
            service_run_id,
            "discovery",
            ordinal,
            |database| {
                run_discovery(
                    database,
                    provider,
                    boundaries.free_space,
                    directories.library,
                    discovery,
                    limits.items,
                )
            },
            |_| ServicePhaseStatus::Succeeded,
        )?;
        ordinal += 1;
        Some(report)
    } else {
        None
    };
    let routing = if discovery.enabled {
        let acoustid_search = if discovery.youtube_search_fallback {
            let acoustid = boundaries.acoustid.ok_or(ServiceError::MissingAcoustId)?;
            Some(DiscoverySearchBoundaries {
                state_directory: directories.state,
                yt_dlp: boundaries.acquisition_adapter,
                probe: boundaries.probe,
                hasher: boundaries.hasher,
                fingerprinter: boundaries.compressed_fingerprinter,
                acoustid,
                maximum_candidates: discovery.youtube_search_max_candidates,
                minimum_score_millionths: (discovery.acoustid_minimum_score * 1_000_000.0).round()
                    as u32,
            })
        } else {
            None
        };
        let report = execute_database_phase(
            database,
            service_run_id,
            "discovery_routing",
            ordinal,
            |database| {
                route_approved_discovery_with_search(
                    database,
                    boundaries.canonical_metadata,
                    limits.items,
                    acoustid_search.as_ref(),
                )
            },
            |report| {
                if report.failures.is_empty() {
                    ServicePhaseStatus::Succeeded
                } else {
                    ServicePhaseStatus::Partial
                }
            },
        )?;
        ordinal += 1;
        Some(report)
    } else {
        None
    };
    let discovery_acquisitions = execute_database_phase(
        database,
        service_run_id,
        "post_discovery_acquisitions",
        ordinal,
        |database| {
            run_pending_acquisitions(
                database,
                directories.state,
                directories.library,
                boundaries.acquisition_adapter,
                boundaries.probe,
                boundaries.hasher,
                limits.items,
            )
        },
        |report| phase_for_work(report.selected, report.failures.is_empty()),
    )?;
    ordinal += 1;
    let health = execute_database_phase(
        database,
        service_run_id,
        "health",
        ordinal,
        |database| {
            reconcile_artifact_health(
                database,
                directories.library,
                boundaries.probe,
                boundaries.hasher,
                limits.items,
            )
        },
        |report| phase_for_work(report.selected, report.failures.is_empty()),
    )?;
    ordinal += 1;
    let fingerprints = execute_database_phase(
        database,
        service_run_id,
        "post_discovery_fingerprints",
        ordinal,
        |database| {
            reconcile_artifact_fingerprints(
                database,
                directories.library,
                boundaries.fingerprinter,
                limits.items,
                limits.fingerprint_audio_seconds,
            )
        },
        |report| {
            if report.failures.is_empty() {
                ServicePhaseStatus::Succeeded
            } else {
                ServicePhaseStatus::Partial
            }
        },
    )?;
    ordinal += 1;
    let repair_assessment = execute_database_phase(
        database,
        service_run_id,
        "repair_assessment",
        ordinal,
        |database| {
            assess_repair_eligibility(database, boundaries.acquisition_adapter, limits.items)
        },
        |report| {
            if report.failures.is_empty() {
                ServicePhaseStatus::Succeeded
            } else {
                ServicePhaseStatus::Partial
            }
        },
    )?;
    ordinal += 1;
    let repair_execution = execute_database_phase(
        database,
        service_run_id,
        "repair_execution",
        ordinal,
        |database| {
            run_automated_repairs(
                database,
                directories,
                boundaries.acquisition_adapter,
                boundaries.probe,
                boundaries.hasher,
                boundaries.fingerprinter,
                limits.items,
            )
        },
        |report| {
            if report.failures.is_empty() {
                ServicePhaseStatus::Succeeded
            } else {
                ServicePhaseStatus::Partial
            }
        },
    )?;
    ordinal += 1;
    let metadata = execute_database_phase(
        database,
        service_run_id,
        "metadata",
        ordinal,
        |database| {
            resolve_canonical_metadata(database, boundaries.canonical_metadata, limits.items)
        },
        |report| phase_for_work(report.selected, report.failures.is_empty()),
    )?;
    ordinal += 1;
    let provider_metadata = execute_database_phase(
        database,
        service_run_id,
        "provider_metadata",
        ordinal,
        |database| enrich_provider_metadata(database, boundaries.source_adapter, limits.items),
        |report| phase_for_work(report.selected, report.failures.is_empty()),
    )?;
    ordinal += 1;
    let genres = execute_database_phase(
        database,
        service_run_id,
        "genres",
        ordinal,
        |database| resolve_genres(database, boundaries.genres, limits.items),
        |report| phase_for_work(report.selected, report.failures.is_empty()),
    )?;
    ordinal += 1;
    let artwork = execute_database_phase(
        database,
        service_run_id,
        "artwork",
        ordinal,
        |database| {
            resolve_release_artwork(
                database,
                boundaries.artwork,
                directories.state,
                limits.items,
            )
        },
        |report| {
            phase_for_work(
                report.selected,
                report.failures.is_empty() && report.provider_failures.is_empty(),
            )
        },
    )?;
    ordinal += 1;
    let lyrics = execute_database_phase(
        database,
        service_run_id,
        "lyrics",
        ordinal,
        |database| resolve_lyrics(database, boundaries.lyrics, limits.items),
        |report| phase_for_work(report.selected, report.failures.is_empty()),
    )?;
    ordinal += 1;
    let tags = execute_database_phase(
        database,
        service_run_id,
        "tags",
        ordinal,
        |database| {
            materialize_canonical_tags(
                database,
                boundaries.remuxer,
                boundaries.probe,
                boundaries.hasher,
                directories.state,
                limits.items,
            )
        },
        |report| phase_for_work(report.selected, report.failures.is_empty()),
    )?;
    ordinal += 1;
    let playlists = execute_database_phase(
        database,
        service_run_id,
        "final_playlists",
        ordinal,
        |database| materialize_playlists(database, directories.library, directories.playlists),
        |report| {
            if report.failures.is_empty() {
                ServicePhaseStatus::Succeeded
            } else {
                ServicePhaseStatus::Partial
            }
        },
    )?;
    Ok(CompleteServiceReport {
        core,
        seeds,
        discovery: discovery_report,
        routing,
        discovery_acquisitions,
        health,
        fingerprints,
        repair_assessment,
        repair_execution,
        metadata,
        provider_metadata,
        genres,
        artwork,
        lyrics,
        tags,
        playlists,
    })
}

fn phase_for_work(selected: u64, successful: bool) -> ServicePhaseStatus {
    if selected == 0 {
        ServicePhaseStatus::Skipped
    } else if successful {
        ServicePhaseStatus::Succeeded
    } else {
        ServicePhaseStatus::Partial
    }
}

fn run_automated_repairs(
    database: &mut Database,
    directories: ServiceDirectories<'_>,
    adapter: &YtDlp,
    probe: &dyn MediaProbe,
    hasher: &dyn ContentHasher,
    fingerprinter: &dyn Fingerprinter,
    limit: usize,
) -> Result<AutomatedRepairReport, String> {
    let candidates = generate_repair_candidates(database, adapter, limit, 5)
        .map_err(|error| error.to_string())?;
    let mut report = AutomatedRepairReport {
        candidates,
        ..Default::default()
    };
    for _ in 0..limit {
        match run_one_repair_verification(
            database,
            directories.state,
            adapter,
            probe,
            hasher,
            fingerprinter,
        )
        .map_err(|error| error.to_string())?
        {
            RepairRunOutcome::Idle => break,
            RepairRunOutcome::Deferred {
                attempt_id,
                message,
            } => {
                report.deferred += 1;
                report
                    .failures
                    .push(format!("attempt {attempt_id}: {message}"));
            }
            RepairRunOutcome::Completed { decision, .. } => match decision {
                RepairVerificationDecision::Verified => report.verified += 1,
                RepairVerificationDecision::Rejected => {
                    report.rejected += 1;
                }
                RepairVerificationDecision::Unresolved => {
                    report.unresolved += 1;
                }
            },
        }
    }
    for attempt_id in database
        .verified_repair_attempt_ids(limit)
        .map_err(|error| error.to_string())?
    {
        match commit_verified_repair(database, attempt_id, directories.library, hasher) {
            Ok(commit) => report.commits.push(commit),
            Err(error) => report
                .failures
                .push(format!("attempt {attempt_id} commit: {error}")),
        }
    }
    Ok(report)
}

fn execute_database_phase<T, E, F, S>(
    database: &mut Database,
    run_id: i64,
    phase: &str,
    ordinal: u32,
    action: F,
    status: S,
) -> Result<T, ServiceError>
where
    T: Serialize,
    E: std::fmt::Display,
    F: FnOnce(&mut Database) -> Result<T, E>,
    S: FnOnce(&T) -> ServicePhaseStatus,
{
    database.start_service_phase(run_id, phase, ordinal)?;
    match action(database) {
        Ok(report) => {
            database.finish_service_phase(
                run_id,
                phase,
                status(&report),
                &serde_json::to_value(&report).map_err(DatabaseError::Json)?,
                None,
            )?;
            Ok(report)
        }
        Err(error) => {
            let message = error.to_string();
            database.finish_service_phase(
                run_id,
                phase,
                ServicePhaseStatus::Failed,
                &serde_json::json!({"error":message}),
                Some(&message),
            )?;
            Err(ServiceError::Phase {
                phase: phase.into(),
                message,
            })
        }
    }
}

/// Structured result of every complete-cycle phase.
#[derive(Debug, Serialize)]
pub struct CompleteServiceReport {
    /// Existing source/core work.
    pub core: SyncRunReport,
    /// Imported exact taste seeds.
    pub seeds: Option<DiscoverySeedReport>,
    /// Recommendation decisions.
    pub discovery: Option<DiscoveryReport>,
    /// Canonical provider routing.
    pub routing: Option<DiscoveryRoutingReport>,
    /// Jobs created by discovery in this cycle.
    pub discovery_acquisitions: AcquisitionBatchReport,
    /// Artifact health.
    pub health: ArtifactHealthReport,
    /// New artifact fingerprints.
    pub fingerprints: FingerprintReport,
    /// Conservative repair eligibility.
    pub repair_assessment: RepairAssessmentReport,
    /// Generated, independently verified, and safely committed repairs.
    pub repair_execution: AutomatedRepairReport,
    /// Canonical metadata.
    pub metadata: MetadataResolutionReport,
    /// Complete provider display metadata kept separate from identity.
    pub provider_metadata: ProviderMetadataReport,
    /// Identity-verified external genres.
    pub genres: GenreResolutionReport,
    /// Canonical artwork.
    pub artwork: ArtworkResolutionReport,
    /// Adjacent lyrics.
    pub lyrics: LyricsReport,
    /// Source-preserving tag output.
    pub tags: MetadataMaterializationReport,
    /// Final Navidrome playlists.
    pub playlists: PlaylistMaterializationReport,
}

impl CompleteServiceReport {
    /// Returns true when every phase completed without isolated failures.
    #[must_use]
    pub fn is_successful(&self) -> bool {
        self.core.is_successful()
            && self
                .routing
                .as_ref()
                .is_none_or(|report| report.failures.is_empty())
            && self.discovery_acquisitions.failures.is_empty()
            && self.health.failures.is_empty()
            && self.fingerprints.failures.is_empty()
            && self.repair_assessment.failures.is_empty()
            && self.repair_execution.failures.is_empty()
            && self.metadata.failures.is_empty()
            && self.provider_metadata.failures.is_empty()
            && self.genres.failures.is_empty()
            && self.artwork.failures.is_empty()
            && self.artwork.provider_failures.is_empty()
            && self.lyrics.failures.is_empty()
            && self.tags.failures.is_empty()
            && self.playlists.failures.is_empty()
    }
}

/// Bounded autonomous repair execution effects.
#[derive(Debug, Default, Serialize)]
pub struct AutomatedRepairReport {
    /// Search-only candidate generation.
    pub candidates: RepairCandidateReport,
    /// Independently verified attempts.
    pub verified: u64,
    /// Rejected contradictions.
    pub rejected: u64,
    /// Insufficient identity evidence.
    pub unresolved: u64,
    /// Retryable processing failures.
    pub deferred: u64,
    /// Safe committed replacements.
    pub commits: Vec<RepairCommitReport>,
    /// Isolated diagnostics.
    pub failures: Vec<String>,
}

/// Complete service-cycle failure.
#[derive(Debug, Error)]
pub enum ServiceError {
    /// Limits must be nonzero.
    #[error("complete service limits must be greater than zero")]
    InvalidLimits,
    /// Enabled discovery lacks a provider.
    #[error("enabled discovery requires a recommendation provider")]
    MissingRecommendations,
    /// Search fallback was enabled without its secret-backed AcoustID boundary.
    #[error("enabled discovery search requires an AcoustID client key")]
    MissingAcoustId,
    /// One phase failed after durable recording.
    #[error("service phase {phase} failed: {message}")]
    Phase {
        /// Stable phase name.
        phase: String,
        /// Bounded diagnostic.
        message: String,
    },
    /// Durable state failed.
    #[error(transparent)]
    Database(#[from] DatabaseError),
}
