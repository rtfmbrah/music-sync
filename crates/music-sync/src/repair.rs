//! Conservative assessment and durable eligibility for lost-media repair.

use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;
use thiserror::Error;

use crate::acquisition::{
    ArtifactCommitEffect, ArtifactCommitError, ValidatedStagedMedia, artifact_destination,
    commit_staged_file, validate_staged_media,
};
use crate::content_hash::ContentHasher;
use crate::fingerprint::{Fingerprinter, RawFingerprint};
use crate::identity::{
    CanonicalIdentityComparison, MetadataCompatibility, ReplacementDecision, ReplacementEvidence,
    compare_raw_fingerprints, verify_replacement,
};
use crate::media_probe::MediaProbe;
use crate::persistence::{
    Database, DatabaseError, ProviderAvailability, RepairCommitPersistence,
    RepairEligibilitySummary, RepairVerificationDecision, RepairVerificationEvidence,
};
use crate::provider::ProviderFailureKind;
use crate::yt_dlp::YtDlp;

/// Revalidates and atomically commits one independently verified replacement.
pub fn commit_verified_repair(
    database: &mut Database,
    attempt_id: i64,
    library_directory: &Path,
    hasher: &dyn ContentHasher,
) -> Result<RepairCommitReport, RepairCommitError> {
    let work = database.verified_repair_attempt(attempt_id)?;
    let current = hasher.hash(&work.validated.path)?;
    if current.bytes_hashed != work.validated.bytes
        || hex_sha256(current.sha256) != work.validated.sha256
    {
        return Err(RepairCommitError::StagedEvidenceChanged(attempt_id));
    }
    let destination_id = format!(
        "recording-{}-attempt-{}",
        work.recording_id, work.attempt_id
    );
    let final_path = artifact_destination(
        library_directory,
        "repair",
        &destination_id,
        &work.validated.path,
    )?;
    let newly_prepared = database.prepare_repair_commit(attempt_id, &final_path)?;
    let filesystem = commit_staged_file(&work.validated, &final_path, hasher, !newly_prepared)?;
    let persistence = database.finalize_repair_commit(attempt_id)?;
    Ok(RepairCommitReport {
        attempt_id,
        final_path,
        filesystem,
        persistence,
    })
}

fn hex_sha256(bytes: [u8; 32]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(64);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

/// Claims and independently verifies at most one generated repair candidate.
pub fn run_one_repair_verification(
    database: &mut Database,
    state_directory: &Path,
    downloader: &YtDlp,
    probe: &dyn MediaProbe,
    hasher: &dyn ContentHasher,
    fingerprinter: &dyn Fingerprinter,
) -> Result<RepairRunOutcome, RepairRunError> {
    let Some(work) = database.claim_next_repair_attempt()? else {
        return Ok(RepairRunOutcome::Idle);
    };
    let result: Result<RepairRunOutcome, RepairRunError> = (|| {
        let staging = prepare_repair_staging(state_directory, work.attempt_id)?;
        let staged = match existing_staged_candidate(&staging)? {
            Some(path) => path,
            None => downloader.download(&work.candidate_url, &staging)?.path,
        };
        let validated = validate_staged_media(&staged, probe, hasher)?;
        let candidate_fingerprint = fingerprinter.fingerprint(&validated.path)?;
        let reference_values: Vec<u32> = serde_json::from_str(&work.reference_fingerprint_json)
            .map_err(RepairRunError::ReferenceFingerprint)?;
        let reference = RawFingerprint {
            duration_ms: u64::try_from(work.reference_fingerprint_duration_ms)
                .map_err(|_| RepairRunError::InvalidReferenceEvidence)?,
            values: reference_values,
        };
        let canonical_identity = compare_canonical_identity(
            work.reference_musicbrainz_recording_id.as_deref(),
            work.reference_isrc.as_deref(),
            validated.musicbrainz_recording_id.as_deref(),
            validated.isrc.as_deref(),
        );
        let metadata = compare_duration(
            work.reference_duration_ms,
            validated
                .duration_ms
                .and_then(|value| i64::try_from(value).ok()),
        );
        let fingerprint = compare_raw_fingerprints(&reference, &candidate_fingerprint);
        let decision = verify_replacement(ReplacementEvidence {
            canonical_identity,
            metadata,
            fingerprint,
        });
        let (durable_decision, reason) = match decision {
            ReplacementDecision::Accept => (
                RepairVerificationDecision::Verified,
                "canonical identity, duration, and perceptual fingerprint match",
            ),
            ReplacementDecision::Reject => (
                RepairVerificationDecision::Rejected,
                "candidate contradicts canonical, duration, or perceptual evidence",
            ),
            ReplacementDecision::Unresolved => (
                RepairVerificationDecision::Unresolved,
                "candidate lacks sufficient independent identity evidence",
            ),
        };
        let evidence = verification_evidence(
            work.attempt_id,
            &validated,
            work.reference_fingerprint_max_seconds,
            &candidate_fingerprint,
            durable_decision,
            reason,
        )?;
        database.complete_repair_verification(&evidence)?;
        Ok(RepairRunOutcome::Completed {
            attempt_id: work.attempt_id,
            decision: durable_decision,
            reason: reason.into(),
        })
    })();
    match result {
        Ok(outcome) => Ok(outcome),
        Err(error) => {
            database.defer_repair_attempt(work.attempt_id, &bounded_message(&error.to_string()))?;
            Ok(RepairRunOutcome::Deferred {
                attempt_id: work.attempt_id,
                message: error.to_string(),
            })
        }
    }
}

fn prepare_repair_staging(
    state_directory: &Path,
    attempt_id: i64,
) -> Result<PathBuf, RepairRunError> {
    if attempt_id <= 0 {
        return Err(RepairRunError::InvalidAttemptId(attempt_id));
    }
    let root = state_directory.join("repair-staging");
    fs::create_dir_all(&root).map_err(|source| RepairRunError::Staging {
        path: root.clone(),
        source,
    })?;
    let directory = root.join(format!("attempt-{attempt_id}"));
    fs::create_dir_all(&directory).map_err(|source| RepairRunError::Staging {
        path: directory.clone(),
        source,
    })?;
    directory
        .canonicalize()
        .map_err(|source| RepairRunError::Staging {
            path: directory,
            source,
        })
}

fn existing_staged_candidate(directory: &Path) -> Result<Option<PathBuf>, RepairRunError> {
    let mut candidates = Vec::new();
    for entry in fs::read_dir(directory).map_err(|source| RepairRunError::Staging {
        path: directory.to_path_buf(),
        source,
    })? {
        let entry = entry.map_err(|source| RepairRunError::Staging {
            path: directory.to_path_buf(),
            source,
        })?;
        let metadata =
            fs::symlink_metadata(entry.path()).map_err(|source| RepairRunError::Staging {
                path: entry.path(),
                source,
            })?;
        if metadata.is_file() && !metadata.file_type().is_symlink() {
            candidates.push(entry.path());
        }
    }
    match candidates.len() {
        0 => Ok(None),
        1 => Ok(candidates.pop()),
        count => Err(RepairRunError::AmbiguousStaging(count)),
    }
}

fn compare_canonical_identity(
    reference_mbid: Option<&str>,
    reference_isrc: Option<&str>,
    candidate_mbid: Option<&str>,
    candidate_isrc: Option<&str>,
) -> CanonicalIdentityComparison {
    let comparisons = [
        reference_mbid
            .zip(candidate_mbid)
            .map(|(left, right)| left == right),
        reference_isrc
            .zip(candidate_isrc)
            .map(|(left, right)| left == right),
    ];
    if comparisons.contains(&Some(false)) {
        CanonicalIdentityComparison::Mismatch
    } else if comparisons.contains(&Some(true)) {
        CanonicalIdentityComparison::Match
    } else {
        CanonicalIdentityComparison::Unavailable
    }
}

fn compare_duration(reference: Option<i64>, candidate: Option<i64>) -> MetadataCompatibility {
    match (reference, candidate) {
        (Some(reference), Some(candidate)) if reference >= 0 && candidate >= 0 => {
            if reference.abs_diff(candidate) <= 3_000 {
                MetadataCompatibility::Match
            } else {
                MetadataCompatibility::Mismatch
            }
        }
        _ => MetadataCompatibility::Unavailable,
    }
}

fn verification_evidence(
    attempt_id: i64,
    validated: &ValidatedStagedMedia,
    max_seconds: i64,
    fingerprint: &RawFingerprint,
    decision: RepairVerificationDecision,
    reason: &str,
) -> Result<RepairVerificationEvidence, RepairRunError> {
    Ok(RepairVerificationEvidence {
        attempt_id,
        staged_path: validated.path.clone(),
        sha256: validated.sha256.clone(),
        bytes: validated.bytes,
        codec: validated.codec.clone(),
        duration_ms: validated
            .duration_ms
            .map(i64::try_from)
            .transpose()
            .map_err(|_| RepairRunError::InvalidCandidateEvidence)?,
        sample_rate_hz: validated.sample_rate_hz.map(i64::from),
        channels: validated.channels.map(i64::from),
        musicbrainz_recording_id: validated.musicbrainz_recording_id.clone(),
        isrc: validated.isrc.clone(),
        fingerprint_max_seconds: max_seconds,
        fingerprint_duration_ms: i64::try_from(fingerprint.duration_ms)
            .map_err(|_| RepairRunError::InvalidCandidateEvidence)?,
        fingerprint_json: serde_json::to_string(&fingerprint.values)
            .map_err(RepairRunError::CandidateFingerprint)?,
        decision,
        reason: reason.into(),
    })
}

fn bounded_message(message: &str) -> String {
    message.chars().take(4_096).collect()
}

/// Generates bounded provider candidates without assigning any identity trust.
pub fn generate_repair_candidates(
    database: &mut Database,
    adapter: &YtDlp,
    maximum_cases: usize,
    maximum_candidates_per_case: usize,
) -> Result<RepairCandidateReport, RepairAssessmentError> {
    if maximum_cases == 0 || maximum_candidates_per_case == 0 {
        return Err(RepairAssessmentError::InvalidLimit);
    }
    let cases = database.eligible_repair_cases(maximum_cases)?;
    let mut report = RepairCandidateReport {
        selected_cases: cases.len() as u64,
        ..RepairCandidateReport::default()
    };
    for repair_case in cases {
        let Some(title) = repair_case
            .source_title
            .as_deref()
            .map(str::trim)
            .filter(|title| !title.is_empty())
        else {
            database.record_repair_candidates(repair_case.case_id, &[])?;
            report.failures.push(RepairAssessmentFailure {
                provider_item_id: repair_case.original_provider_item_id,
                message: "original item has no title for candidate generation".into(),
            });
            continue;
        };
        let query = format!("ytsearch{maximum_candidates_per_case}:{title}");
        match adapter.enumerate(&query) {
            Ok(snapshot) => {
                let candidates = snapshot
                    .items
                    .into_iter()
                    .take(maximum_candidates_per_case)
                    .collect::<Vec<_>>();
                report.generated +=
                    database.record_repair_candidates(repair_case.case_id, &candidates)?;
            }
            Err(error) => report.failures.push(RepairAssessmentFailure {
                provider_item_id: repair_case.original_provider_item_id,
                message: error.to_string(),
            }),
        }
    }
    Ok(report)
}

/// Checks a bounded stable set of unhealthy originals and reconciles repair cases.
pub fn assess_repair_eligibility(
    database: &mut Database,
    adapter: &YtDlp,
    maximum_items: usize,
) -> Result<RepairAssessmentReport, RepairAssessmentError> {
    if maximum_items == 0 {
        return Err(RepairAssessmentError::InvalidLimit);
    }
    let candidates = database.repair_original_candidates(maximum_items)?;
    let mut report = RepairAssessmentReport {
        selected: candidates.len() as u64,
        ..RepairAssessmentReport::default()
    };
    for candidate in candidates {
        if candidate.provider != "youtube" {
            report.failures.push(RepairAssessmentFailure {
                provider_item_id: candidate.provider_item_id,
                message: format!("unsupported provider adapter: {}", candidate.provider),
            });
            continue;
        }
        match adapter.enumerate(&candidate.original_url) {
            Ok(snapshot)
                if snapshot
                    .items
                    .iter()
                    .any(|item| item.provider_item_id == candidate.provider_owned_id) =>
            {
                if database.record_provider_availability(
                    candidate.provider_item_id,
                    ProviderAvailability::Available,
                    "original provider item is available",
                )? {
                    report.availability_changed += 1;
                }
                report.available += 1;
            }
            Ok(_) => {
                if database.record_provider_availability(
                    candidate.provider_item_id,
                    ProviderAvailability::TransientFailure,
                    "provider response did not contain the expected original identity",
                )? {
                    report.availability_changed += 1;
                }
                report.failures.push(RepairAssessmentFailure {
                    provider_item_id: candidate.provider_item_id,
                    message: "provider response did not contain the expected original identity"
                        .into(),
                });
            }
            Err(error) if error.kind() == ProviderFailureKind::PermanentlyUnavailable => {
                if database.record_provider_availability(
                    candidate.provider_item_id,
                    ProviderAvailability::PermanentlyUnavailable,
                    &error.to_string(),
                )? {
                    report.availability_changed += 1;
                }
                report.permanently_unavailable += 1;
            }
            Err(error) => {
                if database.record_provider_availability(
                    candidate.provider_item_id,
                    ProviderAvailability::TransientFailure,
                    &error.to_string(),
                )? {
                    report.availability_changed += 1;
                }
                report.failures.push(RepairAssessmentFailure {
                    provider_item_id: candidate.provider_item_id,
                    message: error.to_string(),
                });
            }
        }
    }
    report.eligibility = database.reconcile_repair_eligibility(maximum_items)?;
    Ok(report)
}

/// Bounded repair assessment effects.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct RepairAssessmentReport {
    /// Unhealthy active originals selected in stable provider-item order.
    pub selected: u64,
    /// Originals observed available and therefore ineligible for replacement.
    pub available: u64,
    /// Originals definitively reported permanently unavailable.
    pub permanently_unavailable: u64,
    /// Provider availability rows whose constrained state changed.
    pub availability_changed: u64,
    /// Durable eligibility reconciliation effects.
    pub eligibility: RepairEligibilitySummary,
    /// Isolated transient, infrastructure, identity, or unsupported-provider failures.
    pub failures: Vec<RepairAssessmentFailure>,
}

/// One assessment that produced no permanent-loss evidence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RepairAssessmentFailure {
    /// Durable original provider-item row ID.
    pub provider_item_id: i64,
    /// Conservative bounded diagnostic.
    pub message: String,
}

/// Search-only candidate-generation effects.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct RepairCandidateReport {
    /// Eligible cases selected in stable case order.
    pub selected_cases: u64,
    /// Newly persisted provider candidates.
    pub generated: u64,
    /// Isolated search failures or cases lacking even a generation query.
    pub failures: Vec<RepairAssessmentFailure>,
}

/// Observable outcome from processing at most one generated repair candidate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum RepairRunOutcome {
    /// No generated candidate was safely runnable.
    Idle,
    /// Candidate verification reached a durable terminal identity decision.
    Completed {
        /// Durable attempt ID.
        attempt_id: i64,
        /// Rejected, unresolved, or verified decision.
        #[serde(serialize_with = "serialize_repair_decision")]
        decision: RepairVerificationDecision,
        /// Auditable decision rationale.
        reason: String,
    },
    /// Infrastructure/provider/processing failure retained staging and deferred work.
    Deferred {
        /// Durable attempt ID.
        attempt_id: i64,
        /// Bounded failure diagnostic.
        message: String,
    },
}

/// Completed verified-repair filesystem and database effects.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RepairCommitReport {
    /// Durable verified attempt.
    pub attempt_id: i64,
    /// New managed repair path; the old artifact path remains untouched.
    pub final_path: PathBuf,
    /// Whether the no-clobber link was created or recovered from prepared intent.
    pub filesystem: ArtifactCommitEffect,
    /// Transactional replacement artifact persistence effect.
    pub persistence: RepairCommitPersistence,
}

/// Failure before or during a verified repair commit.
#[derive(Debug, Error)]
pub enum RepairCommitError {
    /// Durable verified evidence or commit state could not be accessed.
    #[error(transparent)]
    Database(#[from] DatabaseError),
    /// Rehashing staged media failed.
    #[error(transparent)]
    Hash(#[from] crate::content_hash::ContentHashError),
    /// Staged bytes changed since independent verification.
    #[error("verified repair attempt {0} staged bytes changed before commit")]
    StagedEvidenceChanged(i64),
    /// Safe destination derivation or no-clobber filesystem commit failed.
    #[error(transparent)]
    Artifact(#[from] ArtifactCommitError),
}

fn serialize_repair_decision<S>(
    value: &RepairVerificationDecision,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    serializer.serialize_str(match value {
        RepairVerificationDecision::Rejected => "rejected",
        RepairVerificationDecision::Unresolved => "unresolved",
        RepairVerificationDecision::Verified => "verified",
    })
}

/// Failure while staging or independently verifying a repair candidate.
#[derive(Debug, Error)]
pub enum RepairRunError {
    /// Durable state access failed.
    #[error(transparent)]
    Database(#[from] DatabaseError),
    /// Repair staging could not be prepared or inspected.
    #[error("failed to access repair staging {path}: {source}")]
    Staging {
        /// Staging path.
        path: PathBuf,
        /// Underlying filesystem failure.
        source: std::io::Error,
    },
    /// A durable attempt ID cannot form a safe staging namespace.
    #[error("repair attempt ID must be positive: {0}")]
    InvalidAttemptId(i64),
    /// Existing staging contained more than one possible media artifact.
    #[error("repair staging contains {0} candidate files; expected at most one")]
    AmbiguousStaging(usize),
    /// Candidate provider download failed.
    #[error(transparent)]
    Download(#[from] crate::yt_dlp::YtDlpError),
    /// Candidate structure or exact bytes could not be validated.
    #[error(transparent)]
    Validation(#[from] crate::acquisition::StagedMediaValidationError),
    /// Candidate perceptual evidence could not be derived.
    #[error(transparent)]
    Fingerprint(#[from] crate::fingerprint::FingerprintError),
    /// Retained reference fingerprint JSON violated durable invariants.
    #[error("retained reference fingerprint is invalid: {0}")]
    ReferenceFingerprint(serde_json::Error),
    /// Retained reference fingerprint numeric evidence is invalid.
    #[error("retained reference fingerprint evidence is invalid")]
    InvalidReferenceEvidence,
    /// Candidate numeric evidence exceeded durable representation.
    #[error("candidate fingerprint or media evidence exceeds durable representation")]
    InvalidCandidateEvidence,
    /// Candidate raw fingerprint could not be serialized.
    #[error("failed to serialize candidate fingerprint: {0}")]
    CandidateFingerprint(serde_json::Error),
}

/// Fatal repair assessment failure.
#[derive(Debug, Error)]
pub enum RepairAssessmentError {
    /// Work selection must be explicitly bounded and non-zero.
    #[error("maximum repair assessment items must be greater than zero")]
    InvalidLimit,
    /// Durable state access failed.
    #[error(transparent)]
    Database(#[from] DatabaseError),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_comparison_requires_shared_noncontradictory_evidence() {
        assert_eq!(
            compare_canonical_identity(Some("mbid"), None, Some("mbid"), None),
            CanonicalIdentityComparison::Match
        );
        assert_eq!(
            compare_canonical_identity(Some("mbid"), Some("isrc-a"), Some("mbid"), Some("isrc-b")),
            CanonicalIdentityComparison::Mismatch
        );
        assert_eq!(
            compare_canonical_identity(Some("mbid"), None, None, Some("isrc")),
            CanonicalIdentityComparison::Unavailable
        );
    }

    #[test]
    fn duration_comparison_distinguishes_absent_from_mismatch() {
        assert_eq!(
            compare_duration(Some(180_000), Some(182_999)),
            MetadataCompatibility::Match
        );
        assert_eq!(
            compare_duration(Some(180_000), Some(183_001)),
            MetadataCompatibility::Mismatch
        );
        assert_eq!(
            compare_duration(Some(180_000), None),
            MetadataCompatibility::Unavailable
        );
    }
}
