//! SQLite persistence and schema migration foundation.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags, OptionalExtension, Transaction, TransactionBehavior};
use thiserror::Error;

use crate::acquisition::ValidatedStagedMedia;
use crate::config::DiscoveryConfig;
use crate::discovery::{DiscoveryLane, Recommendation, score as discovery_score};
use crate::musicbrainz::{CanonicalRecording, CanonicalRelease};
use crate::provider::SourceSnapshot;
use crate::provider::{AddSourceResult, ConfiguredSource, SourceId};

const MIGRATIONS: &[(u32, &str)] = &[
    (1, include_str!("../migrations/0001_initial.sql")),
    (2, include_str!("../migrations/0002_sources.sql")),
    (3, include_str!("../migrations/0003_source_collections.sql")),
    (4, include_str!("../migrations/0004_acquisition_jobs.sql")),
    (
        5,
        include_str!("../migrations/0005_acquisition_commits.sql"),
    ),
    (6, include_str!("../migrations/0006_playlist_outputs.sql")),
    (
        7,
        include_str!("../migrations/0007_artifact_fingerprints.sql"),
    ),
    (8, include_str!("../migrations/0008_repair_cases.sql")),
    (9, include_str!("../migrations/0009_repair_execution.sql")),
    (
        10,
        include_str!("../migrations/0010_acquisition_canonical_evidence.sql"),
    ),
    (
        11,
        include_str!("../migrations/0011_metadata_provenance.sql"),
    ),
    (12, include_str!("../migrations/0012_release_artwork.sql")),
    (13, include_str!("../migrations/0013_lyrics_sidecars.sql")),
    (
        14,
        include_str!("../migrations/0014_metadata_materialization.sql"),
    ),
    (15, include_str!("../migrations/0015_discovery.sql")),
    (16, include_str!("../migrations/0016_service_runs.sql")),
];

/// Current durable schema version.
pub const CURRENT_SCHEMA_VERSION: u32 = 16;

/// A connection to music-sync's private application state.
#[derive(Debug)]
pub struct Database {
    connection: Connection,
}

impl Database {
    /// Opens a database, creating its parent directory, and applies migrations.
    pub fn open(path: &Path) -> Result<Self, DatabaseError> {
        let parent = path.parent().ok_or_else(|| DatabaseError::MissingParent {
            path: path.to_path_buf(),
        })?;
        fs::create_dir_all(parent).map_err(|source| DatabaseError::CreateDirectory {
            path: parent.to_path_buf(),
            source,
        })?;
        let connection = Connection::open(path).map_err(|source| DatabaseError::Open {
            path: path.to_path_buf(),
            source,
        })?;
        let mut database = Self { connection };
        database.configure()?;
        database.migrate()?;
        Ok(database)
    }

    /// Opens a migrated in-memory database, primarily for deterministic tests.
    pub fn open_in_memory() -> Result<Self, DatabaseError> {
        let connection = Connection::open_in_memory().map_err(|source| DatabaseError::Open {
            path: PathBuf::from(":memory:"),
            source,
        })?;
        let mut database = Self { connection };
        database.configure()?;
        database.migrate()?;
        Ok(database)
    }

    /// Writes a transactionally consistent SQLite snapshot without replacing an
    /// existing file. The caller owns destination naming and retention.
    pub fn backup_to(&self, destination: &Path) -> Result<(), DatabaseError> {
        if destination.exists() {
            return Err(DatabaseError::BackupAlreadyExists(
                destination.to_path_buf(),
            ));
        }
        let parent = destination
            .parent()
            .ok_or_else(|| DatabaseError::MissingParent {
                path: destination.to_path_buf(),
            })?;
        if !parent.is_dir() {
            return Err(DatabaseError::BackupParentMissing(parent.to_path_buf()));
        }
        let destination_text = destination
            .to_str()
            .ok_or_else(|| DatabaseError::BackupPathNotUtf8(destination.to_path_buf()))?;
        self.connection
            .execute("VACUUM INTO ?1", [destination_text])
            .map_err(DatabaseError::Sqlite)?;
        Ok(())
    }

    /// Checks an existing database without creating or migrating it.
    pub fn inspect_read_only(path: &Path) -> Result<DatabaseInspection, DatabaseError> {
        let connection = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(|source| DatabaseError::Open {
            path: path.to_path_buf(),
            source,
        })?;
        connection
            .query_row("PRAGMA quick_check", [], |row| row.get::<_, String>(0))
            .map_err(DatabaseError::Sqlite)
            .and_then(|result| {
                if result == "ok" {
                    Ok(())
                } else {
                    Err(DatabaseError::Integrity(result))
                }
            })?;
        let version = connection
            .query_row("PRAGMA user_version", [], |row| row.get::<_, u32>(0))
            .map_err(DatabaseError::Sqlite)?;
        Ok(DatabaseInspection { version })
    }

    /// Reads a bounded operational summary without creating, migrating, or writing state.
    pub fn operational_status_read_only(
        path: &Path,
        recent_event_limit: usize,
    ) -> Result<OperationalStatus, DatabaseError> {
        let connection = open_immutable_current_schema(path)?;
        let version = CURRENT_SCHEMA_VERSION;
        let count = |sql: &str| {
            connection
                .query_row(sql, [], |row| row.get::<_, u64>(0))
                .map_err(DatabaseError::Sqlite)
        };
        let mut statement = connection
            .prepare(
                "SELECT created_at, level, component, event, message, run_id, job_id
                 FROM events
                 WHERE level IN ('warning', 'error')
                 ORDER BY id DESC LIMIT ?1",
            )
            .map_err(DatabaseError::Sqlite)?;
        let limit = i64::try_from(recent_event_limit).unwrap_or(i64::MAX);
        let events = statement
            .query_map([limit], |row| {
                Ok(OperationalEvent {
                    created_at: row.get(0)?,
                    level: row.get(1)?,
                    component: row.get(2)?,
                    event: row.get(3)?,
                    message: row.get(4)?,
                    run_id: row.get(5)?,
                    job_id: row.get(6)?,
                })
            })
            .map_err(DatabaseError::Sqlite)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(DatabaseError::Sqlite)?;
        Ok(OperationalStatus {
            schema_version: version,
            active_sources: count("SELECT COUNT(*) FROM sources WHERE active = 1")?,
            inactive_sources: count("SELECT COUNT(*) FROM sources WHERE active = 0")?,
            collections: count("SELECT COUNT(*) FROM collections")?,
            active_memberships: count(
                "SELECT COUNT(*) FROM collection_memberships WHERE active = 1",
            )?,
            unresolved_active_memberships: count(
                "SELECT COUNT(*)
                 FROM collection_memberships
                 JOIN provider_items ON provider_items.id =
                      collection_memberships.provider_item_id
                 LEFT JOIN recordings ON recordings.id = provider_items.recording_id
                 LEFT JOIN artifacts ON artifacts.id = recordings.preferred_artifact_id
                 WHERE collection_memberships.active = 1
                   AND (artifacts.id IS NULL OR artifacts.health != 'healthy')",
            )?,
            jobs: JobStatusCounts {
                pending: count("SELECT COUNT(*) FROM jobs WHERE status = 'pending'")?,
                running: count("SELECT COUNT(*) FROM jobs WHERE status = 'running'")?,
                succeeded: count("SELECT COUNT(*) FROM jobs WHERE status = 'succeeded'")?,
                failed: count("SELECT COUNT(*) FROM jobs WHERE status = 'failed'")?,
                deferred: count("SELECT COUNT(*) FROM jobs WHERE status = 'deferred'")?,
            },
            artifacts: ArtifactHealthCounts {
                unknown: count("SELECT COUNT(*) FROM artifacts WHERE health = 'unknown'")?,
                healthy: count("SELECT COUNT(*) FROM artifacts WHERE health = 'healthy'")?,
                missing: count("SELECT COUNT(*) FROM artifacts WHERE health = 'missing'")?,
                corrupt: count("SELECT COUNT(*) FROM artifacts WHERE health = 'corrupt'")?,
            },
            repairs: RepairCaseCounts {
                eligible: count("SELECT COUNT(*) FROM repair_cases WHERE state = 'eligible'")?,
                unresolved: count("SELECT COUNT(*) FROM repair_cases WHERE state = 'unresolved'")?,
                verified: count("SELECT COUNT(*) FROM repair_cases WHERE state = 'verified'")?,
                cancelled: count("SELECT COUNT(*) FROM repair_cases WHERE state = 'cancelled'")?,
            },
            repair_attempts: RepairAttemptCounts {
                generated: count("SELECT COUNT(*) FROM repair_attempts WHERE state = 'generated'")?,
                running: count("SELECT COUNT(*) FROM repair_attempts WHERE state = 'running'")?,
                deferred: count("SELECT COUNT(*) FROM repair_attempts WHERE state = 'deferred'")?,
                rejected: count("SELECT COUNT(*) FROM repair_attempts WHERE state = 'rejected'")?,
                unresolved: count(
                    "SELECT COUNT(*) FROM repair_attempts WHERE state = 'unresolved'",
                )?,
                verified: count("SELECT COUNT(*) FROM repair_attempts WHERE state = 'verified'")?,
                committed: count("SELECT COUNT(*) FROM repair_attempts WHERE state = 'committed'")?,
            },
            metadata: MetadataResolutionCounts {
                pending: count(
                    "SELECT COUNT(*) FROM metadata_resolutions WHERE state = 'pending'",
                )?,
                resolved: count(
                    "SELECT COUNT(*) FROM metadata_resolutions WHERE state = 'resolved'",
                )?,
                ambiguous: count(
                    "SELECT COUNT(*) FROM metadata_resolutions WHERE state = 'ambiguous'",
                )?,
                deferred: count(
                    "SELECT COUNT(*) FROM metadata_resolutions WHERE state = 'deferred'",
                )?,
                selected_fields: count("SELECT COUNT(*) FROM metadata_selections")?,
            },
            artwork: ArtworkResolutionCounts {
                resolved: count(
                    "SELECT COUNT(*) FROM artwork_resolutions WHERE state = 'resolved'",
                )?,
                unavailable: count(
                    "SELECT COUNT(*) FROM artwork_resolutions WHERE state = 'unavailable'",
                )?,
                deferred: count(
                    "SELECT COUNT(*) FROM artwork_resolutions WHERE state = 'deferred'",
                )?,
                cached_blobs: count("SELECT COUNT(*) FROM artwork_blobs")?,
            },
            lyrics: LyricsResolutionCounts {
                resolved: count(
                    "SELECT COUNT(*) FROM lyrics_resolutions WHERE state = 'resolved'",
                )?,
                instrumental: count(
                    "SELECT COUNT(*) FROM lyrics_resolutions WHERE state = 'instrumental'",
                )?,
                unavailable: count(
                    "SELECT COUNT(*) FROM lyrics_resolutions WHERE state = 'unavailable'",
                )?,
                deferred: count(
                    "SELECT COUNT(*) FROM lyrics_resolutions WHERE state = 'deferred'",
                )?,
                committed_outputs: count(
                    "SELECT COUNT(*) FROM lyrics_outputs WHERE state = 'committed'",
                )?,
            },
            metadata_materializations: MetadataMaterializationCounts {
                prepared: count(
                    "SELECT COUNT(*) FROM metadata_materializations WHERE state='prepared'",
                )?,
                committed: count(
                    "SELECT COUNT(*) FROM metadata_materializations WHERE state='committed'",
                )?,
                deferred: count(
                    "SELECT COUNT(*) FROM metadata_materialization_states WHERE state='deferred'",
                )?,
            },
            discovery: DiscoveryCounts {
                approved: count(
                    "SELECT COUNT(*) FROM discovery_candidates WHERE state='approved'",
                )?,
                queued: count("SELECT COUNT(*) FROM discovery_candidates WHERE state='queued'")?,
                unresolved: count(
                    "SELECT COUNT(*) FROM discovery_candidates WHERE state='unresolved'",
                )?,
                acquired: count(
                    "SELECT COUNT(*) FROM discovery_candidates WHERE state='acquired'",
                )?,
                budget_rejected: count(
                    "SELECT COUNT(*) FROM discovery_candidates WHERE state='budget_rejected'",
                )?,
            },
            playlist_outputs: count(
                "SELECT COUNT(*) FROM playlist_outputs WHERE sha256 IS NOT NULL",
            )?,
            recent_events: events,
        })
    }

    /// Starts one mutually exclusive durable service cycle.
    pub fn start_service_run(
        &mut self,
        trigger: ServiceRunTrigger,
        binary_version: &str,
    ) -> Result<i64, DatabaseError> {
        if binary_version.trim().is_empty() {
            return Err(DatabaseError::InvalidServiceRunVersion);
        }
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(DatabaseError::Sqlite)?;
        let running = transaction
            .query_row(
                "SELECT id FROM service_runs WHERE status='running'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .optional()
            .map_err(DatabaseError::Sqlite)?;
        if let Some(run_id) = running {
            return Err(DatabaseError::ServiceRunAlreadyRunning(run_id));
        }
        transaction
            .execute(
                "INSERT INTO service_runs(trigger,status,binary_version) VALUES (?1,'running',?2)",
                rusqlite::params![trigger.as_str(), binary_version],
            )
            .map_err(DatabaseError::Sqlite)?;
        let id = transaction.last_insert_rowid();
        transaction.commit().map_err(DatabaseError::Sqlite)?;
        tracing::info!(
            service_run_id = id,
            trigger = trigger.as_str(),
            "service cycle started"
        );
        Ok(id)
    }

    /// Starts one uniquely named ordered phase inside a running service cycle.
    pub fn start_service_phase(
        &mut self,
        run_id: i64,
        phase: &str,
        ordinal: u32,
    ) -> Result<(), DatabaseError> {
        if phase.trim().is_empty() {
            return Err(DatabaseError::InvalidServicePhase);
        }
        let changed=self.connection.execute("INSERT INTO service_run_phases(service_run_id,phase,ordinal,status) SELECT ?1,?2,?3,'running' FROM service_runs WHERE id=?1 AND status='running'",rusqlite::params![run_id,phase,ordinal]).map_err(DatabaseError::Sqlite)?;
        if changed == 1 {
            tracing::info!(
                service_run_id = run_id,
                phase,
                ordinal,
                "service phase started"
            );
            Ok(())
        } else {
            Err(DatabaseError::ServiceRunNotRunning(run_id))
        }
    }

    /// Finishes one running phase with bounded structured summary and message.
    pub fn finish_service_phase(
        &mut self,
        run_id: i64,
        phase: &str,
        status: ServicePhaseStatus,
        summary: &serde_json::Value,
        message: Option<&str>,
    ) -> Result<(), DatabaseError> {
        let summary = serde_json::to_string(summary).map_err(DatabaseError::Json)?;
        let changed=self.connection.execute("UPDATE service_run_phases SET status=?3,summary_json=?4,message=?5,finished_at=CURRENT_TIMESTAMP WHERE service_run_id=?1 AND phase=?2 AND status='running'",rusqlite::params![run_id,phase,status.as_str(),summary,message]).map_err(DatabaseError::Sqlite)?;
        if changed == 1 {
            tracing::info!(
                service_run_id = run_id,
                phase,
                status = status.as_str(),
                "service phase finished"
            );
            Ok(())
        } else {
            Err(DatabaseError::ServicePhaseNotRunning {
                run_id,
                phase: phase.into(),
            })
        }
    }

    /// Finishes a running service cycle after every phase reached a terminal state.
    pub fn finish_service_run(
        &mut self,
        run_id: i64,
        status: ServiceRunTerminalStatus,
        summary: &serde_json::Value,
    ) -> Result<(), DatabaseError> {
        let summary = serde_json::to_string(summary).map_err(DatabaseError::Json)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(DatabaseError::Sqlite)?;
        let running_phases=transaction.query_row("SELECT COUNT(*) FROM service_run_phases WHERE service_run_id=?1 AND status='running'",[run_id],|row|row.get::<_,u64>(0)).map_err(DatabaseError::Sqlite)?;
        if running_phases != 0 {
            return Err(DatabaseError::ServiceRunHasRunningPhases(run_id));
        }
        let changed=transaction.execute("UPDATE service_runs SET status=?2,summary_json=?3,finished_at=CURRENT_TIMESTAMP WHERE id=?1 AND status='running'",rusqlite::params![run_id,status.as_str(),summary]).map_err(DatabaseError::Sqlite)?;
        if changed != 1 {
            return Err(DatabaseError::ServiceRunNotRunning(run_id));
        }
        transaction.commit().map_err(DatabaseError::Sqlite)?;
        tracing::info!(
            service_run_id = run_id,
            status = status.as_str(),
            "service cycle finished"
        );
        Ok(())
    }

    /// Explicitly marks an abandoned service cycle and its active phase interrupted.
    pub fn recover_interrupted_service_run(&mut self) -> Result<Option<i64>, DatabaseError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(DatabaseError::Sqlite)?;
        let run_id = transaction
            .query_row(
                "SELECT id FROM service_runs WHERE status='running'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .optional()
            .map_err(DatabaseError::Sqlite)?;
        if let Some(run_id) = run_id {
            transaction.execute("UPDATE service_run_phases SET status='failed',message='operator marked abandoned service run interrupted',summary_json='{}',finished_at=CURRENT_TIMESTAMP WHERE service_run_id=?1 AND status='running'",[run_id]).map_err(DatabaseError::Sqlite)?;
            transaction.execute("UPDATE service_runs SET status='interrupted',summary_json='{\"reason\":\"operator recovery\"}',finished_at=CURRENT_TIMESTAMP WHERE id=?1",[run_id]).map_err(DatabaseError::Sqlite)?;
        }
        transaction.commit().map_err(DatabaseError::Sqlite)?;
        if let Some(run_id) = run_id {
            tracing::warn!(
                service_run_id = run_id,
                "service cycle marked interrupted by operator"
            );
        }
        Ok(run_id)
    }

    /// Reads bounded newest-first service-cycle history without mutating state.
    pub fn service_run_history_read_only(
        path: &Path,
        limit: usize,
        status: Option<ServiceRunHistoryStatus>,
    ) -> Result<Vec<ServiceRunHistoryEntry>, DatabaseError> {
        let connection = open_immutable_current_schema(path)?;
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        let mut statement=connection.prepare("SELECT id,trigger,status,binary_version,started_at,finished_at,summary_json,(SELECT COUNT(*) FROM service_run_phases WHERE service_run_id=service_runs.id),(SELECT COUNT(*) FROM service_run_phases WHERE service_run_id=service_runs.id AND status IN ('failed','partial')) FROM service_runs WHERE (?2 IS NULL OR status=?2) ORDER BY id DESC LIMIT ?1").map_err(DatabaseError::Sqlite)?;
        statement
            .query_map(
                rusqlite::params![limit, status.map(ServiceRunHistoryStatus::as_str)],
                |row| {
                    Ok(ServiceRunHistoryEntry {
                        id: row.get(0)?,
                        trigger: row.get(1)?,
                        status: row.get(2)?,
                        binary_version: row.get(3)?,
                        started_at: row.get(4)?,
                        finished_at: row.get(5)?,
                        summary_json: row.get(6)?,
                        phase_count: row.get(7)?,
                        failed_phase_count: row.get(8)?,
                    })
                },
            )
            .map_err(DatabaseError::Sqlite)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(DatabaseError::Sqlite)
    }

    /// Reads one service cycle and its ordered phase details without mutation.
    pub fn service_run_detail_read_only(
        path: &Path,
        run_id: i64,
    ) -> Result<Option<ServiceRunDetail>, DatabaseError> {
        let connection = open_immutable_current_schema(path)?;
        let run=connection.query_row("SELECT id,trigger,status,binary_version,started_at,finished_at,summary_json FROM service_runs WHERE id=?1",[run_id],|row|Ok(ServiceRunHistoryEntry{id:row.get(0)?,trigger:row.get(1)?,status:row.get(2)?,binary_version:row.get(3)?,started_at:row.get(4)?,finished_at:row.get(5)?,summary_json:row.get(6)?,phase_count:0,failed_phase_count:0})).optional().map_err(DatabaseError::Sqlite)?;
        let Some(mut run) = run else { return Ok(None) };
        let mut statement=connection.prepare("SELECT phase,ordinal,status,started_at,finished_at,summary_json,message FROM service_run_phases WHERE service_run_id=?1 ORDER BY ordinal").map_err(DatabaseError::Sqlite)?;
        let phases = statement
            .query_map([run_id], |row| {
                Ok(ServiceRunPhaseHistoryEntry {
                    phase: row.get(0)?,
                    ordinal: row.get(1)?,
                    status: row.get(2)?,
                    started_at: row.get(3)?,
                    finished_at: row.get(4)?,
                    summary_json: row.get(5)?,
                    message: row.get(6)?,
                })
            })
            .map_err(DatabaseError::Sqlite)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(DatabaseError::Sqlite)?;
        run.phase_count = phases.len() as u64;
        run.failed_phase_count = phases
            .iter()
            .filter(|phase| matches!(phase.status.as_str(), "failed" | "partial"))
            .count() as u64;
        Ok(Some(ServiceRunDetail { run, phases }))
    }

    /// Reads bounded newest-first persisted events with exact optional filters.
    pub fn operational_events_read_only(
        path: &Path,
        limit: usize,
        level: Option<&str>,
        component: Option<&str>,
        run_id: Option<i64>,
        job_id: Option<i64>,
    ) -> Result<Vec<OperationalEvent>, DatabaseError> {
        let connection = open_immutable_current_schema(path)?;
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        let mut statement=connection.prepare("SELECT created_at,level,component,event,message,run_id,job_id FROM events WHERE (?2 IS NULL OR level=?2) AND (?3 IS NULL OR component=?3) AND (?4 IS NULL OR run_id=?4) AND (?5 IS NULL OR job_id=?5) ORDER BY id DESC LIMIT ?1").map_err(DatabaseError::Sqlite)?;
        statement
            .query_map(
                rusqlite::params![limit, level, component, run_id, job_id],
                |row| {
                    Ok(OperationalEvent {
                        created_at: row.get(0)?,
                        level: row.get(1)?,
                        component: row.get(2)?,
                        event: row.get(3)?,
                        message: row.get(4)?,
                        run_id: row.get(5)?,
                        job_id: row.get(6)?,
                    })
                },
            )
            .map_err(DatabaseError::Sqlite)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(DatabaseError::Sqlite)
    }

    /// Reads bounded newest-first acquisition history without mutating SQLite state.
    pub fn acquisition_history_read_only(
        path: &Path,
        limit: usize,
    ) -> Result<Vec<AcquisitionHistoryEntry>, DatabaseError> {
        let connection = open_immutable_current_schema(path)?;
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        let mut statement = connection
            .prepare(
                "SELECT jobs.id, jobs.status, jobs.attempt_count, jobs.created_at,
                        jobs.updated_at, provider_items.provider,
                        provider_items.provider_item_id, provider_items.original_url,
                        (SELECT events.message FROM events
                         WHERE events.job_id = jobs.id
                           AND events.level IN ('warning', 'error')
                         ORDER BY events.id DESC LIMIT 1)
                 FROM jobs
                 JOIN acquisition_jobs ON acquisition_jobs.job_id = jobs.id
                 JOIN provider_items ON provider_items.id = acquisition_jobs.provider_item_id
                 WHERE jobs.kind = 'acquire'
                 ORDER BY jobs.id DESC LIMIT ?1",
            )
            .map_err(DatabaseError::Sqlite)?;
        statement
            .query_map([limit], |row| {
                Ok(AcquisitionHistoryEntry {
                    job_id: row.get(0)?,
                    status: row.get(1)?,
                    attempt_count: row.get(2)?,
                    created_at: row.get(3)?,
                    updated_at: row.get(4)?,
                    provider: row.get(5)?,
                    provider_item_id: row.get(6)?,
                    original_url: row.get(7)?,
                    latest_message: row.get(8)?,
                })
            })
            .map_err(DatabaseError::Sqlite)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(DatabaseError::Sqlite)
    }

    /// Loads a stable bounded set of registered artifacts for local health checks.
    pub fn artifact_health_candidates(
        &self,
        limit: usize,
    ) -> Result<Vec<ArtifactHealthCandidate>, DatabaseError> {
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        let mut statement = self
            .connection
            .prepare("SELECT id, path, sha256, health FROM artifacts ORDER BY id LIMIT ?1")
            .map_err(DatabaseError::Sqlite)?;
        let rows = statement
            .query_map([limit], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })
            .map_err(DatabaseError::Sqlite)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(DatabaseError::Sqlite)?;
        rows.into_iter()
            .map(|(id, path, sha256, health)| {
                Ok(ArtifactHealthCandidate {
                    id,
                    path: PathBuf::from(path),
                    sha256,
                    health: ArtifactHealth::parse(&health)?,
                })
            })
            .collect()
    }

    /// Loads healthy artifacts in stable order for fingerprint reconciliation.
    pub fn artifact_fingerprint_candidates(
        &self,
        limit: usize,
    ) -> Result<Vec<ArtifactFingerprintCandidate>, DatabaseError> {
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        let mut statement = self
            .connection
            .prepare(
                "SELECT artifacts.id, artifacts.path,
                        artifact_fingerprints.max_seconds,
                        artifact_fingerprints.fingerprint_json
                 FROM artifacts
                 LEFT JOIN artifact_fingerprints ON
                           artifact_fingerprints.artifact_id = artifacts.id
                 WHERE artifacts.health = 'healthy'
                 ORDER BY artifacts.id LIMIT ?1",
            )
            .map_err(DatabaseError::Sqlite)?;
        statement
            .query_map([limit], |row| {
                Ok(ArtifactFingerprintCandidate {
                    artifact_id: row.get(0)?,
                    path: PathBuf::from(row.get::<_, String>(1)?),
                    prior_max_seconds: row.get(2)?,
                    prior_fingerprint_json: row.get(3)?,
                })
            })
            .map_err(DatabaseError::Sqlite)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(DatabaseError::Sqlite)
    }

    /// Upserts exact raw fingerprint evidence and reports whether bytes changed.
    pub fn record_artifact_fingerprint(
        &mut self,
        evidence: &ArtifactFingerprintEvidence,
    ) -> Result<bool, DatabaseError> {
        let prior = self
            .connection
            .query_row(
                "SELECT algorithm, max_seconds, duration_ms, fingerprint_json
                 FROM artifact_fingerprints WHERE artifact_id = ?1",
                [evidence.artifact_id],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                },
            )
            .optional()
            .map_err(DatabaseError::Sqlite)?;
        let current = (
            2_i64,
            i64::from(evidence.max_seconds),
            evidence.duration_ms,
            evidence.fingerprint_json.clone(),
        );
        if prior.as_ref() == Some(&current) {
            return Ok(false);
        }
        self.connection
            .execute(
                "INSERT INTO artifact_fingerprints(
                     artifact_id, algorithm, max_seconds, duration_ms,
                     fingerprint_json, value_count)
                 VALUES (?1, 2, ?2, ?3, ?4, ?5)
                 ON CONFLICT(artifact_id) DO UPDATE SET
                     algorithm = 2, max_seconds = excluded.max_seconds,
                     duration_ms = excluded.duration_ms,
                     fingerprint_json = excluded.fingerprint_json,
                     value_count = excluded.value_count,
                     updated_at = CURRENT_TIMESTAMP",
                rusqlite::params![
                    evidence.artifact_id,
                    evidence.max_seconds,
                    evidence.duration_ms,
                    evidence.fingerprint_json,
                    evidence.value_count
                ],
            )
            .map_err(DatabaseError::Sqlite)?;
        Ok(true)
    }

    /// Loads stable healthy recordings with strong identity but no resolved metadata.
    pub fn metadata_resolution_candidates(
        &self,
        limit: usize,
    ) -> Result<Vec<MetadataResolutionCandidate>, DatabaseError> {
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        let mut statement = self
            .connection
            .prepare(
                "SELECT recordings.id, recordings.musicbrainz_recording_id, recordings.isrc
             FROM recordings
             JOIN artifacts ON artifacts.id = recordings.preferred_artifact_id
             LEFT JOIN metadata_resolutions ON metadata_resolutions.recording_id = recordings.id
             WHERE artifacts.health = 'healthy'
               AND (recordings.musicbrainz_recording_id IS NOT NULL OR recordings.isrc IS NOT NULL)
               AND (metadata_resolutions.recording_id IS NULL
                    OR metadata_resolutions.state IN ('pending', 'deferred'))
             ORDER BY recordings.id LIMIT ?1",
            )
            .map_err(DatabaseError::Sqlite)?;
        statement
            .query_map([limit], |row| {
                Ok(MetadataResolutionCandidate {
                    recording_id: row.get(0)?,
                    musicbrainz_recording_id: row.get(1)?,
                    isrc: row.get(2)?,
                })
            })
            .map_err(DatabaseError::Sqlite)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(DatabaseError::Sqlite)
    }

    /// Records an ambiguous or retryable canonical resolution without changing metadata.
    pub fn record_metadata_resolution_state(
        &mut self,
        recording_id: i64,
        state: MetadataResolutionState,
        method: MetadataResolutionMethod,
        candidate_count: usize,
        message: &str,
        raw_response_json: &str,
    ) -> Result<(), DatabaseError> {
        if state == MetadataResolutionState::Resolved {
            return Err(DatabaseError::InvalidMetadataResolutionTransition);
        }
        let candidate_count = i64::try_from(candidate_count)
            .map_err(|_| DatabaseError::MetadataCandidateCountTooLarge)?;
        self.connection
            .execute(
                "INSERT INTO metadata_resolutions(recording_id, state, method,
                 candidate_count, message, raw_response_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(recording_id) DO UPDATE SET state = excluded.state,
                 method = excluded.method, candidate_count = excluded.candidate_count,
                 message = excluded.message, raw_response_json = excluded.raw_response_json,
                 updated_at = CURRENT_TIMESTAMP",
                rusqlite::params![
                    recording_id,
                    state.as_str(),
                    method.as_str(),
                    candidate_count,
                    message,
                    raw_response_json
                ],
            )
            .map_err(DatabaseError::Sqlite)?;
        Ok(())
    }

    /// Persists one unambiguous MusicBrainz recording and field-level provenance atomically.
    pub fn record_resolved_metadata(
        &mut self,
        candidate: &MetadataResolutionCandidate,
        method: MetadataResolutionMethod,
        recording: &CanonicalRecording,
        selected_release: Option<&CanonicalRelease>,
        raw_response_json: &str,
    ) -> Result<MetadataPersistenceSummary, DatabaseError> {
        if candidate
            .musicbrainz_recording_id
            .as_deref()
            .is_some_and(|id| id != recording.id)
            || (method == MetadataResolutionMethod::Isrc
                && candidate
                    .isrc
                    .as_ref()
                    .is_some_and(|isrc| !recording.isrcs.contains(isrc)))
        {
            return Err(DatabaseError::MetadataIdentityConflict(
                candidate.recording_id,
            ));
        }
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(DatabaseError::Sqlite)?;
        let exists = transaction
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM recordings WHERE id = ?1)",
                [candidate.recording_id],
                |row| row.get::<_, bool>(0),
            )
            .map_err(DatabaseError::Sqlite)?;
        if !exists {
            return Err(DatabaseError::RecordingNotFound(candidate.recording_id));
        }
        transaction
            .execute(
                "UPDATE recordings SET musicbrainz_recording_id = COALESCE(
                 musicbrainz_recording_id, ?2) WHERE id = ?1",
                rusqlite::params![candidate.recording_id, recording.id],
            )
            .map_err(DatabaseError::Sqlite)?;
        transaction
            .execute(
                "DELETE FROM recording_artist_credits WHERE recording_id = ?1",
                [candidate.recording_id],
            )
            .map_err(DatabaseError::Sqlite)?;
        for (position, credit) in recording.artist_credit.iter().enumerate() {
            transaction
                .execute(
                    "INSERT INTO artists(musicbrainz_artist_id, canonical_name, sort_name,
                     disambiguation) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(musicbrainz_artist_id) DO UPDATE SET
                     canonical_name = excluded.canonical_name,
                     sort_name = excluded.sort_name,
                     disambiguation = excluded.disambiguation,
                     updated_at = CURRENT_TIMESTAMP",
                    rusqlite::params![
                        credit.artist_id,
                        credit.artist_name,
                        credit.sort_name,
                        credit.disambiguation
                    ],
                )
                .map_err(DatabaseError::Sqlite)?;
            let artist_id = transaction
                .query_row(
                    "SELECT id FROM artists WHERE musicbrainz_artist_id = ?1",
                    [&credit.artist_id],
                    |row| row.get::<_, i64>(0),
                )
                .map_err(DatabaseError::Sqlite)?;
            transaction
                .execute(
                    "INSERT INTO recording_artist_credits(recording_id, position, artist_id,
                     credited_name, join_phrase) VALUES (?1, ?2, ?3, ?4, ?5)",
                    rusqlite::params![
                        candidate.recording_id,
                        position as i64,
                        artist_id,
                        credit.credited_name,
                        credit.join_phrase
                    ],
                )
                .map_err(DatabaseError::Sqlite)?;
        }
        let release_id = if let Some(release) = selected_release {
            let secondary_types_json =
                serde_json::to_string(&release.secondary_types).map_err(DatabaseError::Json)?;
            transaction
                .execute(
                    "INSERT INTO releases(musicbrainz_release_id, release_group_id,
                     canonical_title, release_date, country, status, primary_type,
                     secondary_types_json) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                 ON CONFLICT(musicbrainz_release_id) DO UPDATE SET
                     release_group_id = excluded.release_group_id,
                     canonical_title = excluded.canonical_title,
                     release_date = excluded.release_date, country = excluded.country,
                     status = excluded.status, primary_type = excluded.primary_type,
                     secondary_types_json = excluded.secondary_types_json,
                     updated_at = CURRENT_TIMESTAMP",
                    rusqlite::params![
                        release.id,
                        release.release_group_id,
                        release.title,
                        release.date,
                        release.country,
                        release.status,
                        release.primary_type,
                        secondary_types_json
                    ],
                )
                .map_err(DatabaseError::Sqlite)?;
            let release_id = transaction
                .query_row(
                    "SELECT id FROM releases WHERE musicbrainz_release_id = ?1",
                    [&release.id],
                    |row| row.get::<_, i64>(0),
                )
                .map_err(DatabaseError::Sqlite)?;
            transaction
                .execute(
                    "INSERT OR IGNORE INTO recording_releases(recording_id, release_id)
                 VALUES (?1, ?2)",
                    rusqlite::params![candidate.recording_id, release_id],
                )
                .map_err(DatabaseError::Sqlite)?;
            Some(release_id)
        } else {
            None
        };
        let context = match method {
            MetadataResolutionMethod::RecordingMbid => "exact MusicBrainz recording MBID lookup",
            MetadataResolutionMethod::Isrc => "unique MusicBrainz recording for normalized ISRC",
        };
        let mut observations = 0_u64;
        observations += upsert_metadata_selection(
            &transaction,
            candidate.recording_id,
            "title",
            &recording.title,
            &recording.id,
            context,
        )?;
        let artist_credit = recording
            .artist_credit
            .iter()
            .map(|credit| format!("{}{}", credit.credited_name, credit.join_phrase))
            .collect::<String>();
        if !artist_credit.is_empty() {
            observations += upsert_metadata_selection(
                &transaction,
                candidate.recording_id,
                "artist_credit",
                &artist_credit,
                &recording.id,
                context,
            )?;
        }
        if let Some(release) = selected_release {
            observations += upsert_metadata_selection(
                &transaction,
                candidate.recording_id,
                "release",
                &release.title,
                &release.id,
                context,
            )?;
            if let Some(date) = &release.date {
                observations += upsert_metadata_selection(
                    &transaction,
                    candidate.recording_id,
                    "release_date",
                    date,
                    &release.id,
                    context,
                )?;
            }
        }
        transaction
            .execute(
                "INSERT INTO metadata_resolutions(recording_id, state, method,
                 candidate_count, message, raw_response_json)
             VALUES (?1, 'resolved', ?2, 1, 'canonical metadata resolved', ?3)
             ON CONFLICT(recording_id) DO UPDATE SET state = 'resolved',
                 method = excluded.method, candidate_count = 1,
                 message = excluded.message, raw_response_json = excluded.raw_response_json,
                 updated_at = CURRENT_TIMESTAMP",
                rusqlite::params![candidate.recording_id, method.as_str(), raw_response_json],
            )
            .map_err(DatabaseError::Sqlite)?;
        transaction.commit().map_err(DatabaseError::Sqlite)?;
        Ok(MetadataPersistenceSummary {
            artists: recording.artist_credit.len() as u64,
            release_selected: release_id.is_some(),
            observations_selected: observations,
        })
    }

    /// Loads canonically selected releases whose artwork is unresolved or deferred.
    pub fn artwork_resolution_candidates(
        &self,
        limit: usize,
    ) -> Result<Vec<ArtworkResolutionCandidate>, DatabaseError> {
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        let mut statement = self
            .connection
            .prepare(
                "SELECT DISTINCT releases.id, releases.musicbrainz_release_id
                 FROM releases
                 JOIN metadata_observations ON metadata_observations.source_entity_id =
                      releases.musicbrainz_release_id AND metadata_observations.field = 'release'
                 JOIN metadata_selections ON metadata_selections.observation_id =
                      metadata_observations.id
                 LEFT JOIN artwork_resolutions ON artwork_resolutions.release_id = releases.id
                 WHERE artwork_resolutions.release_id IS NULL
                    OR artwork_resolutions.state = 'deferred'
                 ORDER BY releases.id LIMIT ?1",
            )
            .map_err(DatabaseError::Sqlite)?;
        statement
            .query_map([limit], |row| {
                Ok(ArtworkResolutionCandidate {
                    release_id: row.get(0)?,
                    musicbrainz_release_id: row.get(1)?,
                })
            })
            .map_err(DatabaseError::Sqlite)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(DatabaseError::Sqlite)
    }

    /// Records a release with no artwork or a retryable provider failure.
    pub fn record_artwork_resolution_state(
        &mut self,
        release_id: i64,
        state: ArtworkResolutionState,
        message: &str,
        raw_response_json: &str,
    ) -> Result<(), DatabaseError> {
        if state == ArtworkResolutionState::Resolved {
            return Err(DatabaseError::InvalidArtworkResolutionTransition);
        }
        self.connection
            .execute(
                "INSERT INTO artwork_resolutions(release_id, state, message, raw_response_json)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(release_id) DO UPDATE SET state = excluded.state,
                     message = excluded.message, raw_response_json = excluded.raw_response_json,
                     updated_at = CURRENT_TIMESTAMP",
                rusqlite::params![release_id, state.as_str(), message, raw_response_json],
            )
            .map_err(DatabaseError::Sqlite)?;
        Ok(())
    }

    /// Selects one validated immutable artwork blob for a canonical release.
    pub fn record_resolved_artwork(
        &mut self,
        candidate: &ArtworkResolutionCandidate,
        artwork: &ResolvedArtwork,
        blob: &ArtworkBlob,
        raw_response_json: &str,
    ) -> Result<bool, DatabaseError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(DatabaseError::Sqlite)?;
        transaction
            .execute(
                "INSERT OR IGNORE INTO artwork_blobs(sha256, relative_path, mime_type, byte_count)
                 VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![
                    blob.sha256,
                    blob.relative_path,
                    blob.mime_type,
                    blob.byte_count
                ],
            )
            .map_err(DatabaseError::Sqlite)?;
        let blob_id = transaction
            .query_row(
                "SELECT id FROM artwork_blobs WHERE sha256 = ?1",
                [&blob.sha256],
                |row| row.get::<_, i64>(0),
            )
            .map_err(DatabaseError::Sqlite)?;
        let changed = transaction
            .execute(
                "INSERT OR IGNORE INTO release_artwork(release_id, blob_id, source,
                 source_image_id, source_url, role, approved)
                 VALUES (?1, ?2, 'cover_art_archive', ?3, ?4, ?5, ?6)",
                rusqlite::params![
                    candidate.release_id,
                    blob_id,
                    artwork.source_image_id,
                    artwork.source_url,
                    artwork.role,
                    artwork.approved
                ],
            )
            .map_err(DatabaseError::Sqlite)?
            > 0;
        transaction
            .execute(
                "INSERT INTO artwork_resolutions(release_id, state, message, raw_response_json)
                 VALUES (?1, 'resolved', 'release artwork cached and selected', ?2)
                 ON CONFLICT(release_id) DO UPDATE SET state = 'resolved',
                     message = excluded.message, raw_response_json = excluded.raw_response_json,
                     updated_at = CURRENT_TIMESTAMP",
                rusqlite::params![candidate.release_id, raw_response_json],
            )
            .map_err(DatabaseError::Sqlite)?;
        transaction.commit().map_err(DatabaseError::Sqlite)?;
        Ok(changed)
    }

    /// Loads managed healthy recordings needing lyrics resolution or sidecar completion.
    pub fn lyrics_work_candidates(
        &self,
        limit: usize,
    ) -> Result<Vec<LyricsWorkCandidate>, DatabaseError> {
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        let mut statement = self
            .connection
            .prepare(
                "SELECT recordings.id, artifacts.id, artifacts.path, artifacts.duration_ms,
                    title.value, artist.value, release.value,
                    lyrics_observations.id, lyrics_observations.kind,
                    lyrics_observations.content, lyrics_outputs.state
             FROM recordings
             JOIN artifacts ON artifacts.id = recordings.preferred_artifact_id
             JOIN metadata_selections title_selection ON title_selection.recording_id =
                  recordings.id AND title_selection.field = 'title'
             JOIN metadata_observations title ON title.id = title_selection.observation_id
             JOIN metadata_selections artist_selection ON artist_selection.recording_id =
                  recordings.id AND artist_selection.field = 'artist_credit'
             JOIN metadata_observations artist ON artist.id = artist_selection.observation_id
             JOIN metadata_selections release_selection ON release_selection.recording_id =
                  recordings.id AND release_selection.field = 'release'
             JOIN metadata_observations release ON release.id = release_selection.observation_id
             LEFT JOIN lyrics_selections ON lyrics_selections.recording_id = recordings.id
             LEFT JOIN lyrics_observations ON lyrics_observations.id =
                  lyrics_selections.observation_id
             LEFT JOIN lyrics_resolutions ON lyrics_resolutions.recording_id = recordings.id
             LEFT JOIN lyrics_outputs ON lyrics_outputs.recording_id = recordings.id
             WHERE artifacts.health = 'healthy' AND artifacts.duration_ms > 0
               AND (EXISTS (SELECT 1 FROM acquisition_commits
                            WHERE acquisition_commits.final_path = artifacts.path
                              AND acquisition_commits.status = 'committed')
                    OR EXISTS (SELECT 1 FROM repair_commits
                               WHERE repair_commits.final_path = artifacts.path
                                 AND repair_commits.committed_at IS NOT NULL))
               AND (lyrics_outputs.recording_id IS NULL OR lyrics_outputs.state = 'prepared')
               AND (lyrics_selections.recording_id IS NOT NULL
                    OR lyrics_resolutions.recording_id IS NULL
                    OR lyrics_resolutions.state = 'deferred')
             ORDER BY recordings.id LIMIT ?1",
            )
            .map_err(DatabaseError::Sqlite)?;
        statement
            .query_map([limit], |row| {
                Ok(LyricsWorkCandidate {
                    recording_id: row.get(0)?,
                    artifact_id: row.get(1)?,
                    artifact_path: PathBuf::from(row.get::<_, String>(2)?),
                    duration_ms: row.get(3)?,
                    title: row.get(4)?,
                    artist_credit: row.get(5)?,
                    release_title: row.get(6)?,
                    observation_id: row.get(7)?,
                    selected_kind: row.get(8)?,
                    selected_content: row.get(9)?,
                    output_prepared: row.get::<_, Option<String>>(10)?.as_deref()
                        == Some("prepared"),
                })
            })
            .map_err(DatabaseError::Sqlite)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(DatabaseError::Sqlite)
    }

    /// Records unavailable, instrumental, or retryable lyrics state without a sidecar.
    pub fn record_lyrics_resolution_state(
        &mut self,
        recording_id: i64,
        state: LyricsResolutionState,
        message: &str,
        raw_response_json: &str,
    ) -> Result<(), DatabaseError> {
        if state == LyricsResolutionState::Resolved {
            return Err(DatabaseError::InvalidLyricsResolutionTransition);
        }
        self.connection
            .execute(
                "INSERT INTO lyrics_resolutions(recording_id, state, message, raw_response_json)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(recording_id) DO UPDATE SET state = excluded.state,
                 message = excluded.message, raw_response_json = excluded.raw_response_json,
                 updated_at = CURRENT_TIMESTAMP",
                rusqlite::params![recording_id, state.as_str(), message, raw_response_json],
            )
            .map_err(DatabaseError::Sqlite)?;
        Ok(())
    }

    /// Persists one validated lyrics observation and explicit selection atomically.
    pub fn record_resolved_lyrics(
        &mut self,
        recording_id: i64,
        lyrics: &ResolvedLyrics,
    ) -> Result<i64, DatabaseError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(DatabaseError::Sqlite)?;
        transaction
            .execute(
                "INSERT OR IGNORE INTO lyrics_observations(recording_id, provider,
             provider_entity_id, kind, content, signature_json, raw_response_json)
             VALUES (?1, 'lrclib', ?2, ?3, ?4, ?5, ?6)",
                rusqlite::params![
                    recording_id,
                    lyrics.provider_entity_id,
                    lyrics.kind,
                    lyrics.content,
                    lyrics.signature_json,
                    lyrics.raw_response_json
                ],
            )
            .map_err(DatabaseError::Sqlite)?;
        let observation_id = transaction
            .query_row(
                "SELECT id FROM lyrics_observations WHERE recording_id = ?1 AND provider =
             'lrclib' AND provider_entity_id = ?2 AND kind = ?3",
                rusqlite::params![recording_id, lyrics.provider_entity_id, lyrics.kind],
                |row| row.get::<_, i64>(0),
            )
            .map_err(DatabaseError::Sqlite)?;
        transaction
            .execute(
                "INSERT INTO lyrics_selections(recording_id, observation_id) VALUES (?1, ?2)
             ON CONFLICT(recording_id) DO UPDATE SET observation_id = excluded.observation_id,
                 selected_at = CURRENT_TIMESTAMP",
                rusqlite::params![recording_id, observation_id],
            )
            .map_err(DatabaseError::Sqlite)?;
        transaction
            .execute(
                "INSERT INTO lyrics_resolutions(recording_id, state, message, raw_response_json)
             VALUES (?1, 'resolved', 'lyrics validated and selected', ?2)
             ON CONFLICT(recording_id) DO UPDATE SET state = 'resolved',
                 message = excluded.message, raw_response_json = excluded.raw_response_json,
                 updated_at = CURRENT_TIMESTAMP",
                rusqlite::params![recording_id, lyrics.raw_response_json],
            )
            .map_err(DatabaseError::Sqlite)?;
        transaction.commit().map_err(DatabaseError::Sqlite)?;
        Ok(observation_id)
    }

    /// Persists no-clobber sidecar intent before the filesystem effect.
    pub fn prepare_lyrics_output(
        &mut self,
        work: &LyricsWorkCandidate,
        observation_id: i64,
        path: &Path,
        sha256: &str,
        byte_count: i64,
    ) -> Result<bool, DatabaseError> {
        let path = path
            .to_str()
            .ok_or_else(|| DatabaseError::NonUnicodePath(path.into()))?;
        let changed = self
            .connection
            .execute(
                "INSERT OR IGNORE INTO lyrics_outputs(recording_id, observation_id, artifact_id,
             path, sha256, byte_count, state) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'prepared')",
                rusqlite::params![
                    work.recording_id,
                    observation_id,
                    work.artifact_id,
                    path,
                    sha256,
                    byte_count
                ],
            )
            .map_err(DatabaseError::Sqlite)?
            > 0;
        if !changed {
            let matches = self
                .connection
                .query_row(
                    "SELECT observation_id = ?2 AND artifact_id = ?3 AND path = ?4 AND
                        sha256 = ?5 AND byte_count = ?6
                 FROM lyrics_outputs WHERE recording_id = ?1",
                    rusqlite::params![
                        work.recording_id,
                        observation_id,
                        work.artifact_id,
                        path,
                        sha256,
                        byte_count
                    ],
                    |row| row.get::<_, bool>(0),
                )
                .map_err(DatabaseError::Sqlite)?;
            if !matches {
                return Err(DatabaseError::LyricsOutputMismatch(work.recording_id));
            }
        }
        Ok(changed)
    }

    /// Marks a matching prepared sidecar as committed after exact-byte verification.
    pub fn commit_lyrics_output(&mut self, recording_id: i64) -> Result<(), DatabaseError> {
        let changed = self
            .connection
            .execute(
                "UPDATE lyrics_outputs SET state = 'committed', committed_at = CURRENT_TIMESTAMP
             WHERE recording_id = ?1 AND state = 'prepared'",
                [recording_id],
            )
            .map_err(DatabaseError::Sqlite)?;
        if changed == 0 {
            let committed = self
                .connection
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM lyrics_outputs WHERE recording_id = ?1
                 AND state = 'committed')",
                    [recording_id],
                    |row| row.get::<_, bool>(0),
                )
                .map_err(DatabaseError::Sqlite)?;
            if !committed {
                return Err(DatabaseError::LyricsOutputNotPrepared(recording_id));
            }
        }
        Ok(())
    }

    /// Loads canonical managed artifacts eligible for source-preserving tag materialization.
    pub fn metadata_materialization_candidates(
        &self,
        limit: usize,
    ) -> Result<Vec<MetadataMaterializationCandidate>, DatabaseError> {
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        let mut statement = self.connection.prepare(
            "SELECT recordings.id, artifacts.id, artifacts.path, artifacts.sha256,
                    artifacts.codec, artifacts.duration_ms, artifacts.sample_rate_hz,
                    artifacts.channels, title.value, artist.value, release.value,
                    date.value, recordings.musicbrainz_recording_id, recordings.isrc,
                    metadata_materializations.state, artwork_blobs.relative_path,
                    artwork_blobs.mime_type, metadata_materialization_staging.recording_id
             FROM recordings
             JOIN artifacts ON artifacts.id = recordings.preferred_artifact_id
             JOIN metadata_selections ts ON ts.recording_id=recordings.id AND ts.field='title'
             JOIN metadata_observations title ON title.id=ts.observation_id
             JOIN metadata_selections ars ON ars.recording_id=recordings.id AND ars.field='artist_credit'
             JOIN metadata_observations artist ON artist.id=ars.observation_id
             JOIN metadata_selections rs ON rs.recording_id=recordings.id AND rs.field='release'
             JOIN metadata_observations release ON release.id=rs.observation_id
             LEFT JOIN metadata_selections ds ON ds.recording_id=recordings.id AND ds.field='release_date'
             LEFT JOIN metadata_observations date ON date.id=ds.observation_id
             LEFT JOIN metadata_materializations ON metadata_materializations.recording_id=recordings.id
             LEFT JOIN metadata_materialization_states state ON state.recording_id=recordings.id
             LEFT JOIN releases selected_release ON selected_release.musicbrainz_release_id=release.source_entity_id
             LEFT JOIN release_artwork ON release_artwork.release_id=selected_release.id
             LEFT JOIN artwork_blobs ON artwork_blobs.id=release_artwork.blob_id
             LEFT JOIN metadata_materialization_staging ON metadata_materialization_staging.recording_id=recordings.id
             WHERE artifacts.health='healthy' AND artifacts.sha256 IS NOT NULL
               AND (EXISTS (SELECT 1 FROM acquisition_commits WHERE final_path=artifacts.path AND status='committed')
                    OR EXISTS (SELECT 1 FROM repair_commits WHERE final_path=artifacts.path AND committed_at IS NOT NULL))
               AND (metadata_materializations.recording_id IS NULL OR metadata_materializations.state='prepared')
               AND (state.recording_id IS NULL OR state.state='pending')
             ORDER BY recordings.id LIMIT ?1"
        ).map_err(DatabaseError::Sqlite)?;
        statement
            .query_map([limit], |row| {
                Ok(MetadataMaterializationCandidate {
                    recording_id: row.get(0)?,
                    source_artifact_id: row.get(1)?,
                    source_path: PathBuf::from(row.get::<_, String>(2)?),
                    source_sha256: row.get(3)?,
                    codec: row.get(4)?,
                    duration_ms: row.get(5)?,
                    sample_rate_hz: row.get(6)?,
                    channels: row.get(7)?,
                    title: row.get(8)?,
                    artist_credit: row.get(9)?,
                    release_title: row.get(10)?,
                    release_date: row.get(11)?,
                    musicbrainz_recording_id: row.get(12)?,
                    isrc: row.get(13)?,
                    prepared: row.get::<_, Option<String>>(14)?.as_deref() == Some("prepared"),
                    artwork_relative_path: row.get(15)?,
                    artwork_mime_type: row.get(16)?,
                    staging_reserved: row.get::<_, Option<i64>>(17)?.is_some(),
                })
            })
            .map_err(DatabaseError::Sqlite)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(DatabaseError::Sqlite)
    }

    /// Defers one failed materialization until explicit operator retry.
    pub fn defer_metadata_materialization(
        &mut self,
        recording_id: i64,
        message: &str,
    ) -> Result<(), DatabaseError> {
        self.connection.execute(
            "INSERT INTO metadata_materialization_states(recording_id,state,message) VALUES (?1,'deferred',?2)
             ON CONFLICT(recording_id) DO UPDATE SET state='deferred',message=excluded.message,updated_at=CURRENT_TIMESTAMP",
            rusqlite::params![recording_id,message]
        ).map_err(DatabaseError::Sqlite)?;
        Ok(())
    }

    /// Reserves the deterministic hidden staging path before ffmpeg can create it.
    pub fn reserve_metadata_materialization_staging(
        &mut self,
        recording_id: i64,
        path: &Path,
    ) -> Result<bool, DatabaseError> {
        let path = path
            .to_str()
            .ok_or_else(|| DatabaseError::NonUnicodePath(path.into()))?;
        let changed = self.connection.execute(
            "INSERT OR IGNORE INTO metadata_materialization_staging(recording_id,path) VALUES (?1,?2)",
            rusqlite::params![recording_id,path],
        ).map_err(DatabaseError::Sqlite)? > 0;
        if !changed {
            let matches = self
                .connection
                .query_row(
                    "SELECT path=?2 FROM metadata_materialization_staging WHERE recording_id=?1",
                    rusqlite::params![recording_id, path],
                    |row| row.get::<_, bool>(0),
                )
                .map_err(DatabaseError::Sqlite)?;
            if !matches {
                return Err(DatabaseError::MetadataStagingMismatch(recording_id));
            }
        }
        Ok(changed)
    }

    /// Explicitly releases one deferred canonical materialization for retry.
    pub fn retry_metadata_materialization(
        &mut self,
        recording_id: i64,
    ) -> Result<bool, DatabaseError> {
        Ok(self
            .connection
            .execute(
                "UPDATE metadata_materialization_states SET state='pending',
             message='operator explicitly requested retry',updated_at=CURRENT_TIMESTAMP
             WHERE recording_id=?1 AND state='deferred'",
                [recording_id],
            )
            .map_err(DatabaseError::Sqlite)?
            == 1)
    }

    /// Persists complete validated tag-output intent before replacing the visible path.
    pub fn prepare_metadata_materialization(
        &mut self,
        candidate: &MetadataMaterializationCandidate,
        intent: &MetadataMaterializationIntent,
    ) -> Result<bool, DatabaseError> {
        let source_path = candidate
            .source_path
            .to_str()
            .ok_or_else(|| DatabaseError::NonUnicodePath(candidate.source_path.clone()))?
            .to_owned();
        let history_path = intent
            .history_path
            .to_str()
            .ok_or_else(|| DatabaseError::NonUnicodePath(intent.history_path.clone()))?
            .to_owned();
        let staged_path = intent
            .staged_path
            .to_str()
            .ok_or_else(|| DatabaseError::NonUnicodePath(intent.staged_path.clone()))?
            .to_owned();
        let changed = self
            .connection
            .execute(
                "INSERT OR IGNORE INTO metadata_materializations(recording_id,source_artifact_id,
             source_path,source_sha256,history_path,staged_path,final_path,result_sha256,
             result_bytes,codec,duration_ms,sample_rate_hz,channels,canonical_snapshot_json,
             state,message) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,
             'prepared','validated stream-copy output prepared')",
                rusqlite::params![
                    candidate.recording_id,
                    candidate.source_artifact_id,
                    source_path,
                    candidate.source_sha256,
                    history_path,
                    staged_path,
                    candidate
                        .source_path
                        .to_str()
                        .ok_or_else(|| DatabaseError::NonUnicodePath(
                            candidate.source_path.clone()
                        ))?,
                    intent.validated.sha256,
                    i64::try_from(intent.validated.bytes)
                        .map_err(|_| DatabaseError::ArtifactTooLarge(intent.validated.bytes))?,
                    intent.validated.codec,
                    intent.validated.duration_ms,
                    intent.validated.sample_rate_hz,
                    intent.validated.channels,
                    intent.canonical_snapshot_json
                ],
            )
            .map_err(DatabaseError::Sqlite)?
            > 0;
        Ok(changed)
    }

    /// Loads complete prepared evidence for interruption recovery.
    pub fn prepared_metadata_materialization(
        &self,
        recording_id: i64,
    ) -> Result<Option<MetadataMaterializationIntent>, DatabaseError> {
        self.connection
            .query_row(
                "SELECT history_path,staged_path,result_sha256,result_bytes,codec,duration_ms,
                    sample_rate_hz,channels,canonical_snapshot_json
             FROM metadata_materializations WHERE recording_id=?1 AND state='prepared'",
                [recording_id],
                |row| {
                    let bytes = row.get::<_, i64>(3)?;
                    Ok(MetadataMaterializationIntent {
                        history_path: PathBuf::from(row.get::<_, String>(0)?),
                        staged_path: PathBuf::from(row.get::<_, String>(1)?),
                        validated: ValidatedStagedMedia {
                            path: PathBuf::from(row.get::<_, String>(1)?),
                            sha256: row.get(2)?,
                            bytes: u64::try_from(bytes).map_err(|error| {
                                rusqlite::Error::FromSqlConversionFailure(
                                    3,
                                    rusqlite::types::Type::Integer,
                                    Box::new(error),
                                )
                            })?,
                            codec: row.get(4)?,
                            duration_ms: row
                                .get::<_, Option<i64>>(5)?
                                .and_then(|v| u64::try_from(v).ok()),
                            sample_rate_hz: row
                                .get::<_, Option<i64>>(6)?
                                .and_then(|v| u32::try_from(v).ok()),
                            channels: row
                                .get::<_, Option<i64>>(7)?
                                .and_then(|v| u32::try_from(v).ok()),
                            musicbrainz_recording_id: None,
                            isrc: None,
                        },
                        canonical_snapshot_json: row.get(8)?,
                    })
                },
            )
            .optional()
            .map_err(DatabaseError::Sqlite)
    }

    /// Atomically switches durable preference after the filesystem replacement is verified.
    pub fn commit_metadata_materialization(
        &mut self,
        recording_id: i64,
    ) -> Result<ArtifactPersistenceResult, DatabaseError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(DatabaseError::Sqlite)?;
        let row=transaction.query_row(
            "SELECT source_artifact_id,history_path,final_path,result_sha256,result_bytes,
                    codec,duration_ms,sample_rate_hz,channels,result_artifact_id
             FROM metadata_materializations WHERE recording_id=?1 AND state IN ('prepared','committed')",
            [recording_id], |row| Ok((row.get::<_,i64>(0)?,row.get::<_,String>(1)?,row.get::<_,String>(2)?,row.get::<_,String>(3)?,row.get::<_,i64>(4)?,row.get::<_,String>(5)?,row.get::<_,Option<i64>>(6)?,row.get::<_,Option<i64>>(7)?,row.get::<_,Option<i64>>(8)?,row.get::<_,Option<i64>>(9)?))
        ).map_err(DatabaseError::Sqlite)?;
        if let Some(id) = row.9 {
            return Ok(ArtifactPersistenceResult {
                recording_id,
                artifact_id: id,
                inserted: false,
            });
        }
        transaction
            .execute(
                "UPDATE artifacts SET path=?2 WHERE id=?1",
                rusqlite::params![row.0, row.1],
            )
            .map_err(DatabaseError::Sqlite)?;
        transaction.execute(
            "INSERT INTO artifacts(recording_id,path,sha256,duration_ms,codec,sample_rate_hz,channels,health)
             VALUES (?1,?2,?3,?4,?5,?6,?7,'healthy')",
            rusqlite::params![recording_id,row.2,row.3,row.6,row.5,row.7,row.8]
        ).map_err(DatabaseError::Sqlite)?;
        let artifact_id = transaction.last_insert_rowid();
        transaction
            .execute(
                "UPDATE recordings SET preferred_artifact_id=?2 WHERE id=?1",
                rusqlite::params![recording_id, artifact_id],
            )
            .map_err(DatabaseError::Sqlite)?;
        transaction.execute("UPDATE metadata_materializations SET result_artifact_id=?2,state='committed',message='canonical tags committed with source bytes retained',committed_at=CURRENT_TIMESTAMP,updated_at=CURRENT_TIMESTAMP WHERE recording_id=?1",rusqlite::params![recording_id,artifact_id]).map_err(DatabaseError::Sqlite)?;
        transaction
            .execute(
                "DELETE FROM metadata_materialization_states WHERE recording_id=?1",
                [recording_id],
            )
            .map_err(DatabaseError::Sqlite)?;
        transaction
            .execute(
                "DELETE FROM metadata_materialization_staging WHERE recording_id=?1",
                [recording_id],
            )
            .map_err(DatabaseError::Sqlite)?;
        transaction.commit().map_err(DatabaseError::Sqlite)?;
        Ok(ArtifactPersistenceResult {
            recording_id,
            artifact_id,
            inserted: true,
        })
    }

    /// Persists one explainable discovery run and enforces every hard budget transactionally.
    pub fn record_discovery_candidates(
        &mut self,
        provider: &str,
        recommendations: &[Recommendation],
        config: &DiscoveryConfig,
        free_bytes: u64,
        required_free_bytes: u64,
    ) -> Result<DiscoveryPersistenceReport, DatabaseError> {
        let to_i64 =
            |value: u64| i64::try_from(value).map_err(|_| DatabaseError::DiscoveryCountTooLarge);
        let maximum = u64::from(config.max_new_tracks_per_day);
        let target = u64::from(config.target_new_tracks_per_day);
        let per_artist = u64::from(config.max_tracks_per_artist_per_day);
        let (exploration, wildcard) =
            discovery_lane_budgets(maximum, config.exploration_ratio, config.wildcard_ratio);
        let adjacent = maximum.saturating_sub(exploration).saturating_sub(wildcard);
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(DatabaseError::Sqlite)?;
        transaction.execute("INSERT INTO discovery_runs(status,maximum_tracks,per_artist_maximum,exploration_maximum,wildcard_maximum,free_bytes,required_free_bytes) VALUES ('running',?1,?2,?3,?4,?5,?6)",rusqlite::params![to_i64(maximum)?,to_i64(per_artist)?,to_i64(exploration)?,to_i64(wildcard)?,to_i64(free_bytes)?,to_i64(required_free_bytes)?]).map_err(DatabaseError::Sqlite)?;
        let run_id = transaction.last_insert_rowid();
        let count = |sql: &str| {
            transaction
                .query_row(sql, [], |row| row.get::<_, u64>(0))
                .map_err(DatabaseError::Sqlite)
        };
        let mut daily = count(
            "SELECT COUNT(*) FROM discovery_candidates WHERE date(created_at)=date('now') AND state IN ('approved','queued','acquired')",
        )?;
        let mut lane_adjacent = count(
            "SELECT COUNT(*) FROM discovery_candidates WHERE date(created_at)=date('now') AND lane='adjacent' AND state IN ('approved','queued','acquired')",
        )?;
        let mut lane_exploration = count(
            "SELECT COUNT(*) FROM discovery_candidates WHERE date(created_at)=date('now') AND lane='exploration' AND state IN ('approved','queued','acquired')",
        )?;
        let mut lane_wildcard = count(
            "SELECT COUNT(*) FROM discovery_candidates WHERE date(created_at)=date('now') AND lane='wildcard' AND state IN ('approved','queued','acquired')",
        )?;
        let mut report = DiscoveryPersistenceReport {
            run_id,
            ..Default::default()
        };
        for recommendation in recommendations {
            let existing=transaction.query_row("SELECT EXISTS(SELECT 1 FROM recordings WHERE musicbrainz_recording_id=?1) OR EXISTS(SELECT 1 FROM discovery_candidates WHERE musicbrainz_recording_id=?1 AND state IN ('approved','queued','acquired'))",[&recommendation.recording_mbid],|row|row.get::<_,bool>(0)).map_err(DatabaseError::Sqlite)?;
            let artist_today = if let Some(artist) = &recommendation.artist_mbid {
                transaction.query_row("SELECT COUNT(*) FROM discovery_candidates WHERE date(created_at)=date('now') AND musicbrainz_artist_id=?1 AND state IN ('approved','queued','acquired')",[artist],|row|row.get::<_,u64>(0)).map_err(DatabaseError::Sqlite)?
            } else {
                transaction.query_row("SELECT COUNT(*) FROM discovery_candidates WHERE date(created_at)=date('now') AND musicbrainz_artist_id IS NULL AND state IN ('approved','queued','acquired')",[],|row|row.get::<_,u64>(0)).map_err(DatabaseError::Sqlite)?
            };
            let lane_used = match recommendation.lane {
                DiscoveryLane::Adjacent => lane_adjacent,
                DiscoveryLane::Exploration => lane_exploration,
                DiscoveryLane::Wildcard => lane_wildcard,
            };
            let lane_max = match recommendation.lane {
                DiscoveryLane::Adjacent => adjacent,
                DiscoveryLane::Exploration => exploration,
                DiscoveryLane::Wildcard => wildcard,
            };
            let (state, reason) = if existing {
                ("duplicate", "recording MBID already exists or is queued")
            } else if free_bytes < required_free_bytes {
                ("budget_rejected", "minimum free storage guard failed")
            } else if daily >= maximum || daily >= target {
                ("budget_rejected", "daily target or hard maximum reached")
            } else if artist_today >= per_artist {
                ("budget_rejected", "daily per-artist maximum reached")
            } else if lane_used >= lane_max {
                ("budget_rejected", "daily exploration-lane maximum reached")
            } else {
                (
                    "approved",
                    "all canonical deduplication and safety budgets passed",
                )
            };
            let explanation = serde_json::json!({"formula":"provider_score*0.7 + seed_weight*0.3","provider_score_millionths":recommendation.provider_score_millionths,"seed_weight_millionths":recommendation.seed_weight_millionths,"reasons":recommendation.reasons});
            transaction.execute("INSERT INTO discovery_candidates(run_id,musicbrainz_recording_id,musicbrainz_artist_id,lane,provider,provider_score_millionths,taste_score_millionths,explanation_json,state,decision_reason) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",rusqlite::params![run_id,recommendation.recording_mbid,recommendation.artist_mbid,recommendation.lane.as_str(),provider,recommendation.provider_score_millionths,discovery_score(recommendation),explanation.to_string(),state,reason]).map_err(DatabaseError::Sqlite)?;
            match state {
                "approved" => {
                    report.approved += 1;
                    daily += 1;
                    match recommendation.lane {
                        DiscoveryLane::Adjacent => lane_adjacent += 1,
                        DiscoveryLane::Exploration => lane_exploration += 1,
                        DiscoveryLane::Wildcard => lane_wildcard += 1,
                    }
                }
                "duplicate" => report.duplicates += 1,
                _ => report.budget_rejected += 1,
            }
        }
        transaction.execute("UPDATE discovery_runs SET status='succeeded',finished_at=CURRENT_TIMESTAMP WHERE id=?1",[run_id]).map_err(DatabaseError::Sqlite)?;
        transaction.commit().map_err(DatabaseError::Sqlite)?;
        Ok(report)
    }

    /// Replaces active Navidrome favorite seeds using exact local recording MBIDs.
    pub fn replace_navidrome_seeds(
        &mut self,
        recording_mbids: &[String],
    ) -> Result<DiscoverySeedReport, DatabaseError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(DatabaseError::Sqlite)?;
        transaction
            .execute(
                "UPDATE discovery_seeds SET active=0,updated_at=CURRENT_TIMESTAMP WHERE origin='navidrome_favorite'",
                [],
            )
            .map_err(DatabaseError::Sqlite)?;
        let mut report = DiscoverySeedReport::default();
        let unique = recording_mbids.iter().collect::<BTreeSet<_>>();
        for mbid in unique {
            let recording_id = transaction
                .query_row(
                    "SELECT id FROM recordings WHERE musicbrainz_recording_id=?1",
                    [mbid],
                    |row| row.get::<_, i64>(0),
                )
                .optional()
                .map_err(DatabaseError::Sqlite)?;
            if let Some(recording_id) = recording_id {
                transaction.execute("INSERT INTO discovery_seeds(recording_id,origin,weight_millionths,active,updated_at) VALUES (?1,'navidrome_favorite',1000000,1,CURRENT_TIMESTAMP) ON CONFLICT(recording_id) DO UPDATE SET origin='navidrome_favorite',weight_millionths=1000000,active=1,updated_at=CURRENT_TIMESTAMP", [recording_id]).map_err(DatabaseError::Sqlite)?;
                report.matched += 1;
            } else {
                report.unmatched += 1;
            }
        }
        transaction.commit().map_err(DatabaseError::Sqlite)?;
        Ok(report)
    }

    /// Loads approved discovery candidates in stable order for evidence routing.
    pub fn approved_discovery_candidates(
        &self,
        limit: usize,
    ) -> Result<Vec<ApprovedDiscoveryCandidate>, DatabaseError> {
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        let mut statement = self.connection.prepare(
            "SELECT id,musicbrainz_recording_id FROM discovery_candidates WHERE state='approved' ORDER BY id LIMIT ?1",
        ).map_err(DatabaseError::Sqlite)?;
        statement
            .query_map([limit], |row| {
                Ok(ApprovedDiscoveryCandidate {
                    id: row.get(0)?,
                    recording_mbid: row.get(1)?,
                })
            })
            .map_err(DatabaseError::Sqlite)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(DatabaseError::Sqlite)
    }

    /// Atomically creates a discovery-owned provider item and pending acquisition assertion.
    pub fn queue_discovery_acquisition(
        &mut self,
        candidate: &ApprovedDiscoveryCandidate,
        route: &DiscoveryAcquisitionRoute,
    ) -> Result<i64, DatabaseError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(DatabaseError::Sqlite)?;
        let state = transaction
            .query_row(
                "SELECT state FROM discovery_candidates WHERE id=?1",
                [candidate.id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(DatabaseError::Sqlite)?;
        if state.as_deref() != Some("approved") {
            return Err(DatabaseError::DiscoveryCandidateNotApproved(candidate.id));
        }
        transaction
            .execute(
                "INSERT OR IGNORE INTO recordings(musicbrainz_recording_id,isrc) VALUES (?1,?2)",
                rusqlite::params![candidate.recording_mbid, route.canonical_isrc],
            )
            .map_err(DatabaseError::Sqlite)?;
        let recording_id = transaction
            .query_row(
                "SELECT id FROM recordings WHERE musicbrainz_recording_id=?1",
                [&candidate.recording_mbid],
                |row| row.get::<_, i64>(0),
            )
            .map_err(DatabaseError::Sqlite)?;
        transaction.execute("INSERT INTO provider_items(provider,provider_item_id,original_url,source_metadata_json,availability,recording_id) VALUES ('youtube',?1,?2,?3,'available',?4) ON CONFLICT(provider,provider_item_id) DO NOTHING",rusqlite::params![route.provider_item_id,route.url,route.assertion_json,recording_id]).map_err(DatabaseError::Sqlite)?;
        let provider_item_id=transaction.query_row("SELECT id,recording_id FROM provider_items WHERE provider='youtube' AND provider_item_id=?1",[&route.provider_item_id],|row|Ok((row.get::<_,i64>(0)?,row.get::<_,Option<i64>>(1)?))).map_err(DatabaseError::Sqlite)?;
        if provider_item_id.1 != Some(recording_id) {
            return Err(DatabaseError::ProviderItemRecordingConflict);
        }
        transaction
            .execute("INSERT INTO sync_runs(status) VALUES ('succeeded')", [])
            .map_err(DatabaseError::Sqlite)?;
        let run_id = transaction.last_insert_rowid();
        let key = format!("acquire:youtube:{}", route.provider_item_id);
        transaction.execute("INSERT OR IGNORE INTO jobs(run_id,kind,status,idempotency_key) VALUES (?1,'acquire','pending',?2)",rusqlite::params![run_id,key]).map_err(DatabaseError::Sqlite)?;
        let job_id = transaction
            .query_row(
                "SELECT id FROM jobs WHERE idempotency_key=?1",
                [key],
                |row| row.get::<_, i64>(0),
            )
            .map_err(DatabaseError::Sqlite)?;
        transaction
            .execute(
                "INSERT OR IGNORE INTO acquisition_jobs(job_id,provider_item_id) VALUES (?1,?2)",
                rusqlite::params![job_id, provider_item_id.0],
            )
            .map_err(DatabaseError::Sqlite)?;
        transaction.execute("INSERT INTO discovery_acquisition_assertions(candidate_id,provider_item_id,recording_id,relationship_source,canonical_duration_ms,canonical_isrc,assertion_json) VALUES (?1,?2,?3,'musicbrainz_recording_url',?4,?5,?6)",rusqlite::params![candidate.id,provider_item_id.0,recording_id,route.canonical_duration_ms,route.canonical_isrc,route.assertion_json]).map_err(DatabaseError::Sqlite)?;
        transaction.execute("UPDATE discovery_candidates SET state='queued',decision_reason='canonical MusicBrainz recording URL relationship verified',updated_at=CURRENT_TIMESTAMP WHERE id=?1",[candidate.id]).map_err(DatabaseError::Sqlite)?;
        transaction.commit().map_err(DatabaseError::Sqlite)?;
        Ok(job_id)
    }

    /// Marks an approved candidate unresolved without creating provider work.
    pub fn mark_discovery_unresolved(
        &mut self,
        candidate_id: i64,
        reason: &str,
    ) -> Result<bool, DatabaseError> {
        Ok(self.connection.execute("UPDATE discovery_candidates SET state='unresolved',decision_reason=?2,updated_at=CURRENT_TIMESTAMP WHERE id=?1 AND state='approved'",rusqlite::params![candidate_id,reason]).map_err(DatabaseError::Sqlite)?==1)
    }

    /// Loads active unhealthy original items in stable order for availability checks.
    pub fn repair_original_candidates(
        &self,
        limit: usize,
    ) -> Result<Vec<RepairOriginalCandidate>, DatabaseError> {
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        let mut statement = self
            .connection
            .prepare(
                "SELECT DISTINCT provider_items.id, provider_items.provider,
                        provider_items.provider_item_id, provider_items.original_url,
                        provider_items.availability, artifacts.id, artifacts.health
                 FROM provider_items
                 JOIN collection_memberships ON
                      collection_memberships.provider_item_id = provider_items.id
                     AND collection_memberships.active = 1
                 JOIN recordings ON recordings.id = provider_items.recording_id
                 JOIN artifacts ON artifacts.id = recordings.preferred_artifact_id
                 WHERE artifacts.health IN ('missing', 'corrupt')
                 ORDER BY provider_items.id LIMIT ?1",
            )
            .map_err(DatabaseError::Sqlite)?;
        let rows = statement
            .query_map([limit], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, i64>(5)?,
                    row.get::<_, String>(6)?,
                ))
            })
            .map_err(DatabaseError::Sqlite)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(DatabaseError::Sqlite)?;
        rows.into_iter()
            .map(|row| {
                Ok(RepairOriginalCandidate {
                    provider_item_id: row.0,
                    provider: row.1,
                    provider_owned_id: row.2,
                    original_url: row.3,
                    availability: ProviderAvailability::parse(&row.4)?,
                    artifact_id: row.5,
                    artifact_health: ArtifactHealth::parse(&row.6)?,
                })
            })
            .collect()
    }

    /// Records a classified original-provider observation without inferring permanence.
    pub fn record_provider_availability(
        &mut self,
        provider_item_id: i64,
        availability: ProviderAvailability,
        message: &str,
    ) -> Result<bool, DatabaseError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(DatabaseError::Sqlite)?;
        let prior = transaction
            .query_row(
                "SELECT availability FROM provider_items WHERE id = ?1",
                [provider_item_id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(DatabaseError::Sqlite)?
            .ok_or(DatabaseError::ProviderItemNotFound(provider_item_id))?;
        if prior == ProviderAvailability::PermanentlyUnavailable.as_str()
            && availability == ProviderAvailability::TransientFailure
        {
            return Ok(false);
        }
        if prior == availability.as_str() {
            return Ok(false);
        }
        transaction
            .execute(
                "UPDATE provider_items SET availability = ?2 WHERE id = ?1",
                rusqlite::params![provider_item_id, availability.as_str()],
            )
            .map_err(DatabaseError::Sqlite)?;
        transaction
            .execute(
                "INSERT INTO events(level, component, event, message, context_json)
                 VALUES (?1, 'repair', 'provider_availability_observed', ?2,
                         json_object('provider_item_id', ?3, 'availability', ?4))",
                rusqlite::params![
                    if availability == ProviderAvailability::PermanentlyUnavailable {
                        "warning"
                    } else {
                        "info"
                    },
                    message,
                    provider_item_id,
                    availability.as_str()
                ],
            )
            .map_err(DatabaseError::Sqlite)?;
        transaction.commit().map_err(DatabaseError::Sqlite)?;
        Ok(true)
    }

    /// Reconciles durable repair eligibility from conservative persisted evidence.
    pub fn reconcile_repair_eligibility(
        &mut self,
        limit: usize,
    ) -> Result<RepairEligibilitySummary, DatabaseError> {
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(DatabaseError::Sqlite)?;
        let cancelled = transaction
            .execute(
                "UPDATE repair_cases SET state = 'cancelled', updated_at = CURRENT_TIMESTAMP
                 WHERE state IN ('eligible', 'unresolved') AND NOT EXISTS (
                     SELECT 1 FROM provider_items
                     JOIN collection_memberships ON
                          collection_memberships.provider_item_id = provider_items.id
                         AND collection_memberships.active = 1
                     JOIN recordings ON recordings.id = provider_items.recording_id
                     JOIN artifacts ON artifacts.id = repair_cases.reference_artifact_id
                     JOIN artifact_fingerprints ON
                          artifact_fingerprints.artifact_id = artifacts.id
                     WHERE provider_items.id = repair_cases.original_provider_item_id
                       AND provider_items.availability = 'permanently_unavailable'
                       AND artifacts.health IN ('missing', 'corrupt')
                 )",
                [],
            )
            .map_err(DatabaseError::Sqlite)?;
        let eligible_ids = {
            let mut statement = transaction
                .prepare(
                    "SELECT DISTINCT provider_items.id, recordings.id, artifacts.id
                     FROM provider_items
                     JOIN collection_memberships ON
                          collection_memberships.provider_item_id = provider_items.id
                         AND collection_memberships.active = 1
                     JOIN recordings ON recordings.id = provider_items.recording_id
                     JOIN artifacts ON artifacts.id = recordings.preferred_artifact_id
                     JOIN artifact_fingerprints ON
                          artifact_fingerprints.artifact_id = artifacts.id
                     WHERE provider_items.availability = 'permanently_unavailable'
                       AND artifacts.health IN ('missing', 'corrupt')
                     ORDER BY provider_items.id LIMIT ?1",
                )
                .map_err(DatabaseError::Sqlite)?;
            statement
                .query_map([limit], |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, i64>(2)?,
                    ))
                })
                .map_err(DatabaseError::Sqlite)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(DatabaseError::Sqlite)?
        };
        let mut inserted = 0_u64;
        let mut reopened = 0_u64;
        let mut unchanged = 0_u64;
        for (provider_item_id, recording_id, artifact_id) in eligible_ids {
            let prior = transaction
                .query_row(
                    "SELECT state FROM repair_cases WHERE original_provider_item_id = ?1",
                    [provider_item_id],
                    |row| row.get::<_, String>(0),
                )
                .optional()
                .map_err(DatabaseError::Sqlite)?;
            match prior.as_deref() {
                None => {
                    transaction
                        .execute(
                            "INSERT INTO repair_cases(recording_id, original_provider_item_id,
                             reference_artifact_id) VALUES (?1, ?2, ?3)",
                            rusqlite::params![recording_id, provider_item_id, artifact_id],
                        )
                        .map_err(DatabaseError::Sqlite)?;
                    inserted += 1;
                }
                Some("cancelled") => {
                    transaction
                        .execute(
                            "UPDATE repair_cases SET recording_id = ?2, reference_artifact_id = ?3,
                             state = 'eligible', updated_at = CURRENT_TIMESTAMP
                         WHERE original_provider_item_id = ?1",
                            rusqlite::params![provider_item_id, recording_id, artifact_id],
                        )
                        .map_err(DatabaseError::Sqlite)?;
                    reopened += 1;
                }
                Some(_) => unchanged += 1,
            }
        }
        transaction.commit().map_err(DatabaseError::Sqlite)?;
        Ok(RepairEligibilitySummary {
            inserted,
            reopened,
            unchanged,
            cancelled: cancelled as u64,
        })
    }

    /// Loads bounded eligible repair cases with retained reference evidence.
    pub fn eligible_repair_cases(
        &self,
        limit: usize,
    ) -> Result<Vec<EligibleRepairCase>, DatabaseError> {
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        let mut statement = self
            .connection
            .prepare(
                "SELECT repair_cases.id, repair_cases.recording_id,
                    repair_cases.original_provider_item_id, provider_items.source_title,
                    recordings.musicbrainz_recording_id, recordings.isrc,
                    artifacts.duration_ms, artifact_fingerprints.max_seconds,
                    artifact_fingerprints.duration_ms,
                    artifact_fingerprints.fingerprint_json
             FROM repair_cases
             JOIN provider_items ON provider_items.id = repair_cases.original_provider_item_id
             JOIN recordings ON recordings.id = repair_cases.recording_id
             JOIN artifacts ON artifacts.id = repair_cases.reference_artifact_id
             JOIN artifact_fingerprints ON
                  artifact_fingerprints.artifact_id = repair_cases.reference_artifact_id
             WHERE repair_cases.state = 'eligible'
             ORDER BY repair_cases.id LIMIT ?1",
            )
            .map_err(DatabaseError::Sqlite)?;
        statement
            .query_map([limit], |row| {
                Ok(EligibleRepairCase {
                    case_id: row.get(0)?,
                    recording_id: row.get(1)?,
                    original_provider_item_id: row.get(2)?,
                    source_title: row.get(3)?,
                    musicbrainz_recording_id: row.get(4)?,
                    isrc: row.get(5)?,
                    reference_duration_ms: row.get(6)?,
                    fingerprint_max_seconds: row.get(7)?,
                    fingerprint_duration_ms: row.get(8)?,
                    fingerprint_json: row.get(9)?,
                })
            })
            .map_err(DatabaseError::Sqlite)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(DatabaseError::Sqlite)
    }

    /// Stores search-generated candidates as unverified audit evidence.
    pub fn record_repair_candidates(
        &mut self,
        case_id: i64,
        candidates: &[crate::provider::ProviderItem],
    ) -> Result<u64, DatabaseError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(DatabaseError::Sqlite)?;
        let state = transaction
            .query_row(
                "SELECT state FROM repair_cases WHERE id = ?1",
                [case_id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(DatabaseError::Sqlite)?
            .ok_or(DatabaseError::RepairCaseNotFound(case_id))?;
        if state != "eligible" && state != "unresolved" {
            return Err(DatabaseError::RepairCaseNotEligible(case_id));
        }
        let original_identity = transaction
            .query_row(
                "SELECT provider_items.provider, provider_items.provider_item_id
                 FROM repair_cases JOIN provider_items ON
                      provider_items.id = repair_cases.original_provider_item_id
                 WHERE repair_cases.id = ?1",
                [case_id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .map_err(DatabaseError::Sqlite)?;
        let mut inserted = 0_u64;
        for candidate in candidates {
            if original_identity.0 == "youtube" && original_identity.1 == candidate.provider_item_id
            {
                continue;
            }
            let metadata =
                serde_json::to_string(&candidate.raw_metadata).map_err(DatabaseError::Json)?;
            inserted += transaction
                .execute(
                    "INSERT OR IGNORE INTO repair_attempts(
                     repair_case_id, candidate_provider, candidate_provider_item_id,
                     candidate_url, state, reason, candidate_metadata_json)
                 VALUES (?1, 'youtube', ?2, ?3, 'generated',
                         'search generated only; independent identity not yet verified', ?4)",
                    rusqlite::params![case_id, candidate.provider_item_id, candidate.url, metadata],
                )
                .map_err(DatabaseError::Sqlite)? as u64;
        }
        transaction
            .execute(
                "UPDATE repair_cases SET state = 'unresolved', updated_at = CURRENT_TIMESTAMP
             WHERE id = ?1",
                [case_id],
            )
            .map_err(DatabaseError::Sqlite)?;
        transaction.commit().map_err(DatabaseError::Sqlite)?;
        Ok(inserted)
    }

    /// Atomically claims the oldest generated repair candidate whose case remains safe.
    pub fn claim_next_repair_attempt(
        &mut self,
    ) -> Result<Option<RepairAttemptWork>, DatabaseError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(DatabaseError::Sqlite)?;
        let work = transaction
            .query_row(
                "SELECT repair_attempts.id, repair_attempts.repair_case_id,
                        repair_cases.recording_id, repair_cases.original_provider_item_id,
                        repair_attempts.candidate_provider,
                        repair_attempts.candidate_provider_item_id,
                        repair_attempts.candidate_url, repair_attempts.attempt_count + 1,
                        recordings.musicbrainz_recording_id, recordings.isrc,
                        artifacts.duration_ms, artifact_fingerprints.max_seconds,
                        artifact_fingerprints.duration_ms,
                        artifact_fingerprints.fingerprint_json
                 FROM repair_attempts
                 JOIN repair_cases ON repair_cases.id = repair_attempts.repair_case_id
                 JOIN provider_items ON
                      provider_items.id = repair_cases.original_provider_item_id
                 JOIN recordings ON recordings.id = repair_cases.recording_id
                 JOIN artifacts ON artifacts.id = repair_cases.reference_artifact_id
                 JOIN artifact_fingerprints ON
                      artifact_fingerprints.artifact_id = artifacts.id
                 WHERE repair_attempts.state = 'generated'
                   AND repair_cases.state = 'unresolved'
                   AND provider_items.availability = 'permanently_unavailable'
                   AND artifacts.health IN ('missing', 'corrupt')
                   AND EXISTS (
                       SELECT 1 FROM collection_memberships
                       WHERE collection_memberships.provider_item_id = provider_items.id
                         AND collection_memberships.active = 1)
                 ORDER BY repair_attempts.id LIMIT 1",
                [],
                |row| {
                    Ok(RepairAttemptWork {
                        attempt_id: row.get(0)?,
                        case_id: row.get(1)?,
                        recording_id: row.get(2)?,
                        original_provider_item_id: row.get(3)?,
                        candidate_provider: row.get(4)?,
                        candidate_provider_item_id: row.get(5)?,
                        candidate_url: row.get(6)?,
                        attempt: row.get(7)?,
                        reference_musicbrainz_recording_id: row.get(8)?,
                        reference_isrc: row.get(9)?,
                        reference_duration_ms: row.get(10)?,
                        reference_fingerprint_max_seconds: row.get(11)?,
                        reference_fingerprint_duration_ms: row.get(12)?,
                        reference_fingerprint_json: row.get(13)?,
                    })
                },
            )
            .optional()
            .map_err(DatabaseError::Sqlite)?;
        if let Some(work) = &work {
            transaction
                .execute(
                    "UPDATE repair_attempts SET state = 'running',
                         attempt_count = attempt_count + 1, updated_at = CURRENT_TIMESTAMP
                     WHERE id = ?1 AND state = 'generated'",
                    [work.attempt_id],
                )
                .map_err(DatabaseError::Sqlite)?;
        }
        transaction.commit().map_err(DatabaseError::Sqlite)?;
        Ok(work)
    }

    /// Defers one running repair attempt without changing case eligibility.
    pub fn defer_repair_attempt(
        &mut self,
        attempt_id: i64,
        message: &str,
    ) -> Result<bool, DatabaseError> {
        let transaction = self
            .connection
            .transaction()
            .map_err(DatabaseError::Sqlite)?;
        let changed = transaction
            .execute(
                "UPDATE repair_attempts SET state = 'deferred', reason = ?2,
                 updated_at = CURRENT_TIMESTAMP WHERE id = ?1 AND state = 'running'",
                rusqlite::params![attempt_id, message],
            )
            .map_err(DatabaseError::Sqlite)?
            == 1;
        if changed {
            transaction
                .execute(
                    "INSERT INTO events(level, component, event, message, context_json)
                 VALUES ('warning', 'repair', 'repair_attempt_deferred', ?2,
                         json_object('repair_attempt_id', ?1))",
                    rusqlite::params![attempt_id, message],
                )
                .map_err(DatabaseError::Sqlite)?;
        }
        transaction.commit().map_err(DatabaseError::Sqlite)?;
        Ok(changed)
    }

    /// Explicitly releases one deferred repair attempt for a later bounded run.
    pub fn retry_deferred_repair_attempt(
        &mut self,
        attempt_id: i64,
    ) -> Result<bool, DatabaseError> {
        let transaction = self
            .connection
            .transaction()
            .map_err(DatabaseError::Sqlite)?;
        let changed = transaction
            .execute(
                "UPDATE repair_attempts SET state = 'generated',
                 reason = 'operator explicitly requested retry',
                 updated_at = CURRENT_TIMESTAMP
             WHERE id = ?1 AND state = 'deferred'",
                [attempt_id],
            )
            .map_err(DatabaseError::Sqlite)?
            == 1;
        if changed {
            transaction
                .execute(
                    "INSERT INTO events(level, component, event, message, context_json)
                 VALUES ('info', 'repair', 'repair_attempt_retry_requested',
                         'Operator explicitly released deferred repair attempt',
                         json_object('repair_attempt_id', ?1))",
                    [attempt_id],
                )
                .map_err(DatabaseError::Sqlite)?;
        }
        transaction.commit().map_err(DatabaseError::Sqlite)?;
        Ok(changed)
    }

    /// Marks abandoned running repair attempts deferred after operator confirmation.
    pub fn recover_running_repair_attempts(&mut self) -> Result<u64, DatabaseError> {
        let transaction = self
            .connection
            .transaction()
            .map_err(DatabaseError::Sqlite)?;
        let changed = transaction
            .execute(
                "UPDATE repair_attempts SET state = 'deferred',
                 reason = 'operator recovered abandoned running repair attempt',
                 updated_at = CURRENT_TIMESTAMP WHERE state = 'running'",
                [],
            )
            .map_err(DatabaseError::Sqlite)? as u64;
        if changed > 0 {
            transaction
                .execute(
                    "INSERT INTO events(level, component, event, message, context_json)
                 VALUES ('warning', 'repair', 'repair_attempts_recovered',
                         'Operator recovered abandoned running repair attempts',
                         json_object('count', ?1))",
                    [changed],
                )
                .map_err(DatabaseError::Sqlite)?;
        }
        transaction.commit().map_err(DatabaseError::Sqlite)?;
        Ok(changed)
    }

    /// Persists independently derived candidate evidence and its conservative decision.
    pub fn complete_repair_verification(
        &mut self,
        evidence: &RepairVerificationEvidence,
    ) -> Result<(), DatabaseError> {
        let staged_path = evidence
            .staged_path
            .to_str()
            .ok_or_else(|| DatabaseError::NonUnicodePath(evidence.staged_path.clone()))?;
        let bytes = i64::try_from(evidence.bytes)
            .map_err(|_| DatabaseError::ArtifactTooLarge(evidence.bytes))?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(DatabaseError::Sqlite)?;
        let running = transaction
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM repair_attempts WHERE id = ?1 AND state = 'running')",
                [evidence.attempt_id],
                |row| row.get::<_, bool>(0),
            )
            .map_err(DatabaseError::Sqlite)?;
        if !running {
            return Err(DatabaseError::RepairAttemptNotRunning(evidence.attempt_id));
        }
        transaction
            .execute(
                "INSERT INTO repair_attempt_evidence(
                 repair_attempt_id, staged_path, sha256, byte_count, codec, duration_ms,
                 sample_rate_hz, channels, musicbrainz_recording_id, isrc,
                 fingerprint_max_seconds, fingerprint_duration_ms, fingerprint_json,
                 decision, decision_reason)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)
             ON CONFLICT(repair_attempt_id) DO UPDATE SET
                 staged_path = excluded.staged_path, sha256 = excluded.sha256,
                 byte_count = excluded.byte_count, codec = excluded.codec,
                 duration_ms = excluded.duration_ms,
                 sample_rate_hz = excluded.sample_rate_hz, channels = excluded.channels,
                 musicbrainz_recording_id = excluded.musicbrainz_recording_id,
                 isrc = excluded.isrc,
                 fingerprint_max_seconds = excluded.fingerprint_max_seconds,
                 fingerprint_duration_ms = excluded.fingerprint_duration_ms,
                 fingerprint_json = excluded.fingerprint_json,
                 decision = excluded.decision, decision_reason = excluded.decision_reason,
                 updated_at = CURRENT_TIMESTAMP",
                rusqlite::params![
                    evidence.attempt_id,
                    staged_path,
                    evidence.sha256,
                    bytes,
                    evidence.codec,
                    evidence.duration_ms,
                    evidence.sample_rate_hz,
                    evidence.channels,
                    evidence.musicbrainz_recording_id,
                    evidence.isrc,
                    evidence.fingerprint_max_seconds,
                    evidence.fingerprint_duration_ms,
                    evidence.fingerprint_json,
                    evidence.decision.as_str(),
                    evidence.reason,
                ],
            )
            .map_err(DatabaseError::Sqlite)?;
        transaction
            .execute(
                "UPDATE repair_attempts SET state = ?2, reason = ?3,
                 updated_at = CURRENT_TIMESTAMP WHERE id = ?1",
                rusqlite::params![
                    evidence.attempt_id,
                    evidence.decision.as_str(),
                    evidence.reason
                ],
            )
            .map_err(DatabaseError::Sqlite)?;
        if evidence.decision == RepairVerificationDecision::Verified {
            transaction
                .execute(
                    "UPDATE repair_cases SET state = 'verified', updated_at = CURRENT_TIMESTAMP
                 WHERE id = (SELECT repair_case_id FROM repair_attempts WHERE id = ?1)",
                    [evidence.attempt_id],
                )
                .map_err(DatabaseError::Sqlite)?;
        }
        transaction.commit().map_err(DatabaseError::Sqlite)
    }

    /// Lists verified attempts awaiting safe commit in stable order.
    pub fn verified_repair_attempt_ids(&self, limit: usize) -> Result<Vec<i64>, DatabaseError> {
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        let mut statement = self
            .connection
            .prepare("SELECT id FROM repair_attempts WHERE state='verified' ORDER BY id LIMIT ?1")
            .map_err(DatabaseError::Sqlite)?;
        statement
            .query_map([limit], |row| row.get::<_, i64>(0))
            .map_err(DatabaseError::Sqlite)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(DatabaseError::Sqlite)
    }

    /// Loads one verified candidate and its exact staged evidence for commit.
    pub fn verified_repair_attempt(
        &self,
        attempt_id: i64,
    ) -> Result<RepairCommitWork, DatabaseError> {
        self.connection
            .query_row(
                "SELECT repair_attempts.id, repair_attempts.repair_case_id,
                    repair_cases.recording_id, repair_attempts.candidate_provider,
                    repair_attempts.candidate_provider_item_id,
                    repair_attempts.candidate_url, repair_attempts.candidate_metadata_json,
                    repair_attempt_evidence.staged_path, repair_attempt_evidence.sha256,
                    repair_attempt_evidence.byte_count, repair_attempt_evidence.codec,
                    repair_attempt_evidence.duration_ms,
                    repair_attempt_evidence.sample_rate_hz,
                    repair_attempt_evidence.channels,
                    repair_attempt_evidence.musicbrainz_recording_id,
                    repair_attempt_evidence.isrc,
                    repair_attempt_evidence.fingerprint_max_seconds,
                    repair_attempt_evidence.fingerprint_duration_ms,
                    repair_attempt_evidence.fingerprint_json
             FROM repair_attempts
             JOIN repair_cases ON repair_cases.id = repair_attempts.repair_case_id
             JOIN provider_items AS original_provider_item ON
                  original_provider_item.id = repair_cases.original_provider_item_id
             JOIN artifacts AS reference_artifact ON
                  reference_artifact.id = repair_cases.reference_artifact_id
             JOIN repair_attempt_evidence ON
                  repair_attempt_evidence.repair_attempt_id = repair_attempts.id
             WHERE repair_attempts.id = ?1 AND repair_attempts.state IN ('verified', 'committed')
               AND repair_cases.state = 'verified'
               AND repair_attempt_evidence.decision = 'verified'
               AND original_provider_item.availability = 'permanently_unavailable'
               AND reference_artifact.health IN ('missing', 'corrupt')
               AND EXISTS (SELECT 1 FROM collection_memberships
                   WHERE collection_memberships.provider_item_id = original_provider_item.id
                     AND collection_memberships.active = 1)",
                [attempt_id],
                |row| {
                    let bytes = row.get::<_, i64>(9)?;
                    let duration_ms = row.get::<_, Option<i64>>(11)?;
                    let sample_rate_hz = row.get::<_, Option<i64>>(12)?;
                    let channels = row.get::<_, Option<i64>>(13)?;
                    Ok(RepairCommitWork {
                        attempt_id: row.get(0)?,
                        case_id: row.get(1)?,
                        recording_id: row.get(2)?,
                        candidate_provider: row.get(3)?,
                        candidate_provider_item_id: row.get(4)?,
                        candidate_url: row.get(5)?,
                        candidate_metadata_json: row.get(6)?,
                        validated: ValidatedStagedMedia {
                            path: PathBuf::from(row.get::<_, String>(7)?),
                            sha256: row.get(8)?,
                            bytes: u64::try_from(bytes).map_err(|error| {
                                rusqlite::Error::FromSqlConversionFailure(
                                    9,
                                    rusqlite::types::Type::Integer,
                                    Box::new(error),
                                )
                            })?,
                            codec: row.get(10)?,
                            duration_ms: duration_ms.map(u64::try_from).transpose().map_err(
                                |error| {
                                    rusqlite::Error::FromSqlConversionFailure(
                                        11,
                                        rusqlite::types::Type::Integer,
                                        Box::new(error),
                                    )
                                },
                            )?,
                            sample_rate_hz: sample_rate_hz.map(u32::try_from).transpose().map_err(
                                |error| {
                                    rusqlite::Error::FromSqlConversionFailure(
                                        12,
                                        rusqlite::types::Type::Integer,
                                        Box::new(error),
                                    )
                                },
                            )?,
                            channels: channels.map(u32::try_from).transpose().map_err(|error| {
                                rusqlite::Error::FromSqlConversionFailure(
                                    13,
                                    rusqlite::types::Type::Integer,
                                    Box::new(error),
                                )
                            })?,
                            musicbrainz_recording_id: row.get(14)?,
                            isrc: row.get(15)?,
                        },
                        fingerprint_max_seconds: row.get(16)?,
                        fingerprint_duration_ms: row.get(17)?,
                        fingerprint_json: row.get(18)?,
                    })
                },
            )
            .optional()
            .map_err(DatabaseError::Sqlite)?
            .ok_or(DatabaseError::RepairAttemptNotVerified(attempt_id))
    }

    /// Persists recoverable intent before the no-clobber repair filesystem effect.
    pub(crate) fn prepare_repair_commit(
        &mut self,
        attempt_id: i64,
        final_path: &Path,
    ) -> Result<bool, DatabaseError> {
        let final_path = final_path
            .to_str()
            .ok_or_else(|| DatabaseError::NonUnicodePath(final_path.to_path_buf()))?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(DatabaseError::Sqlite)?;
        let verified = transaction
            .query_row(
                "SELECT EXISTS(
                     SELECT 1 FROM repair_attempts
                     JOIN repair_cases ON repair_cases.id = repair_attempts.repair_case_id
                     JOIN provider_items ON
                          provider_items.id = repair_cases.original_provider_item_id
                     JOIN artifacts ON artifacts.id = repair_cases.reference_artifact_id
                     WHERE repair_attempts.id = ?1
                       AND repair_attempts.state IN ('verified', 'committed')
                       AND repair_cases.state = 'verified'
                       AND provider_items.availability = 'permanently_unavailable'
                       AND artifacts.health IN ('missing', 'corrupt')
                       AND EXISTS (SELECT 1 FROM collection_memberships
                           WHERE collection_memberships.provider_item_id = provider_items.id
                             AND collection_memberships.active = 1))",
                [attempt_id],
                |row| row.get::<_, bool>(0),
            )
            .map_err(DatabaseError::Sqlite)?;
        if !verified {
            return Err(DatabaseError::RepairAttemptNotVerified(attempt_id));
        }
        let inserted = transaction
            .execute(
                "INSERT OR IGNORE INTO repair_commits(repair_attempt_id, final_path)
             VALUES (?1, ?2)",
                rusqlite::params![attempt_id, final_path],
            )
            .map_err(DatabaseError::Sqlite)?
            == 1;
        let stored = transaction
            .query_row(
                "SELECT final_path FROM repair_commits WHERE repair_attempt_id = ?1",
                [attempt_id],
                |row| row.get::<_, String>(0),
            )
            .map_err(DatabaseError::Sqlite)?;
        if stored != final_path {
            return Err(DatabaseError::RepairCommitMismatch(attempt_id));
        }
        transaction.commit().map_err(DatabaseError::Sqlite)?;
        Ok(inserted)
    }

    /// Atomically records a prepared verified repair after its final link exists.
    pub(crate) fn finalize_repair_commit(
        &mut self,
        attempt_id: i64,
    ) -> Result<RepairCommitPersistence, DatabaseError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(DatabaseError::Sqlite)?;
        let row = transaction
            .query_row(
                "SELECT repair_cases.recording_id, repair_attempts.repair_case_id,
                    repair_attempts.candidate_provider,
                    repair_attempts.candidate_provider_item_id,
                    repair_attempts.candidate_url,
                    repair_attempts.candidate_metadata_json,
                    repair_attempt_evidence.sha256, repair_attempt_evidence.byte_count,
                    repair_attempt_evidence.codec, repair_attempt_evidence.duration_ms,
                    repair_attempt_evidence.sample_rate_hz, repair_attempt_evidence.channels,
                    repair_attempt_evidence.fingerprint_max_seconds,
                    repair_attempt_evidence.fingerprint_duration_ms,
                    repair_attempt_evidence.fingerprint_json,
                    repair_commits.final_path, repair_commits.artifact_id
             FROM repair_attempts
             JOIN repair_cases ON repair_cases.id = repair_attempts.repair_case_id
             JOIN provider_items AS original_provider_item ON
                  original_provider_item.id = repair_cases.original_provider_item_id
             JOIN artifacts AS reference_artifact ON
                  reference_artifact.id = repair_cases.reference_artifact_id
             JOIN repair_attempt_evidence ON
                  repair_attempt_evidence.repair_attempt_id = repair_attempts.id
             JOIN repair_commits ON repair_commits.repair_attempt_id = repair_attempts.id
             WHERE repair_attempts.id = ?1 AND repair_attempts.state IN ('verified', 'committed')
               AND repair_cases.state = 'verified'
               AND repair_attempt_evidence.decision = 'verified'
               AND original_provider_item.availability = 'permanently_unavailable'
               AND reference_artifact.health IN ('missing', 'corrupt')
               AND EXISTS (SELECT 1 FROM collection_memberships
                   WHERE collection_memberships.provider_item_id = original_provider_item.id
                     AND collection_memberships.active = 1)",
                [attempt_id],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, String>(5)?,
                        row.get::<_, String>(6)?,
                        row.get::<_, i64>(7)?,
                        row.get::<_, String>(8)?,
                        row.get::<_, Option<i64>>(9)?,
                        row.get::<_, Option<i64>>(10)?,
                        row.get::<_, Option<i64>>(11)?,
                        row.get::<_, i64>(12)?,
                        row.get::<_, i64>(13)?,
                        row.get::<_, String>(14)?,
                        row.get::<_, String>(15)?,
                        row.get::<_, Option<i64>>(16)?,
                    ))
                },
            )
            .optional()
            .map_err(DatabaseError::Sqlite)?
            .ok_or(DatabaseError::RepairCommitNotPrepared(attempt_id))?;
        if let Some(artifact_id) = row.16 {
            return Ok(RepairCommitPersistence {
                artifact_id,
                inserted: false,
            });
        }
        let existing_recording = transaction
            .query_row(
                "SELECT recording_id FROM provider_items
                 WHERE provider = ?1 AND provider_item_id = ?2",
                rusqlite::params![row.2, row.3],
                |row| row.get::<_, Option<i64>>(0),
            )
            .optional()
            .map_err(DatabaseError::Sqlite)?
            .flatten();
        if existing_recording.is_some_and(|recording_id| recording_id != row.0) {
            return Err(DatabaseError::RepairCandidateIdentityConflict {
                provider: row.2,
                provider_item_id: row.3,
            });
        }
        transaction
            .execute(
                "INSERT INTO artifacts(recording_id, path, sha256, duration_ms, codec,
                 sample_rate_hz, channels, health)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'healthy')",
                rusqlite::params![row.0, row.15, row.6, row.9, row.8, row.10, row.11],
            )
            .map_err(DatabaseError::Sqlite)?;
        let artifact_id = transaction.last_insert_rowid();
        transaction
            .execute(
                "INSERT INTO artifact_fingerprints(artifact_id, algorithm, max_seconds,
                 duration_ms, fingerprint_json, value_count)
             VALUES (?1, 2, ?2, ?3, ?4, json_array_length(?4))",
                rusqlite::params![artifact_id, row.12, row.13, row.14],
            )
            .map_err(DatabaseError::Sqlite)?;
        transaction
            .execute(
                "UPDATE recordings SET preferred_artifact_id = ?2 WHERE id = ?1",
                rusqlite::params![row.0, artifact_id],
            )
            .map_err(DatabaseError::Sqlite)?;
        transaction
            .execute(
                "INSERT INTO provider_items(provider, provider_item_id, original_url,
                 source_metadata_json, availability, recording_id)
             VALUES (?1, ?2, ?3, ?4, 'available', ?5)
             ON CONFLICT(provider, provider_item_id) DO UPDATE SET
                 original_url = excluded.original_url,
                 source_metadata_json = excluded.source_metadata_json,
                 availability = 'available', recording_id = excluded.recording_id",
                rusqlite::params![row.2, row.3, row.4, row.5, row.0],
            )
            .map_err(DatabaseError::Sqlite)?;
        transaction
            .execute(
                "UPDATE repair_attempts SET state = 'committed', reason =
                 'independently verified replacement committed without overwrite',
                 updated_at = CURRENT_TIMESTAMP WHERE id = ?1",
                [attempt_id],
            )
            .map_err(DatabaseError::Sqlite)?;
        transaction
            .execute(
                "UPDATE repair_commits SET artifact_id = ?2, committed_at = CURRENT_TIMESTAMP
             WHERE repair_attempt_id = ?1",
                rusqlite::params![attempt_id, artifact_id],
            )
            .map_err(DatabaseError::Sqlite)?;
        transaction.commit().map_err(DatabaseError::Sqlite)?;
        Ok(RepairCommitPersistence {
            artifact_id,
            inserted: true,
        })
    }

    /// Persists one conservative artifact-health observation transactionally.
    pub fn record_artifact_health(
        &mut self,
        observation: &ArtifactHealthObservation,
    ) -> Result<bool, DatabaseError> {
        let transaction = self
            .connection
            .transaction()
            .map_err(DatabaseError::Sqlite)?;
        let prior = transaction
            .query_row(
                "SELECT health FROM artifacts WHERE id = ?1",
                [observation.artifact_id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(DatabaseError::Sqlite)?
            .ok_or(DatabaseError::ArtifactNotFound(observation.artifact_id))?;
        let health = observation.health.as_str();
        let changed = prior != health;
        transaction
            .execute(
                "UPDATE artifacts SET health = ?2,
                     sha256 = COALESCE(?3, sha256),
                     duration_ms = COALESCE(?4, duration_ms),
                     codec = COALESCE(?5, codec),
                     sample_rate_hz = COALESCE(?6, sample_rate_hz),
                     channels = COALESCE(?7, channels)
                 WHERE id = ?1",
                rusqlite::params![
                    observation.artifact_id,
                    health,
                    observation.sha256,
                    observation.duration_ms,
                    observation.codec,
                    observation.sample_rate_hz,
                    observation.channels
                ],
            )
            .map_err(DatabaseError::Sqlite)?;
        if changed {
            let level = if observation.health == ArtifactHealth::Healthy {
                "info"
            } else {
                "warning"
            };
            transaction
                .execute(
                    "INSERT INTO events(level, component, event, message, context_json)
                     VALUES (?1, 'artifact-health', 'artifact_health_changed', ?2, ?3)",
                    rusqlite::params![
                        level,
                        format!(
                            "Artifact {} health changed from {} to {}",
                            observation.artifact_id, prior, health
                        ),
                        format!("{{\"artifact_id\":{}}}", observation.artifact_id)
                    ],
                )
                .map_err(DatabaseError::Sqlite)?;
        }
        transaction.commit().map_err(DatabaseError::Sqlite)?;
        Ok(changed)
    }

    /// Reports the applied schema version.
    pub fn schema_version(&self) -> Result<u32, DatabaseError> {
        self.connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .map_err(DatabaseError::Sqlite)
    }

    /// Registers adopted artifact paths atomically without assigning canonical IDs.
    pub fn register_adopted_artifacts(
        &mut self,
        paths: &[PathBuf],
    ) -> Result<AdoptionPersistenceSummary, DatabaseError> {
        let transaction = self
            .connection
            .transaction()
            .map_err(DatabaseError::Sqlite)?;
        let mut summary = AdoptionPersistenceSummary::default();
        for path in paths {
            let path = path
                .to_str()
                .ok_or_else(|| DatabaseError::NonUnicodePath(path.clone()))?;
            let existing = transaction
                .query_row("SELECT id FROM artifacts WHERE path = ?1", [path], |row| {
                    row.get::<_, i64>(0)
                })
                .optional()
                .map_err(DatabaseError::Sqlite)?;
            if existing.is_some() {
                summary.artifacts_existing += 1;
                continue;
            }
            transaction
                .execute("INSERT INTO recordings DEFAULT VALUES", [])
                .map_err(DatabaseError::Sqlite)?;
            let recording_id = transaction.last_insert_rowid();
            transaction
                .execute(
                    "INSERT INTO artifacts(recording_id, path, health) VALUES (?1, ?2, 'unknown')",
                    rusqlite::params![recording_id, path],
                )
                .map_err(DatabaseError::Sqlite)?;
            summary.recordings_inserted += 1;
            summary.artifacts_inserted += 1;
        }
        transaction.commit().map_err(DatabaseError::Sqlite)?;
        Ok(summary)
    }

    /// Adds a YouTube source idempotently or reactivates an existing source.
    pub fn add_source(
        &mut self,
        url: &str,
        name: Option<&str>,
    ) -> Result<AddSourceResult, DatabaseError> {
        let transaction = self
            .connection
            .transaction()
            .map_err(DatabaseError::Sqlite)?;
        let existing = transaction
            .query_row(
                "SELECT id, active FROM sources WHERE provider = 'youtube' AND url = ?1",
                [url],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, bool>(1)?)),
            )
            .optional()
            .map_err(DatabaseError::Sqlite)?;
        let result = if let Some((id, active)) = existing {
            transaction
                .execute(
                    "UPDATE sources SET active = 1, name = COALESCE(?2, name),
                     updated_at = CURRENT_TIMESTAMP WHERE id = ?1",
                    rusqlite::params![id, name],
                )
                .map_err(DatabaseError::Sqlite)?;
            AddSourceResult {
                id: SourceId(id),
                inserted: false,
                reactivated: !active,
            }
        } else {
            transaction
                .execute(
                    "INSERT INTO sources(provider, url, name) VALUES ('youtube', ?1, ?2)",
                    rusqlite::params![url, name],
                )
                .map_err(DatabaseError::Sqlite)?;
            AddSourceResult {
                id: SourceId(transaction.last_insert_rowid()),
                inserted: true,
                reactivated: false,
            }
        };
        transaction.commit().map_err(DatabaseError::Sqlite)?;
        Ok(result)
    }

    /// Lists configured sources in stable ID order.
    pub fn list_sources(
        &self,
        include_inactive: bool,
    ) -> Result<Vec<ConfiguredSource>, DatabaseError> {
        let sql = if include_inactive {
            "SELECT id, provider, url, name, active FROM sources ORDER BY id"
        } else {
            "SELECT id, provider, url, name, active FROM sources WHERE active = 1 ORDER BY id"
        };
        let mut statement = self
            .connection
            .prepare(sql)
            .map_err(DatabaseError::Sqlite)?;
        let rows = statement
            .query_map([], |row| {
                Ok(ConfiguredSource {
                    id: SourceId(row.get(0)?),
                    provider: row.get(1)?,
                    url: row.get(2)?,
                    name: row.get(3)?,
                    active: row.get(4)?,
                })
            })
            .map_err(DatabaseError::Sqlite)?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(DatabaseError::Sqlite)
    }

    /// Loads one configured source by durable ID.
    pub fn get_source(&self, id: SourceId) -> Result<Option<ConfiguredSource>, DatabaseError> {
        self.connection
            .query_row(
                "SELECT id, provider, url, name, active FROM sources WHERE id = ?1",
                [id.0],
                |row| {
                    Ok(ConfiguredSource {
                        id: SourceId(row.get(0)?),
                        provider: row.get(1)?,
                        url: row.get(2)?,
                        name: row.get(3)?,
                        active: row.get(4)?,
                    })
                },
            )
            .optional()
            .map_err(DatabaseError::Sqlite)
    }

    /// Deactivates a source without deleting its row or related history.
    pub fn deactivate_source(&mut self, id: SourceId) -> Result<bool, DatabaseError> {
        self.connection
            .execute(
                "UPDATE sources SET active = 0, updated_at = CURRENT_TIMESTAMP
                 WHERE id = ?1 AND active = 1",
                [id.0],
            )
            .map(|changed| changed == 1)
            .map_err(DatabaseError::Sqlite)
    }

    /// Reconciles one successful provider snapshot atomically.
    pub fn reconcile_source_snapshot(
        &mut self,
        source_id: SourceId,
        snapshot: &SourceSnapshot,
    ) -> Result<SnapshotReconciliationSummary, DatabaseError> {
        let transaction = self
            .connection
            .transaction()
            .map_err(DatabaseError::Sqlite)?;
        let source = transaction
            .query_row(
                "SELECT provider, url, name, active FROM sources WHERE id = ?1",
                [source_id.0],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, bool>(3)?,
                    ))
                },
            )
            .optional()
            .map_err(DatabaseError::Sqlite)?
            .ok_or(DatabaseError::SourceNotFound(source_id))?;
        if !source.3 {
            return Err(DatabaseError::SourceInactive(source_id));
        }
        if source.0 != snapshot.provider {
            return Err(DatabaseError::ProviderMismatch {
                configured: source.0,
                snapshot: snapshot.provider.clone(),
            });
        }

        let collection_id = source_collection_id(
            &transaction,
            source_id,
            snapshot,
            source.2.as_deref().unwrap_or(&source.1),
        )?;
        let previously_active = active_membership_ids(&transaction, collection_id)?;
        transaction
            .execute(
                "UPDATE collection_memberships SET active = 0 WHERE collection_id = ?1",
                [collection_id],
            )
            .map_err(DatabaseError::Sqlite)?;
        transaction
            .execute("INSERT INTO sync_runs(status) VALUES ('running')", [])
            .map_err(DatabaseError::Sqlite)?;
        let run_id = transaction.last_insert_rowid();
        let mut summary = SnapshotReconciliationSummary::default();
        let mut seen = BTreeSet::new();

        for (position, item) in snapshot.items.iter().enumerate() {
            let raw = serde_json::to_string(&item.raw_metadata).map_err(DatabaseError::Json)?;
            let existing = transaction
                .query_row(
                    "SELECT id FROM provider_items WHERE provider = ?1 AND provider_item_id = ?2",
                    rusqlite::params![snapshot.provider, item.provider_item_id],
                    |row| row.get::<_, i64>(0),
                )
                .optional()
                .map_err(DatabaseError::Sqlite)?;
            let provider_item_id = if let Some(id) = existing {
                summary.provider_items_existing += 1;
                transaction
                    .execute(
                        "UPDATE provider_items SET original_url = ?2, source_title = ?3,
                         source_metadata_json = ?4, availability = 'available' WHERE id = ?1",
                        rusqlite::params![id, item.url, item.title, raw],
                    )
                    .map_err(DatabaseError::Sqlite)?;
                id
            } else {
                transaction
                    .execute(
                        "INSERT INTO provider_items(provider, provider_item_id, original_url,
                         source_title, source_metadata_json, availability)
                         VALUES (?1, ?2, ?3, ?4, ?5, 'available')",
                        rusqlite::params![
                            snapshot.provider,
                            item.provider_item_id,
                            item.url,
                            item.title,
                            raw
                        ],
                    )
                    .map_err(DatabaseError::Sqlite)?;
                summary.provider_items_inserted += 1;
                transaction.last_insert_rowid()
            };
            seen.insert(provider_item_id);
            if previously_active.contains(&provider_item_id) {
                summary.memberships_unchanged += 1;
            } else {
                summary.memberships_activated += 1;
            }
            transaction
                .execute(
                    "INSERT INTO collection_memberships(
                         collection_id, provider_item_id, active, position)
                     VALUES (?1, ?2, 1, ?3)
                     ON CONFLICT(collection_id, provider_item_id) DO UPDATE SET
                         active = 1, position = excluded.position,
                         last_seen_at = CURRENT_TIMESTAMP",
                    rusqlite::params![collection_id, provider_item_id, position as i64],
                )
                .map_err(DatabaseError::Sqlite)?;
            let idempotency_key =
                format!("acquire:{}:{}", snapshot.provider, item.provider_item_id);
            summary.jobs_created += transaction
                .execute(
                    "INSERT OR IGNORE INTO jobs(run_id, kind, status, idempotency_key)
                     VALUES (?1, 'acquire', 'pending', ?2)",
                    rusqlite::params![run_id, &idempotency_key],
                )
                .map_err(DatabaseError::Sqlite)? as u64;
            let job_id = transaction
                .query_row(
                    "SELECT id FROM jobs WHERE idempotency_key = ?1",
                    [&idempotency_key],
                    |row| row.get::<_, i64>(0),
                )
                .map_err(DatabaseError::Sqlite)?;
            transaction
                .execute(
                    "INSERT OR IGNORE INTO acquisition_jobs(job_id, provider_item_id)
                     VALUES (?1, ?2)",
                    rusqlite::params![job_id, provider_item_id],
                )
                .map_err(DatabaseError::Sqlite)?;
        }
        summary.memberships_deactivated = previously_active.difference(&seen).count() as u64;
        transaction
            .execute(
                "UPDATE sync_runs SET status = 'succeeded', finished_at = CURRENT_TIMESTAMP
                 WHERE id = ?1",
                [run_id],
            )
            .map_err(DatabaseError::Sqlite)?;
        transaction.commit().map_err(DatabaseError::Sqlite)?;
        Ok(summary)
    }

    /// Atomically claims the oldest explicitly pending acquisition job.
    pub fn claim_next_acquisition(&mut self) -> Result<Option<AcquisitionWork>, DatabaseError> {
        let Some(job_id) = self.runnable_acquisition_job_ids(1)?.into_iter().next() else {
            return Ok(None);
        };
        self.claim_acquisition(job_id)
    }

    /// Snapshots oldest runnable acquisition job IDs without claiming them.
    pub fn runnable_acquisition_job_ids(&self, limit: usize) -> Result<Vec<i64>, DatabaseError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        let mut statement = self
            .connection
            .prepare(
                "SELECT jobs.id FROM jobs
                 JOIN acquisition_jobs ON acquisition_jobs.job_id = jobs.id
                 WHERE jobs.kind = 'acquire' AND jobs.status = 'pending'
                 ORDER BY jobs.id LIMIT ?1",
            )
            .map_err(DatabaseError::Sqlite)?;
        let rows = statement
            .query_map([limit], |row| row.get(0))
            .map_err(DatabaseError::Sqlite)?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(DatabaseError::Sqlite)
    }

    /// Atomically claims one specific pending acquisition job.
    pub fn claim_acquisition(
        &mut self,
        job_id: i64,
    ) -> Result<Option<AcquisitionWork>, DatabaseError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(DatabaseError::Sqlite)?;
        let work = transaction
            .query_row(
                "SELECT jobs.id, jobs.attempt_count, provider_items.provider,
                        provider_items.provider_item_id, provider_items.original_url,
                        provider_items.source_title
                 FROM jobs
                 JOIN acquisition_jobs ON acquisition_jobs.job_id = jobs.id
                 JOIN provider_items ON provider_items.id = acquisition_jobs.provider_item_id
                 WHERE jobs.id = ?1 AND jobs.kind = 'acquire'
                   AND jobs.status = 'pending'",
                [job_id],
                |row| {
                    Ok(AcquisitionWork {
                        job_id: row.get(0)?,
                        attempt: row.get::<_, u32>(1)? + 1,
                        provider: row.get(2)?,
                        provider_item_id: row.get(3)?,
                        original_url: row.get(4)?,
                        source_title: row.get(5)?,
                    })
                },
            )
            .optional()
            .map_err(DatabaseError::Sqlite)?;
        if let Some(work) = &work {
            transaction
                .execute(
                    "UPDATE jobs SET status = 'running', attempt_count = attempt_count + 1,
                     updated_at = CURRENT_TIMESTAMP WHERE id = ?1",
                    [work.job_id],
                )
                .map_err(DatabaseError::Sqlite)?;
        }
        transaction.commit().map_err(DatabaseError::Sqlite)?;
        Ok(work)
    }

    /// Defers a running acquisition while retaining its staged evidence for retry.
    pub fn defer_acquisition(&mut self, job_id: i64, message: &str) -> Result<bool, DatabaseError> {
        let transaction = self
            .connection
            .transaction()
            .map_err(DatabaseError::Sqlite)?;
        let changed = transaction
            .execute(
                "UPDATE jobs SET status = 'deferred', updated_at = CURRENT_TIMESTAMP
                 WHERE id = ?1 AND kind = 'acquire' AND status = 'running'",
                [job_id],
            )
            .map_err(DatabaseError::Sqlite)?
            == 1;
        if changed {
            transaction
                .execute(
                    "INSERT INTO events(run_id, job_id, level, component, event, message)
                     SELECT run_id, id, 'warning', 'acquisition', 'acquisition_deferred', ?2
                     FROM jobs WHERE id = ?1",
                    rusqlite::params![job_id, message],
                )
                .map_err(DatabaseError::Sqlite)?;
        }
        transaction.commit().map_err(DatabaseError::Sqlite)?;
        Ok(changed)
    }

    /// Converts abandoned running acquisitions into explicitly retryable work.
    pub fn recover_interrupted_acquisitions(&mut self) -> Result<u64, DatabaseError> {
        let transaction = self
            .connection
            .transaction()
            .map_err(DatabaseError::Sqlite)?;
        let job_ids = {
            let mut statement = transaction
                .prepare("SELECT id FROM jobs WHERE kind = 'acquire' AND status = 'running'")
                .map_err(DatabaseError::Sqlite)?;
            statement
                .query_map([], |row| row.get::<_, i64>(0))
                .map_err(DatabaseError::Sqlite)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(DatabaseError::Sqlite)?
        };
        let changed = transaction
            .execute(
                "UPDATE jobs SET status = 'deferred', updated_at = CURRENT_TIMESTAMP
                 WHERE kind = 'acquire' AND status = 'running'",
                [],
            )
            .map_err(DatabaseError::Sqlite)? as u64;
        for job_id in job_ids {
            transaction
                .execute(
                    "INSERT INTO events(run_id, job_id, level, component, event, message)
                     SELECT run_id, id, 'warning', 'acquisition',
                            'acquisition_interrupted_recovered',
                            'Operator recovered abandoned running acquisition'
                     FROM jobs WHERE id = ?1",
                    [job_id],
                )
                .map_err(DatabaseError::Sqlite)?;
        }
        transaction.commit().map_err(DatabaseError::Sqlite)?;
        Ok(changed)
    }

    /// Explicitly releases one deferred acquisition for a future bounded run.
    pub fn retry_deferred_acquisition(&mut self, job_id: i64) -> Result<bool, DatabaseError> {
        let transaction = self
            .connection
            .transaction()
            .map_err(DatabaseError::Sqlite)?;
        let changed = transaction
            .execute(
                "UPDATE jobs SET status = 'pending', updated_at = CURRENT_TIMESTAMP
                 WHERE id = ?1 AND kind = 'acquire' AND status = 'deferred'",
                [job_id],
            )
            .map_err(DatabaseError::Sqlite)?
            == 1;
        if changed {
            transaction
                .execute(
                    "INSERT INTO events(run_id, job_id, level, component, event, message)
                     SELECT run_id, id, 'info', 'acquisition',
                            'acquisition_retry_requested',
                            'Operator explicitly released deferred acquisition for retry'
                     FROM jobs WHERE id = ?1",
                    [job_id],
                )
                .map_err(DatabaseError::Sqlite)?;
        }
        transaction.commit().map_err(DatabaseError::Sqlite)?;
        Ok(changed)
    }

    pub(crate) fn prepare_acquisition_commit(
        &mut self,
        job_id: i64,
        validated: &ValidatedStagedMedia,
        final_path: &Path,
    ) -> Result<bool, DatabaseError> {
        let staged_path = validated
            .path
            .to_str()
            .ok_or_else(|| DatabaseError::NonUnicodePath(validated.path.clone()))?;
        let final_path_text = final_path
            .to_str()
            .ok_or_else(|| DatabaseError::NonUnicodePath(final_path.to_path_buf()))?;
        let bytes = i64::try_from(validated.bytes)
            .map_err(|_| DatabaseError::ArtifactTooLarge(validated.bytes))?;
        let duration_ms = validated
            .duration_ms
            .map(i64::try_from)
            .transpose()
            .map_err(|_| DatabaseError::ArtifactTooLarge(validated.duration_ms.unwrap_or(0)))?;
        let transaction = self
            .connection
            .transaction()
            .map_err(DatabaseError::Sqlite)?;
        let running = transaction
            .query_row(
                "SELECT EXISTS(
                     SELECT 1 FROM jobs WHERE id = ?1 AND kind = 'acquire'
                     AND status IN ('running', 'succeeded')
                 )",
                [job_id],
                |row| row.get::<_, bool>(0),
            )
            .map_err(DatabaseError::Sqlite)?;
        if !running {
            return Err(DatabaseError::AcquisitionNotRunning(job_id));
        }
        let discovery_assertion = transaction
            .query_row(
                "SELECT recordings.musicbrainz_recording_id,
                        discovery_acquisition_assertions.canonical_isrc,
                        discovery_acquisition_assertions.canonical_duration_ms
                 FROM acquisition_jobs
                 JOIN discovery_acquisition_assertions ON
                      discovery_acquisition_assertions.provider_item_id=acquisition_jobs.provider_item_id
                 JOIN recordings ON recordings.id=discovery_acquisition_assertions.recording_id
                 WHERE acquisition_jobs.job_id=?1",
                [job_id],
                |row| Ok((row.get::<_, Option<String>>(0)?, row.get::<_, Option<String>>(1)?, row.get::<_, Option<u64>>(2)?)),
            )
            .optional()
            .map_err(DatabaseError::Sqlite)?;
        if let Some((canonical_mbid, canonical_isrc, canonical_duration)) = discovery_assertion {
            let identity_matches = canonical_mbid
                .as_ref()
                .zip(validated.musicbrainz_recording_id.as_ref())
                .is_some_and(|(left, right)| left == right)
                || canonical_isrc
                    .as_ref()
                    .zip(validated.isrc.as_ref())
                    .is_some_and(|(left, right)| left == right);
            let duration_matches = canonical_duration
                .zip(validated.duration_ms)
                .is_some_and(|(left, right)| left.abs_diff(right) <= 2_000);
            if !identity_matches || !duration_matches {
                return Err(DatabaseError::DiscoveryAcquisitionEvidenceMismatch(job_id));
            }
        }
        let inserted = transaction
            .execute(
                "INSERT OR IGNORE INTO acquisition_commits(
                     job_id, staged_path, final_path, sha256, bytes, codec,
                     duration_ms, sample_rate_hz, channels, status,
                     musicbrainz_recording_id, isrc)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 'prepared', ?10, ?11)",
                rusqlite::params![
                    job_id,
                    staged_path,
                    final_path_text,
                    validated.sha256,
                    bytes,
                    validated.codec,
                    duration_ms,
                    validated.sample_rate_hz,
                    validated.channels,
                    validated.musicbrainz_recording_id,
                    validated.isrc,
                ],
            )
            .map_err(DatabaseError::Sqlite)?
            == 1;
        let stored = transaction
            .query_row(
                "SELECT staged_path, final_path, sha256, bytes, codec, duration_ms,
                        sample_rate_hz, channels, musicbrainz_recording_id, isrc
                 FROM acquisition_commits WHERE job_id = ?1",
                [job_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, i64>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, Option<i64>>(5)?,
                        row.get::<_, Option<u32>>(6)?,
                        row.get::<_, Option<u32>>(7)?,
                        row.get::<_, Option<String>>(8)?,
                        row.get::<_, Option<String>>(9)?,
                    ))
                },
            )
            .optional()
            .map_err(DatabaseError::Sqlite)?
            .ok_or_else(|| DatabaseError::ArtifactPathReserved(final_path.to_path_buf()))?;
        let expected = (
            staged_path.to_owned(),
            final_path_text.to_owned(),
            validated.sha256.clone(),
            bytes,
            validated.codec.clone(),
            duration_ms,
            validated.sample_rate_hz,
            validated.channels,
            validated.musicbrainz_recording_id.clone(),
            validated.isrc.clone(),
        );
        if stored != expected {
            return Err(DatabaseError::AcquisitionCommitMismatch(job_id));
        }
        transaction.commit().map_err(DatabaseError::Sqlite)?;
        Ok(inserted)
    }

    pub(crate) fn finalize_acquisition_commit(
        &mut self,
        job_id: i64,
    ) -> Result<ArtifactPersistenceResult, DatabaseError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(DatabaseError::Sqlite)?;
        let commit = transaction
            .query_row(
                "SELECT acquisition_commits.status, acquisition_commits.final_path,
                        acquisition_commits.sha256, acquisition_commits.codec,
                        acquisition_commits.duration_ms,
                        acquisition_commits.sample_rate_hz, acquisition_commits.channels,
                        acquisition_commits.musicbrainz_recording_id,
                        acquisition_commits.isrc,
                        acquisition_jobs.provider_item_id, jobs.status
                 FROM acquisition_commits
                 JOIN acquisition_jobs USING(job_id)
                 JOIN jobs ON jobs.id = acquisition_commits.job_id
                 WHERE acquisition_commits.job_id = ?1",
                [job_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, Option<i64>>(4)?,
                        row.get::<_, Option<u32>>(5)?,
                        row.get::<_, Option<u32>>(6)?,
                        row.get::<_, Option<String>>(7)?,
                        row.get::<_, Option<String>>(8)?,
                        row.get::<_, i64>(9)?,
                        row.get::<_, String>(10)?,
                    ))
                },
            )
            .optional()
            .map_err(DatabaseError::Sqlite)?
            .ok_or(DatabaseError::AcquisitionCommitNotPrepared(job_id))?;
        if commit.0 == "committed" && commit.10 == "succeeded" {
            let (recording_id, artifact_id) = transaction
                .query_row(
                    "SELECT artifacts.recording_id, artifacts.id FROM artifacts
                     WHERE artifacts.path = ?1",
                    [&commit.1],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .map_err(DatabaseError::Sqlite)?;
            transaction.commit().map_err(DatabaseError::Sqlite)?;
            return Ok(ArtifactPersistenceResult {
                recording_id,
                artifact_id,
                inserted: false,
            });
        }
        if commit.0 != "prepared" || commit.10 != "running" {
            return Err(DatabaseError::AcquisitionCommitNotPrepared(job_id));
        }
        let existing_recording_id = transaction
            .query_row(
                "SELECT recording_id FROM provider_items WHERE id = ?1",
                [commit.9],
                |row| row.get::<_, Option<i64>>(0),
            )
            .map_err(DatabaseError::Sqlite)?;
        let recording_id = if let Some(recording_id) = existing_recording_id {
            let prior = transaction
                .query_row(
                    "SELECT musicbrainz_recording_id, isrc FROM recordings WHERE id = ?1",
                    [recording_id],
                    |row| {
                        Ok((
                            row.get::<_, Option<String>>(0)?,
                            row.get::<_, Option<String>>(1)?,
                        ))
                    },
                )
                .map_err(DatabaseError::Sqlite)?;
            if prior
                .0
                .as_ref()
                .zip(commit.7.as_ref())
                .is_some_and(|(left, right)| left != right)
                || prior
                    .1
                    .as_ref()
                    .zip(commit.8.as_ref())
                    .is_some_and(|(left, right)| left != right)
            {
                return Err(DatabaseError::AcquisitionCanonicalConflict(job_id));
            }
            transaction
                .execute(
                    "UPDATE recordings SET
                     musicbrainz_recording_id = COALESCE(musicbrainz_recording_id, ?2),
                     isrc = COALESCE(isrc, ?3) WHERE id = ?1",
                    rusqlite::params![recording_id, commit.7, commit.8],
                )
                .map_err(DatabaseError::Sqlite)?;
            recording_id
        } else {
            transaction
                .execute(
                    "INSERT INTO recordings(musicbrainz_recording_id, isrc) VALUES (?1, ?2)",
                    rusqlite::params![commit.7, commit.8],
                )
                .map_err(DatabaseError::Sqlite)?;
            transaction.last_insert_rowid()
        };
        transaction
            .execute(
                "UPDATE provider_items SET recording_id = ?2
                 WHERE id = ?1 AND recording_id IS NULL",
                rusqlite::params![commit.9, recording_id],
            )
            .map_err(DatabaseError::Sqlite)?;
        transaction
            .execute(
                "INSERT INTO artifacts(
                     recording_id, path, sha256, duration_ms, codec,
                     sample_rate_hz, channels, health)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'healthy')",
                rusqlite::params![
                    recording_id,
                    commit.1,
                    commit.2,
                    commit.4,
                    commit.3,
                    commit.5,
                    commit.6,
                ],
            )
            .map_err(DatabaseError::Sqlite)?;
        let artifact_id = transaction.last_insert_rowid();
        transaction
            .execute(
                "UPDATE recordings SET preferred_artifact_id = ?2
                 WHERE id = ?1 AND preferred_artifact_id IS NULL",
                rusqlite::params![recording_id, artifact_id],
            )
            .map_err(DatabaseError::Sqlite)?;
        transaction
            .execute(
                "UPDATE jobs SET status = 'succeeded', updated_at = CURRENT_TIMESTAMP
                 WHERE id = ?1 AND status = 'running'",
                [job_id],
            )
            .map_err(DatabaseError::Sqlite)?;
        transaction
            .execute(
                "UPDATE acquisition_commits SET status = 'committed',
                 committed_at = CURRENT_TIMESTAMP WHERE job_id = ?1",
                [job_id],
            )
            .map_err(DatabaseError::Sqlite)?;
        transaction.execute(
            "UPDATE discovery_candidates SET state='acquired',decision_reason='verified acquisition committed',updated_at=CURRENT_TIMESTAMP WHERE id=(SELECT candidate_id FROM discovery_acquisition_assertions WHERE provider_item_id=?1)",
            [commit.9],
        ).map_err(DatabaseError::Sqlite)?;
        transaction.commit().map_err(DatabaseError::Sqlite)?;
        Ok(ArtifactPersistenceResult {
            recording_id,
            artifact_id,
            inserted: true,
        })
    }

    /// Loads ordered active collections with only preferred healthy artifact paths.
    pub fn playlist_snapshots(&self) -> Result<Vec<PlaylistSnapshot>, DatabaseError> {
        let mut statement = self
            .connection
            .prepare("SELECT id, name FROM collections ORDER BY id")
            .map_err(DatabaseError::Sqlite)?;
        let rows = statement
            .query_map([], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(DatabaseError::Sqlite)?;
        let collections = rows
            .collect::<Result<Vec<_>, _>>()
            .map_err(DatabaseError::Sqlite)?;
        let mut snapshots = Vec::with_capacity(collections.len());
        for (collection_id, name) in collections {
            let active: u64 = self
                .connection
                .query_row(
                    "SELECT COUNT(*) FROM collection_memberships
                     WHERE collection_id = ?1 AND active = 1",
                    [collection_id],
                    |row| row.get(0),
                )
                .map_err(DatabaseError::Sqlite)?;
            let mut entries = self
                .connection
                .prepare(
                    "SELECT artifacts.path
                     FROM collection_memberships
                     JOIN provider_items ON provider_items.id =
                          collection_memberships.provider_item_id
                     JOIN recordings ON recordings.id = provider_items.recording_id
                     JOIN artifacts ON artifacts.id = recordings.preferred_artifact_id
                     WHERE collection_memberships.collection_id = ?1
                       AND collection_memberships.active = 1
                       AND artifacts.health = 'healthy'
                     ORDER BY collection_memberships.position,
                              collection_memberships.provider_item_id",
                )
                .map_err(DatabaseError::Sqlite)?;
            let rows = entries
                .query_map([collection_id], |row| row.get::<_, String>(0))
                .map_err(DatabaseError::Sqlite)?;
            let paths = rows
                .map(|row| row.map(PathBuf::from))
                .collect::<Result<Vec<_>, _>>()
                .map_err(DatabaseError::Sqlite)?;
            snapshots.push(PlaylistSnapshot {
                collection_id,
                name,
                unresolved: active.saturating_sub(paths.len() as u64),
                entries: paths,
            });
        }
        Ok(snapshots)
    }

    pub(crate) fn prepare_playlist_output(
        &mut self,
        collection_id: i64,
        path: &Path,
    ) -> Result<PlaylistOutputOwnership, DatabaseError> {
        let path_text = path
            .to_str()
            .ok_or_else(|| DatabaseError::NonUnicodePath(path.to_path_buf()))?;
        let transaction = self
            .connection
            .transaction()
            .map_err(DatabaseError::Sqlite)?;
        let inserted = transaction
            .execute(
                "INSERT OR IGNORE INTO playlist_outputs(collection_id, path)
                 VALUES (?1, ?2)",
                rusqlite::params![collection_id, path_text],
            )
            .map_err(DatabaseError::Sqlite)?
            == 1;
        let stored = transaction
            .query_row(
                "SELECT path, sha256 FROM playlist_outputs WHERE collection_id = ?1",
                [collection_id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
            )
            .optional()
            .map_err(DatabaseError::Sqlite)?
            .ok_or_else(|| DatabaseError::PlaylistPathReserved(path.to_path_buf()))?;
        if stored.0 != path_text {
            return Err(DatabaseError::PlaylistOutputMismatch(collection_id));
        }
        transaction.commit().map_err(DatabaseError::Sqlite)?;
        Ok(PlaylistOutputOwnership {
            newly_reserved: inserted,
            prior_sha256: stored.1,
        })
    }

    pub(crate) fn record_playlist_output(
        &mut self,
        collection_id: i64,
        sha256: &str,
    ) -> Result<(), DatabaseError> {
        let changed = self
            .connection
            .execute(
                "UPDATE playlist_outputs SET sha256 = ?2, updated_at = CURRENT_TIMESTAMP
                 WHERE collection_id = ?1",
                rusqlite::params![collection_id, sha256],
            )
            .map_err(DatabaseError::Sqlite)?;
        if changed != 1 {
            return Err(DatabaseError::PlaylistOutputMismatch(collection_id));
        }
        Ok(())
    }

    #[cfg(test)]
    fn table_count(&self, table: &str) -> Result<u64, DatabaseError> {
        let sql = format!("SELECT COUNT(*) FROM {table}");
        self.connection
            .query_row(&sql, [], |row| row.get(0))
            .map_err(DatabaseError::Sqlite)
    }

    fn configure(&self) -> Result<(), DatabaseError> {
        self.connection
            .execute_batch(
                "PRAGMA foreign_keys = ON;\n\
                 PRAGMA journal_mode = WAL;\n\
                 PRAGMA synchronous = NORMAL;\n\
                 PRAGMA busy_timeout = 5000;",
            )
            .map_err(DatabaseError::Sqlite)
    }

    fn migrate(&mut self) -> Result<(), DatabaseError> {
        let transaction = self
            .connection
            .transaction()
            .map_err(DatabaseError::Sqlite)?;
        let current = transaction
            .query_row("PRAGMA user_version", [], |row| row.get::<_, u32>(0))
            .map_err(DatabaseError::Sqlite)?;
        if current > CURRENT_SCHEMA_VERSION {
            return Err(DatabaseError::NewerSchema {
                found: current,
                supported: CURRENT_SCHEMA_VERSION,
            });
        }
        for (version, sql) in MIGRATIONS {
            if *version > current {
                apply_migration(&transaction, *version, sql)?;
            }
        }
        transaction.commit().map_err(DatabaseError::Sqlite)
    }
}

fn discovery_lane_budgets(maximum: u64, exploration_ratio: f64, wildcard_ratio: f64) -> (u64, u64) {
    let quota = |ratio: f64| {
        if maximum == 0 || ratio == 0.0 {
            0
        } else {
            ((maximum as f64) * ratio).round().max(1.0) as u64
        }
    };
    let mut exploration = quota(exploration_ratio).min(maximum);
    let wildcard = quota(wildcard_ratio).min(maximum.saturating_sub(exploration));
    if wildcard_ratio > 0.0 && wildcard == 0 && exploration > 0 {
        exploration -= 1;
        return (exploration, 1);
    }
    (exploration, wildcard)
}

fn sqlite_uri_path(path: &str) -> String {
    use std::fmt::Write;

    path.bytes()
        .fold(String::with_capacity(path.len()), |mut output, byte| {
            if byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'-' | b'_' | b'.' | b'~') {
                output.push(char::from(byte));
            } else {
                let _ = write!(output, "%{byte:02X}");
            }
            output
        })
}

fn open_immutable_current_schema(path: &Path) -> Result<Connection, DatabaseError> {
    let path_text = path
        .to_str()
        .ok_or_else(|| DatabaseError::NonUnicodePath(path.to_path_buf()))?;
    let uri = format!("file:{}?immutable=1", sqlite_uri_path(path_text));
    let connection = Connection::open_with_flags(
        uri,
        OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_URI,
    )
    .map_err(|source| DatabaseError::Open {
        path: path.to_path_buf(),
        source,
    })?;
    let version = connection
        .query_row("PRAGMA user_version", [], |row| row.get::<_, u32>(0))
        .map_err(DatabaseError::Sqlite)?;
    if version != CURRENT_SCHEMA_VERSION {
        return Err(DatabaseError::StatusSchemaMismatch {
            found: version,
            required: CURRENT_SCHEMA_VERSION,
        });
    }
    Ok(connection)
}

fn upsert_metadata_selection(
    transaction: &Transaction<'_>,
    recording_id: i64,
    field: &str,
    value: &str,
    source_entity_id: &str,
    resolution_context: &str,
) -> Result<u64, DatabaseError> {
    let inserted = transaction
        .execute(
            "INSERT OR IGNORE INTO metadata_observations(recording_id, field, value,
             source, source_entity_id, confidence_millionths, resolution_context)
         VALUES (?1, ?2, ?3, 'musicbrainz', ?4, 1000000, ?5)",
            rusqlite::params![
                recording_id,
                field,
                value,
                source_entity_id,
                resolution_context
            ],
        )
        .map_err(DatabaseError::Sqlite)? as u64;
    let observation_id = transaction
        .query_row(
            "SELECT id FROM metadata_observations WHERE recording_id = ?1 AND field = ?2
         AND source = 'musicbrainz' AND source_entity_id = ?3 AND value = ?4",
            rusqlite::params![recording_id, field, source_entity_id, value],
            |row| row.get::<_, i64>(0),
        )
        .map_err(DatabaseError::Sqlite)?;
    transaction
        .execute(
            "INSERT INTO metadata_selections(recording_id, field, observation_id)
         VALUES (?1, ?2, ?3) ON CONFLICT(recording_id, field) DO UPDATE SET
             observation_id = excluded.observation_id, selected_at = CURRENT_TIMESTAMP",
            rusqlite::params![recording_id, field, observation_id],
        )
        .map_err(DatabaseError::Sqlite)?;
    Ok(inserted)
}

fn source_collection_id(
    transaction: &Transaction<'_>,
    source_id: SourceId,
    snapshot: &SourceSnapshot,
    fallback_name: &str,
) -> Result<i64, DatabaseError> {
    if let Some(id) = transaction
        .query_row(
            "SELECT collection_id FROM source_collections WHERE source_id = ?1",
            [source_id.0],
            |row| row.get(0),
        )
        .optional()
        .map_err(DatabaseError::Sqlite)?
    {
        return Ok(id);
    }
    let provider_collection_id = snapshot
        .provider_collection_id
        .clone()
        .unwrap_or_else(|| format!("source:{}", source_id.0));
    let name = snapshot.title.as_deref().unwrap_or(fallback_name);
    transaction
        .execute(
            "INSERT INTO collections(provider, provider_collection_id, name) VALUES (?1, ?2, ?3)
             ON CONFLICT(provider, provider_collection_id) DO UPDATE SET name = excluded.name",
            rusqlite::params![snapshot.provider, provider_collection_id, name],
        )
        .map_err(DatabaseError::Sqlite)?;
    let collection_id = transaction
        .query_row(
            "SELECT id FROM collections WHERE provider = ?1 AND provider_collection_id = ?2",
            rusqlite::params![snapshot.provider, provider_collection_id],
            |row| row.get(0),
        )
        .map_err(DatabaseError::Sqlite)?;
    transaction
        .execute(
            "INSERT INTO source_collections(source_id, collection_id) VALUES (?1, ?2)",
            rusqlite::params![source_id.0, collection_id],
        )
        .map_err(DatabaseError::Sqlite)?;
    Ok(collection_id)
}

fn active_membership_ids(
    transaction: &Transaction<'_>,
    collection_id: i64,
) -> Result<BTreeSet<i64>, DatabaseError> {
    let mut statement = transaction
        .prepare(
            "SELECT provider_item_id FROM collection_memberships
             WHERE collection_id = ?1 AND active = 1",
        )
        .map_err(DatabaseError::Sqlite)?;
    let rows = statement
        .query_map([collection_id], |row| row.get(0))
        .map_err(DatabaseError::Sqlite)?;
    rows.collect::<Result<BTreeSet<_>, _>>()
        .map_err(DatabaseError::Sqlite)
}

/// Effects of committing one successful source snapshot.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
pub struct SnapshotReconciliationSummary {
    /// Newly inserted provider items.
    pub provider_items_inserted: u64,
    /// Existing provider items refreshed from the snapshot.
    pub provider_items_existing: u64,
    /// New or previously inactive memberships activated.
    pub memberships_activated: u64,
    /// Previously active memberships absent from the snapshot and deactivated.
    pub memberships_deactivated: u64,
    /// Memberships that remained active.
    pub memberships_unchanged: u64,
    /// New idempotent acquisition jobs created.
    pub jobs_created: u64,
}

/// One atomically claimed provider-item acquisition attempt.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct AcquisitionWork {
    /// Durable acquisition job ID used for staging and state transitions.
    pub job_id: i64,
    /// One-based attempt number after this claim.
    pub attempt: u32,
    /// Provider adapter name.
    pub provider: String,
    /// Provider-owned item identity; not a recording identity.
    pub provider_item_id: String,
    /// Auditable provider URL passed to the acquisition adapter.
    pub original_url: String,
    /// Provider title retained only as source metadata.
    pub source_title: Option<String>,
}

/// SQLite effects of finalizing one no-clobber artifact commit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct ArtifactPersistenceResult {
    /// Recording associated with the provider item.
    pub recording_id: i64,
    /// Healthy physical artifact row.
    pub artifact_id: i64,
    /// Whether this call inserted the artifact rather than observing committed state.
    pub inserted: bool,
}

/// Ordered active membership paths available for one collection playlist.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct PlaylistSnapshot {
    /// Durable collection ID used for stable output naming.
    pub collection_id: i64,
    /// Current user/provider-facing collection name.
    pub name: String,
    /// Preferred healthy artifacts in membership order.
    pub entries: Vec<PathBuf>,
    /// Active memberships not yet backed by a preferred healthy artifact.
    pub unresolved: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PlaylistOutputOwnership {
    pub(crate) newly_reserved: bool,
    pub(crate) prior_sha256: Option<String>,
}

/// Database effects of one explicit adoption apply.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
pub struct AdoptionPersistenceSummary {
    /// New unresolved recording rows inserted.
    pub recordings_inserted: u64,
    /// New artifact rows inserted.
    pub artifacts_inserted: u64,
    /// Artifact paths already present and left unchanged.
    pub artifacts_existing: u64,
}

fn apply_migration(
    transaction: &Transaction<'_>,
    version: u32,
    sql: &str,
) -> Result<(), DatabaseError> {
    transaction
        .execute_batch(sql)
        .map_err(|source| DatabaseError::Migration { version, source })?;
    let prior: Option<u32> = transaction
        .query_row(
            "SELECT version FROM schema_migrations WHERE version = ?1",
            [version],
            |row| row.get(0),
        )
        .optional()
        .map_err(DatabaseError::Sqlite)?;
    if prior.is_some() {
        return Err(DatabaseError::DuplicateMigration(version));
    }
    transaction
        .execute(
            "INSERT INTO schema_migrations(version) VALUES (?1)",
            [version],
        )
        .map_err(DatabaseError::Sqlite)?;
    transaction
        .pragma_update(None, "user_version", version)
        .map_err(DatabaseError::Sqlite)?;
    Ok(())
}

/// Result of a read-only database inspection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DatabaseInspection {
    /// Schema version declared by SQLite.
    pub version: u32,
}

/// Read-only durable operational summary.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct OperationalStatus {
    /// Durable schema version read from SQLite.
    pub schema_version: u32,
    /// Configured sources eligible for synchronization.
    pub active_sources: u64,
    /// Preserved but inactive configured sources.
    pub inactive_sources: u64,
    /// Durable provider collections.
    pub collections: u64,
    /// Currently active collection memberships.
    pub active_memberships: u64,
    /// Active memberships lacking a preferred healthy artifact.
    pub unresolved_active_memberships: u64,
    /// Durable job counts by constrained state.
    pub jobs: JobStatusCounts,
    /// Physical artifact counts by constrained health.
    pub artifacts: ArtifactHealthCounts,
    /// Repair cases grouped by their conservative durable state.
    pub repairs: RepairCaseCounts,
    /// Repair candidate attempts grouped by their constrained state.
    pub repair_attempts: RepairAttemptCounts,
    /// Canonical metadata resolution and selected-field counts.
    pub metadata: MetadataResolutionCounts,
    /// Release artwork resolution and immutable-cache counts.
    pub artwork: ArtworkResolutionCounts,
    /// Lyrics resolution and committed adjacent-output counts.
    pub lyrics: LyricsResolutionCounts,
    /// Source-preserving canonical tag materialization counts.
    pub metadata_materializations: MetadataMaterializationCounts,
    /// Autonomous discovery decisions and routing counts.
    pub discovery: DiscoveryCounts,
    /// Playlist outputs with committed exact-byte evidence.
    pub playlist_outputs: u64,
    /// Bounded newest-first warning/error events.
    pub recent_events: Vec<OperationalEvent>,
}

/// Durable job counts by state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct JobStatusCounts {
    /// Jobs not yet claimed.
    pub pending: u64,
    /// Jobs claimed by an incomplete process.
    pub running: u64,
    /// Successfully completed jobs.
    pub succeeded: u64,
    /// Terminally failed jobs.
    pub failed: u64,
    /// Retryable jobs deferred after an ordinary failure.
    pub deferred: u64,
}

/// Physical artifact counts by health state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct ArtifactHealthCounts {
    /// Artifacts not yet health-reconciled.
    pub unknown: u64,
    /// Structurally accepted local artifacts.
    pub healthy: u64,
    /// Artifacts recorded but absent from storage.
    pub missing: u64,
    /// Artifacts known to be structurally corrupt.
    pub corrupt: u64,
}

/// Durable repair-case counts by constrained state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct RepairCaseCounts {
    /// Cases satisfying every persisted prerequisite for candidate generation.
    pub eligible: u64,
    /// Cases retaining insufficient candidate evidence.
    pub unresolved: u64,
    /// Cases with an independently verified candidate awaiting or completing commit.
    pub verified: u64,
    /// Cases made ineligible by recovered health, membership, or availability.
    pub cancelled: u64,
}

/// Durable repair-attempt counts by constrained execution/decision state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct RepairAttemptCounts {
    /// Search-generated candidates awaiting claim.
    pub generated: u64,
    /// Exclusively claimed candidates.
    pub running: u64,
    /// Retryable attempts retained after infrastructure failure.
    pub deferred: u64,
    /// Candidates contradicting known evidence.
    pub rejected: u64,
    /// Candidates with insufficient evidence.
    pub unresolved: u64,
    /// Independently verified candidates awaiting commit.
    pub verified: u64,
    /// Verified candidates committed as healthy preferred artifacts.
    pub committed: u64,
}

/// Durable canonical metadata resolution counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct MetadataResolutionCounts {
    /// Explicitly pending recording resolutions.
    pub pending: u64,
    /// Unambiguously resolved recordings.
    pub resolved: u64,
    /// Strong identifiers with ambiguous provider results.
    pub ambiguous: u64,
    /// Retryable provider failures.
    pub deferred: u64,
    /// Explicit field selections backed by provenance observations.
    pub selected_fields: u64,
}

/// Durable release-artwork and immutable-cache counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct ArtworkResolutionCounts {
    /// Releases with selected cached art.
    pub resolved: u64,
    /// Releases for which the provider reported no usable art.
    pub unavailable: u64,
    /// Releases awaiting retry after an ordinary failure.
    pub deferred: u64,
    /// Unique content-addressed image blobs.
    pub cached_blobs: u64,
}

/// Durable lyrics resolution and sidecar counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct LyricsResolutionCounts {
    /// Recordings with selected text.
    pub resolved: u64,
    /// Recordings explicitly identified as instrumental.
    pub instrumental: u64,
    /// Recordings without provider lyrics.
    pub unavailable: u64,
    /// Recordings awaiting retry.
    pub deferred: u64,
    /// Atomically committed owned adjacent sidecars.
    pub committed_outputs: u64,
}

/// Durable canonical tag materialization counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct MetadataMaterializationCounts {
    /// Validated output awaiting or recovering filesystem commit.
    pub prepared: u64,
    /// Committed derived artifacts with retained source history.
    pub committed: u64,
    /// Failures excluded until explicit retry.
    pub deferred: u64,
}

/// Durable autonomous-discovery counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct DiscoveryCounts {
    /// Approved canonical recommendations.
    pub approved: u64,
    /// Candidates queued through verified acquisition routing.
    pub queued: u64,
    /// Candidates lacking sufficient acquisition evidence.
    pub unresolved: u64,
    /// Successfully acquired discovery recordings.
    pub acquired: u64,
    /// Candidates rejected by hard budgets.
    pub budget_rejected: u64,
}

/// Transactional effects of one discovery run.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct DiscoveryPersistenceReport {
    /// Durable run ID.
    pub run_id: i64,
    /// Approved candidates.
    pub approved: u64,
    /// Exact canonical duplicates.
    pub duplicates: u64,
    /// Hard-budget rejections.
    pub budget_rejected: u64,
}

/// Result of importing read-only taste signals.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct DiscoverySeedReport {
    /// Signals exactly matched to local canonical recordings.
    pub matched: u64,
    /// Signals without an existing local canonical recording.
    pub unmatched: u64,
}

/// Approved canonical candidate awaiting provider relationship evidence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovedDiscoveryCandidate {
    /// Durable candidate ID.
    pub id: i64,
    /// Exact MusicBrainz recording MBID.
    pub recording_mbid: String,
}

/// Strong recording-level provider route ready for durable acquisition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveryAcquisitionRoute {
    /// Provider-owned YouTube video ID.
    pub provider_item_id: String,
    /// Exact relationship URL.
    pub url: String,
    /// Canonical duration required for staged verification.
    pub canonical_duration_ms: u64,
    /// Unique canonical ISRC when available.
    pub canonical_isrc: Option<String>,
    /// Auditable bounded relationship evidence.
    pub assertion_json: String,
}

/// One bounded persisted warning or error event.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct OperationalEvent {
    /// SQLite creation timestamp.
    pub created_at: String,
    /// Persisted event severity.
    pub level: String,
    /// Emitting subsystem.
    pub component: String,
    /// Stable event name.
    pub event: String,
    /// Human-readable diagnostic.
    pub message: String,
    /// Related synchronization run when present.
    pub run_id: Option<i64>,
    /// Related durable job when present.
    pub job_id: Option<i64>,
}

/// Origin of one service-cycle invocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceRunTrigger {
    /// Interactive/operator invocation.
    Manual,
    /// systemd timer invocation.
    Timer,
    /// Controlled crash-recovery validation.
    RecoveryTest,
}
impl ServiceRunTrigger {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Manual => "manual",
            Self::Timer => "timer",
            Self::RecoveryTest => "recovery_test",
        }
    }
}

/// Terminal service-cycle state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceRunTerminalStatus {
    /// Every phase succeeded.
    Succeeded,
    /// Useful work committed but one or more phases failed.
    Partial,
    /// Fatal cycle failure.
    Failed,
}
impl ServiceRunTerminalStatus {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Succeeded => "succeeded",
            Self::Partial => "partial",
            Self::Failed => "failed",
        }
    }
}

/// Terminal state of one service phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServicePhaseStatus {
    /// Phase succeeded.
    Succeeded,
    /// Phase committed partial useful work.
    Partial,
    /// Phase failed.
    Failed,
    /// Dependency or configuration deliberately skipped the phase.
    Skipped,
}
impl ServicePhaseStatus {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Succeeded => "succeeded",
            Self::Partial => "partial",
            Self::Failed => "failed",
            Self::Skipped => "skipped",
        }
    }
}

/// Optional run-history terminal-state filter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceRunHistoryStatus {
    /// Currently active.
    Running,
    /// Successful.
    Succeeded,
    /// Partial.
    Partial,
    /// Failed.
    Failed,
    /// Explicitly recovered interruption.
    Interrupted,
}
impl ServiceRunHistoryStatus {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Succeeded => "succeeded",
            Self::Partial => "partial",
            Self::Failed => "failed",
            Self::Interrupted => "interrupted",
        }
    }
}

/// Read-only durable service-cycle history row.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ServiceRunHistoryEntry {
    /// Durable ID.
    pub id: i64,
    /// Invocation origin.
    pub trigger: String,
    /// Durable state.
    pub status: String,
    /// Producing binary version.
    pub binary_version: String,
    /// Start timestamp.
    pub started_at: String,
    /// Terminal timestamp.
    pub finished_at: Option<String>,
    /// Exact summary JSON.
    pub summary_json: String,
    /// Number of phases.
    pub phase_count: u64,
    /// Failed or partial phases.
    pub failed_phase_count: u64,
}

/// One ordered phase in read-only service-cycle history.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ServiceRunPhaseHistoryEntry {
    /// Stable phase name.
    pub phase: String,
    /// Execution ordinal.
    pub ordinal: u32,
    /// Durable phase state.
    pub status: String,
    /// Start timestamp.
    pub started_at: String,
    /// Terminal timestamp.
    pub finished_at: Option<String>,
    /// Exact summary JSON.
    pub summary_json: String,
    /// Optional bounded diagnostic.
    pub message: Option<String>,
}

/// One service cycle with ordered phase evidence.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ServiceRunDetail {
    /// Cycle summary.
    pub run: ServiceRunHistoryEntry,
    /// Ordered phase summaries.
    pub phases: Vec<ServiceRunPhaseHistoryEntry>,
}

/// One bounded read-only acquisition history entry.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct AcquisitionHistoryEntry {
    /// Durable acquisition job ID.
    pub job_id: i64,
    /// Current constrained job state.
    pub status: String,
    /// Number of actual claims attempted.
    pub attempt_count: u32,
    /// SQLite job creation timestamp.
    pub created_at: String,
    /// SQLite last transition timestamp.
    pub updated_at: String,
    /// Provider adapter name.
    pub provider: String,
    /// Provider-owned item identity.
    pub provider_item_id: String,
    /// Auditable original provider URL.
    pub original_url: String,
    /// Newest persisted event message when present.
    pub latest_message: Option<String>,
}

/// Registered artifact selected for a local health check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactHealthCandidate {
    /// Durable artifact ID.
    pub id: i64,
    /// Registered absolute media path.
    pub path: PathBuf,
    /// Prior exact-byte hash when known.
    pub sha256: Option<String>,
    /// Previously persisted health.
    pub health: ArtifactHealth,
}

/// Constrained physical artifact health.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactHealth {
    /// Health has not yet been reconciled.
    Unknown,
    /// Local bytes passed structural and integrity checks.
    Healthy,
    /// Registered path is absent.
    Missing,
    /// Local bytes contradict persisted integrity evidence or are not a regular file.
    Corrupt,
}

impl ArtifactHealth {
    fn parse(value: &str) -> Result<Self, DatabaseError> {
        match value {
            "unknown" => Ok(Self::Unknown),
            "healthy" => Ok(Self::Healthy),
            "missing" => Ok(Self::Missing),
            "corrupt" => Ok(Self::Corrupt),
            unexpected => Err(DatabaseError::InvalidArtifactHealth(unexpected.into())),
        }
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::Healthy => "healthy",
            Self::Missing => "missing",
            Self::Corrupt => "corrupt",
        }
    }
}

/// Persistable evidence from one completed artifact health check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactHealthObservation {
    /// Durable checked artifact ID.
    pub artifact_id: i64,
    /// Conservatively determined health.
    pub health: ArtifactHealth,
    /// Exact current SHA-256 when successfully hashed.
    pub sha256: Option<String>,
    /// Structurally observed duration.
    pub duration_ms: Option<i64>,
    /// Structurally observed codec.
    pub codec: Option<String>,
    /// Structurally observed sample rate.
    pub sample_rate_hz: Option<i64>,
    /// Structurally observed channel count.
    pub channels: Option<i64>,
}

/// Healthy artifact selected for bounded fingerprint reconciliation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactFingerprintCandidate {
    /// Durable artifact ID.
    pub artifact_id: i64,
    /// Registered media path.
    pub path: PathBuf,
    /// Prior extraction bound when fingerprinted.
    pub prior_max_seconds: Option<i64>,
    /// Prior raw fingerprint JSON when present.
    pub prior_fingerprint_json: Option<String>,
}

/// Persistable raw Chromaprint evidence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactFingerprintEvidence {
    /// Durable artifact ID.
    pub artifact_id: i64,
    /// Algorithm-2 extraction audio bound.
    pub max_seconds: u32,
    /// fpcalc duration in milliseconds.
    pub duration_ms: i64,
    /// JSON array containing raw unsigned fingerprint values.
    pub fingerprint_json: String,
    /// Number of raw values.
    pub value_count: i64,
}

/// Healthy recording with strong identity selected for canonical metadata lookup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetadataResolutionCandidate {
    /// Durable canonical recording row.
    pub recording_id: i64,
    /// Exact MusicBrainz recording ID when already known.
    pub musicbrainz_recording_id: Option<String>,
    /// Normalized embedded ISRC fallback when MBID is absent.
    pub isrc: Option<String>,
}

/// Strong lookup method used for one durable resolution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetadataResolutionMethod {
    /// Exact recording MBID lookup.
    RecordingMbid,
    /// Unique recording returned for normalized ISRC.
    Isrc,
}

impl MetadataResolutionMethod {
    const fn as_str(self) -> &'static str {
        match self {
            Self::RecordingMbid => "recording_mbid",
            Self::Isrc => "isrc",
        }
    }
}

/// Non-destructive durable canonical resolution state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetadataResolutionState {
    /// Awaiting a provider result.
    Pending,
    /// Canonical metadata was selected transactionally.
    Resolved,
    /// Strong identifier mapped to multiple recordings.
    Ambiguous,
    /// Provider or infrastructure failure requires a later explicit retry.
    Deferred,
}

impl MetadataResolutionState {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Resolved => "resolved",
            Self::Ambiguous => "ambiguous",
            Self::Deferred => "deferred",
        }
    }
}

/// Transactional canonical metadata persistence effects.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct MetadataPersistenceSummary {
    /// Ordered artist-credit components persisted.
    pub artists: u64,
    /// Whether one deterministic release was selected.
    pub release_selected: bool,
    /// New field observations selected; repeat may be zero.
    pub observations_selected: u64,
}

/// Canonically selected release awaiting artwork resolution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtworkResolutionCandidate {
    /// Durable release row ID.
    pub release_id: i64,
    /// Exact MusicBrainz release MBID.
    pub musicbrainz_release_id: String,
}

/// Non-destructive durable artwork resolution state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtworkResolutionState {
    /// A validated immutable blob was selected.
    Resolved,
    /// The provider has no usable image for this release.
    Unavailable,
    /// Provider or infrastructure failure requires a later retry.
    Deferred,
}

impl ArtworkResolutionState {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Resolved => "resolved",
            Self::Unavailable => "unavailable",
            Self::Deferred => "deferred",
        }
    }
}

/// Auditable provider selection persisted for a release.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedArtwork {
    /// Provider-owned image identity.
    pub source_image_id: String,
    /// Exact selected download URL.
    pub source_url: String,
    /// Normalized front, back, or other role.
    pub role: String,
    /// Provider approval flag.
    pub approved: bool,
}

/// Validated content-addressed image committed to state storage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtworkBlob {
    /// Lowercase hexadecimal SHA-256.
    pub sha256: String,
    /// State-directory-relative immutable cache path.
    pub relative_path: String,
    /// Magic-byte-derived media type.
    pub mime_type: String,
    /// Exact image length.
    pub byte_count: i64,
}

/// Managed recording ready for lyrics lookup or prepared-output recovery.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LyricsWorkCandidate {
    /// Durable recording row.
    pub recording_id: i64,
    /// Preferred healthy managed artifact.
    pub artifact_id: i64,
    /// Absolute audio path.
    pub artifact_path: PathBuf,
    /// Structurally probed audio duration.
    pub duration_ms: i64,
    /// Selected canonical title.
    pub title: String,
    /// Selected canonical artist credit.
    pub artist_credit: String,
    /// Selected canonical release title.
    pub release_title: String,
    /// Existing selected lyrics observation, when resolution already succeeded.
    pub observation_id: Option<i64>,
    /// Existing selected synchronized/plain kind.
    pub selected_kind: Option<String>,
    /// Existing selected content.
    pub selected_content: Option<String>,
    /// Whether durable output intent predates this run.
    pub output_prepared: bool,
}

/// Non-destructive durable lyrics resolution state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LyricsResolutionState {
    /// Validated text selected.
    Resolved,
    /// Provider-confirmed instrumental.
    Instrumental,
    /// No current provider record.
    Unavailable,
    /// Retryable failure.
    Deferred,
}

impl LyricsResolutionState {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Resolved => "resolved",
            Self::Instrumental => "instrumental",
            Self::Unavailable => "unavailable",
            Self::Deferred => "deferred",
        }
    }
}

/// Validated provider lyrics and complete provenance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedLyrics {
    /// LRCLIB record ID.
    pub provider_entity_id: String,
    /// `synchronized` or `plain`.
    pub kind: String,
    /// Validated nonempty lyrics text.
    pub content: String,
    /// Exact canonical request signature JSON.
    pub signature_json: String,
    /// Exact bounded response JSON.
    pub raw_response_json: String,
}

/// Healthy owned artifact and selected canonical fields for stream-copy tagging.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetadataMaterializationCandidate {
    /// Durable recording row.
    pub recording_id: i64,
    /// Current preferred source artifact.
    pub source_artifact_id: i64,
    /// Visible source path to replace only after archival.
    pub source_path: PathBuf,
    /// Persisted exact source bytes.
    pub source_sha256: String,
    /// Source audio codec.
    pub codec: Option<String>,
    /// Source duration.
    pub duration_ms: Option<i64>,
    /// Source sample rate.
    pub sample_rate_hz: Option<i64>,
    /// Source channel count.
    pub channels: Option<i64>,
    /// Selected title.
    pub title: String,
    /// Selected artist credit.
    pub artist_credit: String,
    /// Selected release title.
    pub release_title: String,
    /// Selected release date.
    pub release_date: Option<String>,
    /// Strong recording MBID when known.
    pub musicbrainz_recording_id: Option<String>,
    /// Normalized ISRC when known.
    pub isrc: Option<String>,
    /// Whether durable commit intent already exists.
    pub prepared: bool,
    /// Selected canonical release-art cache path relative to application state.
    pub artwork_relative_path: Option<String>,
    /// Magic-byte-derived selected artwork MIME type.
    pub artwork_mime_type: Option<String>,
    /// Whether the deterministic hidden staging path is durably owned.
    pub staging_reserved: bool,
}

/// Complete validated stream-copy output intent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetadataMaterializationIntent {
    /// Immutable content-addressed original-byte history path.
    pub history_path: PathBuf,
    /// Hidden validated output path beside the final artifact.
    pub staged_path: PathBuf,
    /// Independently validated output evidence.
    pub validated: ValidatedStagedMedia,
    /// Exact selected-field snapshot used for ffmpeg arguments.
    pub canonical_snapshot_json: String,
}

/// Original provider object whose active recording artifact needs repair assessment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepairOriginalCandidate {
    /// Durable provider-item row ID.
    pub provider_item_id: i64,
    /// Provider adapter name.
    pub provider: String,
    /// Provider-owned object ID.
    pub provider_owned_id: String,
    /// Auditable original URL checked directly.
    pub original_url: String,
    /// Previously persisted availability.
    pub availability: ProviderAvailability,
    /// Preferred reference artifact retaining identity evidence.
    pub artifact_id: i64,
    /// Missing or corrupt local state.
    pub artifact_health: ArtifactHealth,
}

/// Constrained provider availability persisted independently from artifact health.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderAvailability {
    /// Availability has not been checked.
    Unknown,
    /// The original provider object is usable.
    Available,
    /// Evidence is insufficient due to a retryable or infrastructure failure.
    TransientFailure,
    /// The provider explicitly and definitively reported permanent loss.
    PermanentlyUnavailable,
}

impl ProviderAvailability {
    fn parse(value: &str) -> Result<Self, DatabaseError> {
        match value {
            "unknown" => Ok(Self::Unknown),
            "available" => Ok(Self::Available),
            "transient_failure" => Ok(Self::TransientFailure),
            "permanently_unavailable" => Ok(Self::PermanentlyUnavailable),
            unexpected => Err(DatabaseError::InvalidProviderAvailability(
                unexpected.into(),
            )),
        }
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::Available => "available",
            Self::TransientFailure => "transient_failure",
            Self::PermanentlyUnavailable => "permanently_unavailable",
        }
    }
}

/// Durable effects from one bounded repair-eligibility reconciliation.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
pub struct RepairEligibilitySummary {
    /// Newly created eligible cases.
    pub inserted: u64,
    /// Previously cancelled cases made eligible again.
    pub reopened: u64,
    /// Existing active cases whose eligibility remains unchanged.
    pub unchanged: u64,
    /// Cases cancelled because a safety prerequisite no longer holds.
    pub cancelled: u64,
}

/// Eligible repair case retaining canonical, duration, and fingerprint references.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EligibleRepairCase {
    /// Durable repair-case ID.
    pub case_id: i64,
    /// Canonical recording row targeted by repair.
    pub recording_id: i64,
    /// Definitively unavailable original provider row.
    pub original_provider_item_id: i64,
    /// Provider title usable only for candidate generation.
    pub source_title: Option<String>,
    /// Optional canonical MusicBrainz recording identity.
    pub musicbrainz_recording_id: Option<String>,
    /// Optional canonical ISRC evidence.
    pub isrc: Option<String>,
    /// Structural duration from the reference artifact.
    pub reference_duration_ms: Option<i64>,
    /// Audio seconds used for reference fingerprint extraction.
    pub fingerprint_max_seconds: i64,
    /// Duration reported alongside the retained reference fingerprint.
    pub fingerprint_duration_ms: i64,
    /// Serialized raw algorithm-2 fingerprint values.
    pub fingerprint_json: String,
}

/// Exclusively claimed generated repair candidate plus retained reference evidence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepairAttemptWork {
    /// Durable attempt ID and staging namespace.
    pub attempt_id: i64,
    /// Parent repair-case ID.
    pub case_id: i64,
    /// Recording whose preferred artifact may be repaired.
    pub recording_id: i64,
    /// Definitively unavailable original provider row.
    pub original_provider_item_id: i64,
    /// Candidate provider adapter.
    pub candidate_provider: String,
    /// Provider-owned candidate identity.
    pub candidate_provider_item_id: String,
    /// Auditable candidate download URL.
    pub candidate_url: String,
    /// Monotonic claim attempt number.
    pub attempt: u32,
    /// Reference canonical MBID when known.
    pub reference_musicbrainz_recording_id: Option<String>,
    /// Reference canonical ISRC when known.
    pub reference_isrc: Option<String>,
    /// Reference structural duration when known.
    pub reference_duration_ms: Option<i64>,
    /// Extraction bound of the retained reference fingerprint.
    pub reference_fingerprint_max_seconds: i64,
    /// Duration reported with the retained reference fingerprint.
    pub reference_fingerprint_duration_ms: i64,
    /// Serialized retained raw fingerprint values.
    pub reference_fingerprint_json: String,
}

/// Persistable independently derived staged repair evidence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepairVerificationEvidence {
    /// Claimed durable repair attempt.
    pub attempt_id: i64,
    /// Validated staged candidate path.
    pub staged_path: PathBuf,
    /// Exact candidate bytes SHA-256.
    pub sha256: String,
    /// Exact stable candidate byte count.
    pub bytes: u64,
    /// Candidate audio codec.
    pub codec: String,
    /// Candidate structural duration.
    pub duration_ms: Option<i64>,
    /// Candidate audio sample rate.
    pub sample_rate_hz: Option<i64>,
    /// Candidate channel count.
    pub channels: Option<i64>,
    /// Candidate embedded canonical MBID.
    pub musicbrainz_recording_id: Option<String>,
    /// Candidate embedded canonical ISRC.
    pub isrc: Option<String>,
    /// Candidate fingerprint extraction bound.
    pub fingerprint_max_seconds: i64,
    /// Candidate fingerprint duration.
    pub fingerprint_duration_ms: i64,
    /// Serialized raw candidate fingerprint.
    pub fingerprint_json: String,
    /// Conservative verification result.
    pub decision: RepairVerificationDecision,
    /// Auditable reason for the decision.
    pub reason: String,
}

/// Verified staged candidate loaded for a recoverable no-clobber commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepairCommitWork {
    /// Durable verified attempt.
    pub attempt_id: i64,
    /// Parent repair case.
    pub case_id: i64,
    /// Target canonical recording.
    pub recording_id: i64,
    /// Verified candidate provider.
    pub candidate_provider: String,
    /// Verified candidate provider identity.
    pub candidate_provider_item_id: String,
    /// Auditable candidate URL.
    pub candidate_url: String,
    /// Stored raw candidate provider metadata.
    pub candidate_metadata_json: String,
    /// Exact validated staged media evidence.
    pub validated: ValidatedStagedMedia,
    /// Candidate fingerprint extraction bound.
    pub fingerprint_max_seconds: i64,
    /// Candidate fingerprint duration.
    pub fingerprint_duration_ms: i64,
    /// Candidate raw fingerprint JSON.
    pub fingerprint_json: String,
}

/// Transactional database effect of finalizing a repair commit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct RepairCommitPersistence {
    /// New healthy replacement artifact ID.
    pub artifact_id: i64,
    /// Whether this call inserted it rather than observing committed recovery.
    pub inserted: bool,
}

/// Durable terminal result of independently checking one candidate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepairVerificationDecision {
    /// Candidate contradicts known identity or version evidence.
    Rejected,
    /// Candidate evidence is insufficient for a safe decision.
    Unresolved,
    /// Candidate satisfies canonical, duration, and perceptual checks.
    Verified,
}

impl RepairVerificationDecision {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Rejected => "rejected",
            Self::Unresolved => "unresolved",
            Self::Verified => "verified",
        }
    }
}

/// Persistence initialization or migration failure.
#[derive(Debug, Error)]
pub enum DatabaseError {
    /// A backup never replaces an earlier snapshot.
    #[error("database backup already exists: {0}")]
    BackupAlreadyExists(PathBuf),
    /// Backup storage must be provisioned explicitly by the operator.
    #[error("database backup parent directory does not exist: {0}")]
    BackupParentMissing(PathBuf),
    /// SQLite's VACUUM INTO interface requires a Unicode destination.
    #[error("database backup path is not valid UTF-8: {0}")]
    BackupPathNotUtf8(PathBuf),
    /// A database path did not have a parent directory.
    #[error("database path has no parent: {path}")]
    MissingParent {
        /// Invalid database path.
        path: PathBuf,
    },
    /// The state directory could not be created.
    #[error("failed to create state directory {path}: {source}")]
    CreateDirectory {
        /// Directory path.
        path: PathBuf,
        /// Underlying I/O error.
        source: std::io::Error,
    },
    /// SQLite could not open the database.
    #[error("failed to open database {path}: {source}")]
    Open {
        /// Database path.
        path: PathBuf,
        /// Underlying SQLite error.
        source: rusqlite::Error,
    },
    /// SQLite operation failed.
    #[error("SQLite operation failed: {0}")]
    Sqlite(rusqlite::Error),
    /// A migration failed and was rolled back.
    #[error("database migration {version} failed: {source}")]
    Migration {
        /// Migration version.
        version: u32,
        /// Underlying SQLite error.
        source: rusqlite::Error,
    },
    /// The database was written by a newer application.
    #[error("database schema {found} is newer than supported schema {supported}")]
    NewerSchema {
        /// Version found on disk.
        found: u32,
        /// Highest version supported by this build.
        supported: u32,
    },
    /// Read-only status requires the current schema because it never migrates state.
    #[error("database schema {found} does not match status schema {required}")]
    StatusSchemaMismatch {
        /// Version found on disk.
        found: u32,
        /// Version required by this build's status queries.
        required: u32,
    },
    /// Migration bookkeeping was inconsistent.
    #[error("migration {0} was already recorded")]
    DuplicateMigration(u32),
    /// SQLite integrity check failed.
    #[error("database integrity check failed: {0}")]
    Integrity(String),
    /// SQLite text cannot losslessly represent an artifact path.
    #[error("artifact path is not valid Unicode and cannot be persisted: {0:?}")]
    NonUnicodePath(PathBuf),
    /// The requested configured source does not exist.
    #[error("configured source does not exist: {}", .0.0)]
    SourceNotFound(SourceId),
    /// Inactive sources cannot be reconciled.
    #[error("configured source is inactive: {}", .0.0)]
    SourceInactive(SourceId),
    /// Snapshot provider did not match the configured source provider.
    #[error("snapshot provider {snapshot} does not match configured provider {configured}")]
    ProviderMismatch {
        /// Configured provider.
        configured: String,
        /// Snapshot provider.
        snapshot: String,
    },
    /// Raw provider metadata could not be serialized.
    #[error("failed to serialize provider metadata: {0}")]
    Json(serde_json::Error),
    /// Validated artifact byte counts exceeded SQLite's signed integer range.
    #[error("artifact value exceeds SQLite integer range: {0}")]
    ArtifactTooLarge(u64),
    /// An artifact ID did not exist during health persistence.
    #[error("artifact {0} does not exist")]
    ArtifactNotFound(i64),
    /// Durable artifact health violated the constrained schema vocabulary.
    #[error("invalid durable artifact health: {0}")]
    InvalidArtifactHealth(String),
    /// Durable provider availability violated the constrained schema vocabulary.
    #[error("invalid durable provider availability: {0}")]
    InvalidProviderAvailability(String),
    /// A provider item did not exist during availability persistence.
    #[error("provider item {0} does not exist")]
    ProviderItemNotFound(i64),
    /// A repair case did not exist during candidate persistence.
    #[error("repair case {0} does not exist")]
    RepairCaseNotFound(i64),
    /// Candidate generation cannot mutate a terminal or cancelled repair case.
    #[error("repair case {0} is not eligible for candidate generation")]
    RepairCaseNotEligible(i64),
    /// Repair verification requires an exclusively running attempt.
    #[error("repair attempt {0} is not running")]
    RepairAttemptNotRunning(i64),
    /// Repair commit requires a verified attempt and evidence.
    #[error("repair attempt {0} is not verified")]
    RepairAttemptNotVerified(i64),
    /// Repeated repair preparation supplied a different final path.
    #[error("prepared repair commit does not match attempt {0}")]
    RepairCommitMismatch(i64),
    /// Repair finalization requires matching prepared intent.
    #[error("repair commit is not prepared for verified attempt {0}")]
    RepairCommitNotPrepared(i64),
    /// A provider candidate already belongs to another canonical recording.
    #[error(
        "repair candidate {provider}:{provider_item_id} is already associated with another recording"
    )]
    RepairCandidateIdentityConflict {
        /// Candidate provider adapter.
        provider: String,
        /// Provider-owned candidate identity.
        provider_item_id: String,
    },
    /// Metadata state-only persistence cannot claim a resolved transition.
    #[error("resolved metadata requires canonical observation persistence")]
    InvalidMetadataResolutionTransition,
    /// Artwork state-only persistence cannot claim a resolved transition.
    #[error("resolved artwork requires validated blob persistence")]
    InvalidArtworkResolutionTransition,
    /// Lyrics state-only persistence cannot claim a resolved transition.
    #[error("resolved lyrics require validated observation persistence")]
    InvalidLyricsResolutionTransition,
    /// Repeated sidecar preparation contradicted durable intent.
    #[error("prepared lyrics output does not match recording {0}")]
    LyricsOutputMismatch(i64),
    /// Sidecar finalization requires matching prepared intent.
    #[error("lyrics output is not prepared for recording {0}")]
    LyricsOutputNotPrepared(i64),
    /// Repeated hidden staging reservation contradicted its durable path.
    #[error("metadata staging path does not match recording {0}")]
    MetadataStagingMismatch(i64),
    /// Discovery counts exceeded SQLite integer representation.
    #[error("discovery count exceeds SQLite range")]
    DiscoveryCountTooLarge,
    /// Discovery candidate was no longer approved when routing attempted.
    #[error("discovery candidate {0} is not approved")]
    DiscoveryCandidateNotApproved(i64),
    /// Discovery staging lacked exact canonical identity or compatible duration.
    #[error("discovery acquisition evidence does not match assertion for job {0}")]
    DiscoveryAcquisitionEvidenceMismatch(i64),
    /// Existing provider identity belongs to another canonical recording.
    #[error("provider item already belongs to another canonical recording")]
    ProviderItemRecordingConflict,
    /// A service cycle is already active.
    #[error("service run {0} is already running")]
    ServiceRunAlreadyRunning(i64),
    /// Requested service cycle is absent or terminal.
    #[error("service run {0} is not running")]
    ServiceRunNotRunning(i64),
    /// Binary version must be auditable.
    #[error("service run binary version must not be empty")]
    InvalidServiceRunVersion,
    /// Phase name must be stable and nonempty.
    #[error("service phase name must not be empty")]
    InvalidServicePhase,
    /// Requested phase is absent or terminal.
    #[error("service phase {phase} in run {run_id} is not running")]
    ServicePhaseNotRunning {
        /// Owning service cycle.
        run_id: i64,
        /// Stable phase name.
        phase: String,
    },
    /// A cycle cannot finish while a phase is active.
    #[error("service run {0} still has running phases")]
    ServiceRunHasRunningPhases(i64),
    /// Metadata provider candidate count exceeded durable representation.
    #[error("metadata candidate count exceeds SQLite range")]
    MetadataCandidateCountTooLarge,
    /// Canonical provider evidence contradicted the target recording identity.
    #[error("canonical metadata conflicts with recording {0}")]
    MetadataIdentityConflict(i64),
    /// Target recording did not exist during canonical metadata persistence.
    #[error("recording {0} does not exist")]
    RecordingNotFound(i64),
    /// Commit preparation requires a currently running acquisition.
    #[error("acquisition job is not running: {0}")]
    AcquisitionNotRunning(i64),
    /// Another prepared job already reserved the intended artifact path.
    #[error("artifact destination is already reserved: {0}")]
    ArtifactPathReserved(PathBuf),
    /// Repeated preparation supplied different evidence or paths.
    #[error("prepared acquisition commit does not match job {0}")]
    AcquisitionCommitMismatch(i64),
    /// Finalization requires matching prepared intent and a running job.
    #[error("acquisition commit is not prepared for running job {0}")]
    AcquisitionCommitNotPrepared(i64),
    /// Embedded canonical evidence contradicted an already associated recording.
    #[error("acquisition canonical evidence conflicts with recording for job {0}")]
    AcquisitionCanonicalConflict(i64),
    /// Another collection already reserved the intended playlist path.
    #[error("playlist output path is already reserved: {0}")]
    PlaylistPathReserved(PathBuf),
    /// Repeated materialization requested a different path for a collection.
    #[error("playlist output does not match collection {0}")]
    PlaylistOutputMismatch(i64),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn database_backup_is_consistent_and_never_clobbers() -> Result<(), Box<dyn std::error::Error>>
    {
        let root = tempfile::tempdir()?;
        let source = root.path().join("state.sqlite3");
        let backup = root.path().join("backup.sqlite3");
        let database = Database::open(&source)?;
        database.backup_to(&backup)?;
        assert_eq!(
            Database::inspect_read_only(&backup)?.version,
            CURRENT_SCHEMA_VERSION
        );
        assert!(matches!(
            database.backup_to(&backup),
            Err(DatabaseError::BackupAlreadyExists(path)) if path == backup
        ));
        assert!(matches!(
            database.backup_to(&root.path().join("missing/backup.sqlite3")),
            Err(DatabaseError::BackupParentMissing(_))
        ));
        Ok(())
    }

    #[test]
    fn service_runs_are_exclusive_auditable_and_explicitly_recoverable()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let path = root.path().join("state.sqlite3");
        let mut database = Database::open(&path)?;
        let first = database.start_service_run(ServiceRunTrigger::Timer, "0.1.0")?;
        database.start_service_phase(first, "sources", 0)?;
        assert!(
            matches!(database.start_service_run(ServiceRunTrigger::Manual,"0.1.0"),Err(DatabaseError::ServiceRunAlreadyRunning(id)) if id==first)
        );
        assert!(
            matches!(database.finish_service_run(first,ServiceRunTerminalStatus::Succeeded,&serde_json::json!({})),Err(DatabaseError::ServiceRunHasRunningPhases(id)) if id==first)
        );
        assert_eq!(database.recover_interrupted_service_run()?, Some(first));
        let second = database.start_service_run(ServiceRunTrigger::Manual, "0.1.0")?;
        database.start_service_phase(second, "sources", 0)?;
        database.finish_service_phase(
            second,
            "sources",
            ServicePhaseStatus::Succeeded,
            &serde_json::json!({"sources":2}),
            None,
        )?;
        database.finish_service_run(
            second,
            ServiceRunTerminalStatus::Succeeded,
            &serde_json::json!({"phases":1}),
        )?;
        drop(database);
        let history = Database::service_run_history_read_only(&path, 10, None)?;
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].status, "succeeded");
        assert_eq!(history[0].phase_count, 1);
        assert_eq!(history[1].status, "interrupted");
        assert_eq!(history[1].failed_phase_count, 1);
        Ok(())
    }

    #[test]
    fn discovery_lane_budgets_retain_small_nonzero_lanes() {
        assert_eq!(discovery_lane_budgets(15, 0.20, 0.05), (3, 1));
        assert_eq!(discovery_lane_budgets(3, 0.34, 0.33), (1, 1));
        assert_eq!(discovery_lane_budgets(1, 0.5, 0.5), (0, 1));
    }

    #[test]
    fn navidrome_seed_import_is_exact_and_replaces_active_set()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut database = Database::open_in_memory()?;
        let first = "11111111-1111-1111-1111-111111111111";
        let second = "22222222-2222-2222-2222-222222222222";
        database.connection.execute(
            "INSERT INTO recordings(musicbrainz_recording_id) VALUES (?1)",
            [first],
        )?;
        let report =
            database.replace_navidrome_seeds(&[first.into(), second.into(), first.into()])?;
        assert_eq!(report.matched, 1);
        assert_eq!(report.unmatched, 1);
        assert_eq!(database.table_count("discovery_seeds")?, 1);
        let empty = database.replace_navidrome_seeds(&[])?;
        assert_eq!(empty, DiscoverySeedReport::default());
        assert_eq!(
            database.connection.query_row(
                "SELECT active FROM discovery_seeds WHERE recording_id=1",
                [],
                |row| row.get::<_, u32>(0),
            )?,
            0
        );
        Ok(())
    }
    use crate::musicbrainz::{CanonicalArtistCredit, CanonicalRecording, CanonicalRelease};
    use crate::provider::ProviderItem;
    use serde_json::json;
    use std::os::unix::ffi::OsStringExt;

    fn snapshot(item_ids: &[&str]) -> SourceSnapshot {
        SourceSnapshot {
            provider: "youtube".into(),
            provider_collection_id: Some("fixture-playlist".into()),
            title: Some("Fixture playlist".into()),
            items: item_ids
                .iter()
                .map(|id| ProviderItem {
                    provider_item_id: (*id).into(),
                    url: format!("https://www.youtube.com/watch?v={id}"),
                    title: Some(format!("Track {id}")),
                    duration_ms: Some(120_000),
                    raw_metadata: json!({ "id": id }),
                })
                .collect(),
        }
    }

    #[test]
    fn migrates_an_empty_database_transactionally() -> Result<(), DatabaseError> {
        let database = Database::open_in_memory()?;
        assert_eq!(database.schema_version()?, CURRENT_SCHEMA_VERSION);
        Ok(())
    }

    #[test]
    fn reopening_is_idempotent() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("state.sqlite3");
        drop(Database::open(&path)?);
        let database = Database::open(&path)?;
        assert_eq!(database.schema_version()?, CURRENT_SCHEMA_VERSION);
        Ok(())
    }

    #[test]
    fn adopted_artifacts_are_inserted_atomically_and_idempotently()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut database = Database::open_in_memory()?;
        let paths = vec![
            PathBuf::from("/music/a.flac"),
            PathBuf::from("/music/b.m4a"),
        ];

        let first = database.register_adopted_artifacts(&paths)?;
        let second = database.register_adopted_artifacts(&paths)?;

        assert_eq!(first.artifacts_inserted, 2);
        assert_eq!(first.recordings_inserted, 2);
        assert_eq!(second.artifacts_inserted, 0);
        assert_eq!(second.artifacts_existing, 2);
        assert_eq!(database.table_count("recordings")?, 2);
        assert_eq!(database.table_count("artifacts")?, 2);
        Ok(())
    }

    #[test]
    fn invalid_path_rolls_back_the_entire_adoption_batch() -> Result<(), Box<dyn std::error::Error>>
    {
        let mut database = Database::open_in_memory()?;
        let invalid = PathBuf::from(std::ffi::OsString::from_vec(vec![b'/', 0xff]));
        let paths = vec![PathBuf::from("/music/valid.flac"), invalid];

        let result = database.register_adopted_artifacts(&paths);

        assert!(matches!(result, Err(DatabaseError::NonUnicodePath(_))));
        assert_eq!(database.table_count("recordings")?, 0);
        assert_eq!(database.table_count("artifacts")?, 0);
        Ok(())
    }

    #[test]
    fn source_add_deactivate_and_reactivate_are_idempotent()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut database = Database::open_in_memory()?;
        let url = "https://www.youtube.com/playlist?list=fixture";

        let first = database.add_source(url, Some("Fixture"))?;
        let repeated = database.add_source(url, None)?;
        assert!(first.inserted);
        assert_eq!(repeated.id, first.id);
        assert!(!repeated.inserted);
        assert!(!repeated.reactivated);
        assert!(database.deactivate_source(first.id)?);
        assert!(!database.deactivate_source(first.id)?);
        assert!(database.list_sources(false)?.is_empty());

        let reactivated = database.add_source(url, None)?;
        assert!(reactivated.reactivated);
        assert_eq!(database.list_sources(false)?.len(), 1);
        assert_eq!(
            database.list_sources(true)?[0].name.as_deref(),
            Some("Fixture")
        );
        Ok(())
    }

    #[test]
    fn source_snapshot_reconciliation_is_transactional_and_idempotent()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut database = Database::open_in_memory()?;
        let source = database.add_source(
            "https://www.youtube.com/playlist?list=fixture-playlist",
            Some("Fixture"),
        )?;

        let first = database.reconcile_source_snapshot(source.id, &snapshot(&["one", "two"]))?;
        let repeated = database.reconcile_source_snapshot(source.id, &snapshot(&["one", "two"]))?;

        assert_eq!(first.provider_items_inserted, 2);
        assert_eq!(first.memberships_activated, 2);
        assert_eq!(first.jobs_created, 2);
        assert_eq!(repeated.provider_items_inserted, 0);
        assert_eq!(repeated.provider_items_existing, 2);
        assert_eq!(repeated.memberships_unchanged, 2);
        assert_eq!(repeated.jobs_created, 0);
        assert_eq!(database.table_count("provider_items")?, 2);
        assert_eq!(database.table_count("collection_memberships")?, 2);
        assert_eq!(database.table_count("jobs")?, 2);
        assert_eq!(database.table_count("sync_runs")?, 2);
        Ok(())
    }

    #[test]
    fn missing_snapshot_items_only_deactivate_membership() -> Result<(), Box<dyn std::error::Error>>
    {
        let mut database = Database::open_in_memory()?;
        let source = database.add_source(
            "https://www.youtube.com/playlist?list=fixture-playlist",
            None,
        )?;
        database.reconcile_source_snapshot(source.id, &snapshot(&["one", "two"]))?;

        let removal = database.reconcile_source_snapshot(source.id, &snapshot(&["two"]))?;
        let active: u64 = database.connection.query_row(
            "SELECT COUNT(*) FROM collection_memberships WHERE active = 1",
            [],
            |row| row.get(0),
        )?;

        assert_eq!(removal.memberships_deactivated, 1);
        assert_eq!(removal.memberships_unchanged, 1);
        assert_eq!(removal.jobs_created, 0);
        assert_eq!(active, 1);
        assert_eq!(database.table_count("collection_memberships")?, 2);
        assert_eq!(database.table_count("provider_items")?, 2);
        assert_eq!(database.table_count("jobs")?, 2);
        Ok(())
    }

    #[test]
    fn invalid_snapshot_rolls_back_without_membership_changes()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut database = Database::open_in_memory()?;
        let source = database.add_source(
            "https://www.youtube.com/playlist?list=fixture-playlist",
            None,
        )?;
        let mut invalid = snapshot(&["one"]);
        invalid.provider = "unexpected".into();

        let result = database.reconcile_source_snapshot(source.id, &invalid);

        assert!(matches!(
            result,
            Err(DatabaseError::ProviderMismatch { .. })
        ));
        assert_eq!(database.table_count("provider_items")?, 0);
        assert_eq!(database.table_count("collection_memberships")?, 0);
        assert_eq!(database.table_count("jobs")?, 0);
        assert_eq!(database.table_count("sync_runs")?, 0);
        Ok(())
    }

    #[test]
    fn acquisition_claim_defer_and_recover_are_explicit() -> Result<(), Box<dyn std::error::Error>>
    {
        let mut database = Database::open_in_memory()?;
        let source = database.add_source(
            "https://www.youtube.com/playlist?list=fixture-playlist",
            None,
        )?;
        database.reconcile_source_snapshot(source.id, &snapshot(&["one", "two"]))?;

        let first = database
            .claim_next_acquisition()?
            .ok_or("missing first acquisition")?;
        assert_eq!(first.provider_item_id, "one");
        assert_eq!(first.attempt, 1);
        assert!(database.defer_acquisition(first.job_id, "temporary provider failure")?);
        assert!(!database.defer_acquisition(first.job_id, "already deferred")?);
        assert!(database.claim_acquisition(first.job_id)?.is_none());
        assert!(database.retry_deferred_acquisition(first.job_id)?);
        assert!(!database.retry_deferred_acquisition(first.job_id)?);

        let retry = database
            .claim_next_acquisition()?
            .ok_or("missing deferred acquisition")?;
        assert_eq!(retry.job_id, first.job_id);
        assert_eq!(retry.attempt, 2);
        assert_eq!(database.recover_interrupted_acquisitions()?, 1);
        assert!(database.claim_acquisition(first.job_id)?.is_none());
        assert!(database.retry_deferred_acquisition(first.job_id)?);

        let recovered = database
            .claim_next_acquisition()?
            .ok_or("missing recovered acquisition")?;
        assert_eq!(recovered.job_id, first.job_id);
        assert_eq!(recovered.attempt, 3);
        assert!(database.defer_acquisition(recovered.job_id, "awaiting artifact validation")?);
        assert_eq!(database.table_count("acquisition_jobs")?, 2);
        assert_eq!(database.table_count("events")?, 5);
        Ok(())
    }

    #[test]
    fn migration_four_backfills_existing_acquisition_jobs() -> Result<(), Box<dyn std::error::Error>>
    {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("v3.sqlite3");
        let connection = Connection::open(&path)?;
        for (version, sql) in &MIGRATIONS[..3] {
            connection.execute_batch(sql)?;
            connection.execute(
                "INSERT INTO schema_migrations(version) VALUES (?1)",
                [version],
            )?;
            connection.pragma_update(None, "user_version", version)?;
        }
        connection.execute(
            "INSERT INTO provider_items(
                 provider, provider_item_id, original_url, source_metadata_json)
             VALUES ('youtube', 'existing', 'https://youtu.be/existing', '{}')",
            [],
        )?;
        connection.execute("INSERT INTO sync_runs(status) VALUES ('succeeded')", [])?;
        connection.execute(
            "INSERT INTO jobs(run_id, kind, status, idempotency_key)
             VALUES (1, 'acquire', 'pending', 'acquire:youtube:existing')",
            [],
        )?;
        drop(connection);

        let database = Database::open(&path)?;

        assert_eq!(database.schema_version()?, CURRENT_SCHEMA_VERSION);
        assert_eq!(database.table_count("acquisition_jobs")?, 1);
        Ok(())
    }

    #[test]
    fn repair_eligibility_requires_every_conservative_prerequisite()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut database = Database::open_in_memory()?;
        database
            .connection
            .execute("INSERT INTO recordings DEFAULT VALUES", [])?;
        database.connection.execute(
            "INSERT INTO artifacts(recording_id, path, health)
             VALUES (1, '/music/missing.opus', 'missing')",
            [],
        )?;
        database.connection.execute(
            "UPDATE recordings SET preferred_artifact_id = 1 WHERE id = 1",
            [],
        )?;
        database.connection.execute(
            "INSERT INTO provider_items(provider, provider_item_id, original_url,
                 recording_id) VALUES ('youtube', 'original',
                 'https://youtu.be/original', 1)",
            [],
        )?;
        database.connection.execute(
            "INSERT INTO collections(provider, provider_collection_id, name)
             VALUES ('youtube', 'fixture', 'Fixture')",
            [],
        )?;
        database.connection.execute(
            "INSERT INTO collection_memberships(collection_id, provider_item_id, active)
             VALUES (1, 1, 1)",
            [],
        )?;
        database.record_artifact_fingerprint(&ArtifactFingerprintEvidence {
            artifact_id: 1,
            max_seconds: 120,
            duration_ms: 180_000,
            fingerprint_json: "[1,2,3]".into(),
            value_count: 3,
        })?;

        let candidates = database.repair_original_candidates(10)?;
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].availability, ProviderAvailability::Unknown);
        assert_eq!(database.reconcile_repair_eligibility(10)?.inserted, 0);
        assert!(database.record_provider_availability(
            1,
            ProviderAvailability::TransientFailure,
            "provider timeout"
        )?);
        assert_eq!(database.reconcile_repair_eligibility(10)?.inserted, 0);

        assert!(database.record_provider_availability(
            1,
            ProviderAvailability::PermanentlyUnavailable,
            "provider explicitly reports deleted"
        )?);
        assert_eq!(database.reconcile_repair_eligibility(10)?.inserted, 1);
        assert_eq!(database.reconcile_repair_eligibility(10)?.unchanged, 1);
        assert!(!database.record_provider_availability(
            1,
            ProviderAvailability::TransientFailure,
            "later provider timeout does not erase definitive evidence"
        )?);
        assert_eq!(
            database.repair_original_candidates(10)?[0].availability,
            ProviderAvailability::PermanentlyUnavailable
        );
        let eligible = database.eligible_repair_cases(10)?;
        assert_eq!(eligible.len(), 1);
        let candidate = ProviderItem {
            provider_item_id: "candidate".into(),
            url: "https://youtu.be/candidate".into(),
            title: Some("Text is generation only".into()),
            duration_ms: Some(180_000),
            raw_metadata: json!({"id": "candidate"}),
        };
        assert_eq!(
            database
                .record_repair_candidates(eligible[0].case_id, std::slice::from_ref(&candidate),)?,
            1
        );
        assert_eq!(
            database.record_repair_candidates(eligible[0].case_id, &[candidate])?,
            0
        );
        assert_eq!(database.table_count("repair_attempts")?, 1);
        let work = database
            .claim_next_repair_attempt()?
            .ok_or("generated repair attempt was not claimable")?;
        assert_eq!(work.attempt, 1);
        assert!(database.claim_next_repair_attempt()?.is_none());
        database.complete_repair_verification(&RepairVerificationEvidence {
            attempt_id: work.attempt_id,
            staged_path: PathBuf::from("/state/repair-staging/attempt-1/media.opus"),
            sha256: "00".repeat(32),
            bytes: 13,
            codec: "opus".into(),
            duration_ms: Some(180_000),
            sample_rate_hz: Some(48_000),
            channels: Some(2),
            musicbrainz_recording_id: None,
            isrc: None,
            fingerprint_max_seconds: 120,
            fingerprint_duration_ms: 180_000,
            fingerprint_json: "[1,2,3]".into(),
            decision: RepairVerificationDecision::Unresolved,
            reason: "canonical identity unavailable".into(),
        })?;
        assert_eq!(database.table_count("repair_attempt_evidence")?, 1);

        database
            .connection
            .execute("UPDATE artifacts SET health = 'healthy' WHERE id = 1", [])?;
        assert_eq!(database.reconcile_repair_eligibility(10)?.cancelled, 1);
        database
            .connection
            .execute("UPDATE artifacts SET health = 'missing' WHERE id = 1", [])?;
        assert_eq!(database.reconcile_repair_eligibility(10)?.reopened, 1);
        Ok(())
    }

    #[test]
    fn migrates_existing_v1_database_forward() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("v1.sqlite3");
        let connection = Connection::open(&path)?;
        connection.execute_batch(include_str!("../migrations/0001_initial.sql"))?;
        connection.execute("INSERT INTO schema_migrations(version) VALUES (1)", [])?;
        connection.pragma_update(None, "user_version", 1_u32)?;
        drop(connection);

        let database = Database::open(&path)?;

        assert_eq!(database.schema_version()?, CURRENT_SCHEMA_VERSION);
        assert!(database.list_sources(true)?.is_empty());
        Ok(())
    }

    #[test]
    fn migration_nine_preserves_existing_generated_repair_attempts()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("v8.sqlite3");
        let connection = Connection::open(&path)?;
        connection.execute_batch("PRAGMA foreign_keys = ON")?;
        for (version, sql) in &MIGRATIONS[..8] {
            connection.execute_batch(sql)?;
            connection.execute(
                "INSERT INTO schema_migrations(version) VALUES (?1)",
                [version],
            )?;
            connection.pragma_update(None, "user_version", version)?;
        }
        connection.execute("INSERT INTO recordings DEFAULT VALUES", [])?;
        connection.execute(
            "INSERT INTO artifacts(recording_id, path, health)
             VALUES (1, '/music/missing.opus', 'missing')",
            [],
        )?;
        connection.execute(
            "INSERT INTO provider_items(provider, provider_item_id, original_url,
                 availability, recording_id)
             VALUES ('youtube', 'original', 'https://youtu.be/original',
                     'permanently_unavailable', 1)",
            [],
        )?;
        connection.execute(
            "INSERT INTO repair_cases(recording_id, original_provider_item_id,
                 reference_artifact_id) VALUES (1, 1, 1)",
            [],
        )?;
        connection.execute(
            "INSERT INTO repair_attempts(repair_case_id, candidate_provider,
                 candidate_provider_item_id, candidate_url, state, reason)
             VALUES (1, 'youtube', 'candidate', 'https://youtu.be/candidate',
                     'generated', 'search generated only')",
            [],
        )?;
        drop(connection);

        let database = Database::open(&path)?;
        let state = database.connection.query_row(
            "SELECT state, attempt_count FROM repair_attempts WHERE id = 1",
            [],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, u32>(1)?)),
        )?;
        assert_eq!(state, ("generated".into(), 0));
        assert_eq!(database.schema_version()?, CURRENT_SCHEMA_VERSION);
        Ok(())
    }

    #[test]
    fn canonical_metadata_persists_field_provenance_and_selection_idempotently()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut database = Database::open_in_memory()?;
        let mbid = "f59c5520-5f46-4d2c-b2c4-822eabf53419";
        database.connection.execute(
            "INSERT INTO recordings(musicbrainz_recording_id) VALUES (?1)",
            [mbid],
        )?;
        database.connection.execute(
            "INSERT INTO artifacts(recording_id, path, health)
             VALUES (1, '/music/track.opus', 'healthy')",
            [],
        )?;
        database.connection.execute(
            "UPDATE recordings SET preferred_artifact_id = 1 WHERE id = 1",
            [],
        )?;
        let candidate = database.metadata_resolution_candidates(10)?.remove(0);
        let release = CanonicalRelease {
            id: "99999999-8888-7777-6666-555555555555".into(),
            title: "Fixture Album".into(),
            date: Some("2024-02-03".into()),
            country: Some("US".into()),
            status: Some("Official".into()),
            release_group_id: Some("12345678-1234-1234-1234-123456789abc".into()),
            primary_type: Some("Album".into()),
            secondary_types: Vec::new(),
        };
        let recording = CanonicalRecording {
            id: mbid.into(),
            title: "Fixture Track".into(),
            length_ms: Some(180_000),
            isrcs: vec!["USABC2412345".into()],
            artist_credit: vec![CanonicalArtistCredit {
                artist_id: "11111111-2222-3333-4444-555555555555".into(),
                artist_name: "Fixture Artist".into(),
                sort_name: Some("Artist, Fixture".into()),
                disambiguation: None,
                credited_name: "Fixture Artist".into(),
                join_phrase: String::new(),
            }],
            releases: vec![release.clone()],
            url_relations: Vec::new(),
        };

        let first = database.record_resolved_metadata(
            &candidate,
            MetadataResolutionMethod::RecordingMbid,
            &recording,
            Some(&release),
            "{\"fixture\":true}",
        )?;
        let repeat = database.record_resolved_metadata(
            &candidate,
            MetadataResolutionMethod::RecordingMbid,
            &recording,
            Some(&release),
            "{\"fixture\":true}",
        )?;

        assert_eq!(first.observations_selected, 4);
        assert_eq!(repeat.observations_selected, 0);
        assert_eq!(database.table_count("artists")?, 1);
        assert_eq!(database.table_count("releases")?, 1);
        assert_eq!(database.table_count("recording_artist_credits")?, 1);
        assert_eq!(database.table_count("metadata_observations")?, 4);
        assert_eq!(database.table_count("metadata_selections")?, 4);
        assert!(database.metadata_resolution_candidates(10)?.is_empty());
        assert!(database.lyrics_work_candidates(10)?.is_empty());

        let artwork_candidate = database.artwork_resolution_candidates(10)?.remove(0);
        let artwork = ResolvedArtwork {
            source_image_id: "42".into(),
            source_url: "https://archive.org/fixture.jpg".into(),
            role: "front".into(),
            approved: true,
        };
        let blob = ArtworkBlob {
            sha256: "ab".repeat(32),
            relative_path: format!("artwork-cache/ab/{}.jpg", "ab".repeat(32)),
            mime_type: "image/jpeg".into(),
            byte_count: 123,
        };
        assert!(database.record_resolved_artwork(
            &artwork_candidate,
            &artwork,
            &blob,
            "{\"images\":[]}",
        )?);
        assert!(!database.record_resolved_artwork(
            &artwork_candidate,
            &artwork,
            &blob,
            "{\"images\":[]}",
        )?);
        assert!(database.artwork_resolution_candidates(10)?.is_empty());
        assert_eq!(database.table_count("artwork_blobs")?, 1);
        assert_eq!(database.table_count("release_artwork")?, 1);
        Ok(())
    }
}
