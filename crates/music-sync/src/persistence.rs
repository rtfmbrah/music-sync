//! SQLite persistence and schema migration foundation.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags, OptionalExtension, Transaction, TransactionBehavior};
use thiserror::Error;

use crate::acquisition::ValidatedStagedMedia;
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
];

/// Current durable schema version.
pub const CURRENT_SCHEMA_VERSION: u32 = 8;

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
            playlist_outputs: count(
                "SELECT COUNT(*) FROM playlist_outputs WHERE sha256 IS NOT NULL",
            )?,
            recent_events: events,
        })
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
        let mut inserted = 0_u64;
        for candidate in candidates {
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
        let inserted = transaction
            .execute(
                "INSERT OR IGNORE INTO acquisition_commits(
                     job_id, staged_path, final_path, sha256, bytes, codec,
                     duration_ms, sample_rate_hz, channels, status)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 'prepared')",
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
                ],
            )
            .map_err(DatabaseError::Sqlite)?
            == 1;
        let stored = transaction
            .query_row(
                "SELECT staged_path, final_path, sha256, bytes, codec, duration_ms,
                        sample_rate_hz, channels
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
                        row.get::<_, i64>(7)?,
                        row.get::<_, String>(8)?,
                    ))
                },
            )
            .optional()
            .map_err(DatabaseError::Sqlite)?
            .ok_or(DatabaseError::AcquisitionCommitNotPrepared(job_id))?;
        if commit.0 == "committed" && commit.8 == "succeeded" {
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
        if commit.0 != "prepared" || commit.8 != "running" {
            return Err(DatabaseError::AcquisitionCommitNotPrepared(job_id));
        }
        let existing_recording_id = transaction
            .query_row(
                "SELECT recording_id FROM provider_items WHERE id = ?1",
                [commit.7],
                |row| row.get::<_, Option<i64>>(0),
            )
            .map_err(DatabaseError::Sqlite)?;
        let recording_id = if let Some(recording_id) = existing_recording_id {
            recording_id
        } else {
            transaction
                .execute("INSERT INTO recordings DEFAULT VALUES", [])
                .map_err(DatabaseError::Sqlite)?;
            transaction.last_insert_rowid()
        };
        transaction
            .execute(
                "UPDATE provider_items SET recording_id = ?2
                 WHERE id = ?1 AND recording_id IS NULL",
                rusqlite::params![commit.7, recording_id],
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

/// Persistence initialization or migration failure.
#[derive(Debug, Error)]
pub enum DatabaseError {
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
}
