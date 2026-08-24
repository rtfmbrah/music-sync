//! Bounded orchestration for one timer-friendly synchronization run.

use std::path::Path;

use serde::Serialize;
use thiserror::Error;

use crate::acquisition::{AcquisitionBatchReport, AcquisitionRunError, run_pending_acquisitions};
use crate::content_hash::ContentHasher;
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
    maximum_jobs: usize,
) -> Result<SyncRunReport, SyncRunError> {
    if maximum_jobs == 0 {
        return Err(SyncRunError::InvalidJobLimit);
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
        maximum_jobs,
    )?;
    let playlists = materialize_playlists(database, directories.library, directories.playlists)?;
    Ok(SyncRunReport {
        sources: source_results,
        acquisitions,
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
}

/// Structured result of one complete bounded synchronization cycle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SyncRunReport {
    /// Independently attempted active sources in stable ID order.
    pub sources: Vec<SourceSyncResult>,
    /// Bounded acquisition snapshot result.
    pub acquisitions: AcquisitionBatchReport,
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
    /// A sync run must always bound acquisition work.
    #[error("maximum acquisition jobs must be greater than zero")]
    InvalidJobLimit,
    /// Durable state access or reconciliation failed.
    #[error(transparent)]
    Database(#[from] DatabaseError),
    /// Acquisition orchestration encountered a fatal database failure.
    #[error(transparent)]
    Acquisition(#[from] AcquisitionRunError),
    /// Playlist orchestration encountered a fatal database failure.
    #[error(transparent)]
    Playlist(#[from] PlaylistError),
}
