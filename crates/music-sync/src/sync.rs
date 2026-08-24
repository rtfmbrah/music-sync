//! Bounded orchestration for one timer-friendly synchronization run.

use std::path::Path;

use serde::Serialize;
use thiserror::Error;

use crate::acquisition::{AcquisitionBatchReport, AcquisitionRunError, run_pending_acquisitions};
use crate::content_hash::ContentHasher;
use crate::fingerprint::{
    FingerprintReconciliationError, FingerprintReport, Fingerprinter,
    reconcile_artifact_fingerprints,
};
use crate::media_probe::MediaProbe;
use crate::persistence::{Database, DatabaseError, SnapshotReconciliationSummary};
use crate::playlist::{PlaylistError, PlaylistMaterializationReport, materialize_playlists};
use crate::provider::SourceId;
use crate::yt_dlp::YtDlp;

/// Runs all bounded phases required for one ordinary synchronization cycle.
pub fn run_sync(
    database: &mut Database,
    directories: SyncDirectories<'_>,
    boundaries: SyncBoundaries<'_>,
    limits: SyncLimits,
) -> Result<SyncRunReport, SyncRunError> {
    if limits.maximum_jobs == 0
        || limits.maximum_fingerprints == 0
        || limits.fingerprint_audio_seconds == 0
    {
        return Err(SyncRunError::InvalidLimits);
    }
    let sources = database.list_sources(false)?;
    let mut source_results = Vec::with_capacity(sources.len());
    for source in sources {
        match boundaries.source_adapter.enumerate(&source.url) {
            Ok(snapshot) => {
                let summary = database.reconcile_source_snapshot(source.id, &snapshot)?;
                source_results.push(SourceSyncResult::Reconciled {
                    source_id: source.id,
                    summary,
                });
            }
            Err(error) => source_results.push(SourceSyncResult::Failed {
                source_id: source.id,
                message: error.to_string(),
            }),
        }
    }
    let acquisitions = run_pending_acquisitions(
        database,
        directories.state,
        directories.library,
        boundaries.acquisition_adapter,
        boundaries.probe,
        boundaries.hasher,
        limits.maximum_jobs,
    )?;
    let fingerprints = reconcile_artifact_fingerprints(
        database,
        directories.library,
        boundaries.fingerprinter,
        limits.maximum_fingerprints,
        limits.fingerprint_audio_seconds,
    )?;
    let playlists = materialize_playlists(database, directories.library, directories.playlists)?;
    Ok(SyncRunReport {
        sources: source_results,
        acquisitions,
        fingerprints,
        playlists,
    })
}

/// Filesystem roots used by one synchronization run.
#[derive(Debug, Clone, Copy)]
pub struct SyncDirectories<'a> {
    /// Durable application state and acquisition staging root.
    pub state: &'a Path,
    /// Managed audio library root.
    pub library: &'a Path,
    /// Owned Navidrome playlist output root.
    pub playlists: &'a Path,
}

/// External boundaries used by one synchronization run.
pub struct SyncBoundaries<'a> {
    /// Provider adapter with the source-enumeration deadline.
    pub source_adapter: &'a YtDlp,
    /// Provider adapter with the per-download deadline.
    pub acquisition_adapter: &'a YtDlp,
    /// Structural media probe boundary.
    pub probe: &'a dyn MediaProbe,
    /// Exact-byte content hashing boundary.
    pub hasher: &'a dyn ContentHasher,
    /// Perceptual fingerprint extraction boundary.
    pub fingerprinter: &'a dyn Fingerprinter,
}

/// Explicit non-zero work bounds for one synchronization run.
#[derive(Debug, Clone, Copy)]
pub struct SyncLimits {
    /// Maximum pending acquisitions attempted once.
    pub maximum_jobs: usize,
    /// Maximum healthy artifacts fingerprint-reconciled.
    pub maximum_fingerprints: usize,
    /// Maximum audio seconds consumed per fingerprint.
    pub fingerprint_audio_seconds: u32,
}

/// Structured result of one complete bounded synchronization cycle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SyncRunReport {
    /// Independently attempted active sources in stable ID order.
    pub sources: Vec<SourceSyncResult>,
    /// Bounded acquisition snapshot result.
    pub acquisitions: AcquisitionBatchReport,
    /// Automatic raw fingerprint reconciliation after acquisition.
    pub fingerprints: FingerprintReport,
    /// Atomic playlist materialization result.
    pub playlists: PlaylistMaterializationReport,
}

impl SyncRunReport {
    /// Returns true when every independently recoverable operation succeeded.
    #[must_use]
    pub fn is_successful(&self) -> bool {
        self.sources
            .iter()
            .all(|result| matches!(result, SourceSyncResult::Reconciled { .. }))
            && self.acquisitions.failures.is_empty()
            && self.fingerprints.failures.is_empty()
            && self.playlists.failures.is_empty()
    }
}

/// Result of independently synchronizing one active source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum SourceSyncResult {
    /// The provider snapshot was enumerated and committed.
    Reconciled {
        /// Durable configured source ID.
        source_id: SourceId,
        /// Transactional membership and job effects.
        summary: SnapshotReconciliationSummary,
    },
    /// Enumeration failed before any membership mutation.
    Failed {
        /// Durable configured source ID.
        source_id: SourceId,
        /// Provider failure diagnostic.
        message: String,
    },
}

/// Fatal failure preventing completion of all synchronization phases.
#[derive(Debug, Error)]
pub enum SyncRunError {
    /// A sync run must always bound acquisition and fingerprint work.
    #[error("sync acquisition and fingerprint limits must be greater than zero")]
    InvalidLimits,
    /// Durable state access or reconciliation failed.
    #[error(transparent)]
    Database(#[from] DatabaseError),
    /// Acquisition orchestration encountered a fatal database failure.
    #[error(transparent)]
    Acquisition(#[from] AcquisitionRunError),
    /// Fingerprint orchestration encountered a fatal durable-state failure.
    #[error(transparent)]
    Fingerprint(#[from] FingerprintReconciliationError),
    /// Playlist orchestration encountered a fatal database failure.
    #[error(transparent)]
    Playlist(#[from] PlaylistError),
}
