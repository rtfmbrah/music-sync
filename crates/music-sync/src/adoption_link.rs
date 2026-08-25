//! Explicit audio-verified bridge from adopted artifacts to provider objects.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;
use thiserror::Error;

use crate::acquisition::{ValidatedStagedMedia, validate_staged_media};
use crate::content_hash::ContentHasher;
use crate::fingerprint::{Fingerprinter, RawFingerprint};
use crate::identity::{FingerprintComparison, compare_raw_fingerprints};
use crate::media_probe::MediaProbe;
use crate::persistence::{
    AdoptionProviderCandidate, AdoptionProviderQuarantineCandidate,
    AdoptionProviderVerificationState, AdoptionProviderVerificationStatus, Database, DatabaseError,
};
use crate::yt_dlp::YtDlp;

/// Verifies a bounded set of unique filename-generated candidates without modifying audio.
pub fn verify_adopted_provider_links(
    database: &mut Database,
    state_directory: &Path,
    downloader: &YtDlp,
    probe: &dyn MediaProbe,
    hasher: &dyn ContentHasher,
    fingerprinter: &dyn Fingerprinter,
    maximum_items: usize,
) -> Result<AdoptionLinkReport, AdoptionLinkError> {
    if maximum_items == 0 {
        return Err(AdoptionLinkError::InvalidLimit);
    }
    let candidates = database.adoption_provider_candidates(maximum_items.saturating_mul(16))?;
    let mut grouped: BTreeMap<i64, Vec<AdoptionProviderCandidate>> = BTreeMap::new();
    for candidate in candidates {
        if filename_contains_exact_id(&candidate.artifact_path, &candidate.provider_item_id) {
            grouped
                .entry(candidate.provider_item_database_id)
                .or_default()
                .push(candidate);
        }
    }
    let mut report = AdoptionLinkReport::default();
    for candidates in grouped.into_values().take(maximum_items) {
        report.selected += 1;
        if candidates.len() != 1 {
            report.ambiguous += 1;
            continue;
        }
        let candidate = &candidates[0];
        database.start_adoption_provider_verification(candidate)?;
        match verify_one(
            candidate,
            state_directory,
            downloader,
            probe,
            hasher,
            fingerprinter,
        ) {
            Ok(evidence) => {
                let duration_matches = candidate
                    .artifact_duration_ms
                    .zip(
                        evidence
                            .validated
                            .duration_ms
                            .and_then(|value| i64::try_from(value).ok()),
                    )
                    .is_some_and(|(left, right)| left.abs_diff(right) <= 3_000);
                let comparison = compare_raw_fingerprints(&evidence.reference, &evidence.candidate);
                let (status, decision, message) = match (duration_matches, comparison) {
                    (true, FingerprintComparison::Match) => (
                        AdoptionProviderVerificationStatus::Verified,
                        "match",
                        "duration and perceptual fingerprint match",
                    ),
                    (_, FingerprintComparison::Mismatch) | (false, _) => (
                        AdoptionProviderVerificationStatus::Rejected,
                        "mismatch",
                        "duration or perceptual fingerprint contradicts the candidate",
                    ),
                    _ => (
                        AdoptionProviderVerificationStatus::Deferred,
                        "unavailable",
                        "perceptual evidence is insufficient",
                    ),
                };
                let fingerprint_json = serde_json::to_string(&evidence.candidate.values)?;
                database.finish_adoption_provider_verification(
                    candidate,
                    status,
                    Some(&evidence.validated),
                    Some(&fingerprint_json),
                    Some(decision),
                    Some(message),
                )?;
                match status {
                    AdoptionProviderVerificationStatus::Verified => report.verified += 1,
                    AdoptionProviderVerificationStatus::Rejected => report.rejected += 1,
                    AdoptionProviderVerificationStatus::Deferred => report.deferred += 1,
                }
            }
            Err(error) => {
                let message = error.to_string();
                database.finish_adoption_provider_verification(
                    candidate,
                    AdoptionProviderVerificationStatus::Deferred,
                    None,
                    None,
                    None,
                    Some(&message),
                )?;
                report.deferred += 1;
                report.failures.push(AdoptionLinkFailure {
                    provider_item_id: candidate.provider_item_id.clone(),
                    message,
                });
            }
        }
    }
    Ok(report)
}

/// Defers downloads that could duplicate an unresolved adopted filename candidate.
pub fn quarantine_unverified_adopted_provider_links(
    database: &mut Database,
    maximum_items: usize,
) -> Result<AdoptionLinkQuarantineReport, AdoptionLinkError> {
    if maximum_items == 0 {
        return Err(AdoptionLinkError::InvalidLimit);
    }
    let candidates =
        database.adoption_provider_quarantine_candidates(maximum_items.saturating_mul(16))?;
    let mut grouped: BTreeMap<i64, Vec<AdoptionProviderQuarantineCandidate>> = BTreeMap::new();
    for candidate in candidates {
        if filename_contains_exact_id(&candidate.artifact_path, &candidate.provider_item_id) {
            grouped
                .entry(candidate.provider_item_database_id)
                .or_default()
                .push(candidate);
        }
    }

    let mut report = AdoptionLinkQuarantineReport::default();
    for candidates in grouped.into_values().take(maximum_items) {
        report.selected += 1;
        let candidate = &candidates[0];
        match candidate.verification_status {
            Some(AdoptionProviderVerificationState::Running) => report.running += 1,
            Some(AdoptionProviderVerificationState::Rejected) => report.rejected += 1,
            Some(AdoptionProviderVerificationState::Verified) => report.verified += 1,
            None | Some(AdoptionProviderVerificationState::Deferred) => {
                if candidate.acquisition_deferred {
                    report.already_quarantined += 1;
                } else if database.quarantine_unverified_adoption_acquisition(
                    candidate.provider_item_database_id,
                    candidate.acquisition_job_id,
                )? {
                    report.quarantined += 1;
                } else {
                    report.changed_during_run += 1;
                }
            }
        }
    }
    Ok(report)
}

fn verify_one(
    candidate: &AdoptionProviderCandidate,
    state_directory: &Path,
    downloader: &YtDlp,
    probe: &dyn MediaProbe,
    hasher: &dyn ContentHasher,
    fingerprinter: &dyn Fingerprinter,
) -> Result<VerificationEvidence, AdoptionLinkBoundaryError> {
    let staging = state_directory
        .join("adoption-provider-staging")
        .join(format!(
            "provider-item-{}",
            candidate.provider_item_database_id
        ));
    fs::create_dir_all(&staging)?;
    let staged = existing_staged_file(&staging)?.map_or_else(
        || {
            downloader
                .download(&candidate.original_url, &staging)
                .map(|value| value.path)
        },
        Ok,
    )?;
    let validated = validate_staged_media(&staged, probe, hasher)?;
    let candidate_fingerprint = fingerprinter.fingerprint(&validated.path)?;
    let reference_values = serde_json::from_str(&candidate.fingerprint_json)?;
    let reference = RawFingerprint {
        duration_ms: u64::try_from(candidate.fingerprint_duration_ms)
            .map_err(|_| AdoptionLinkBoundaryError::InvalidReference)?,
        values: reference_values,
    };
    Ok(VerificationEvidence {
        validated,
        reference,
        candidate: candidate_fingerprint,
    })
}

fn existing_staged_file(directory: &Path) -> Result<Option<PathBuf>, std::io::Error> {
    let mut files = fs::read_dir(directory)?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_file())
        .collect::<Vec<_>>();
    files.sort();
    Ok((files.len() == 1).then(|| files.remove(0)))
}

fn filename_contains_exact_id(path: &Path, provider_id: &str) -> bool {
    let Some(stem) = path.file_stem().and_then(|value| value.to_str()) else {
        return false;
    };
    token_occurs_with_boundaries(stem, provider_id)
        || token_occurs_with_boundaries(stem, &format!("youtube-{provider_id}"))
}

fn token_occurs_with_boundaries(stem: &str, token: &str) -> bool {
    stem.match_indices(token).any(|(start, value)| {
        let end = start + value.len();
        let before = stem[..start].chars().next_back();
        let after = stem[end..].chars().next();
        before.is_none_or(|value| !is_id_character(value))
            && after.is_none_or(|value| !is_id_character(value))
    })
}

fn is_id_character(value: char) -> bool {
    value.is_ascii_alphanumeric() || matches!(value, '-' | '_')
}

struct VerificationEvidence {
    validated: ValidatedStagedMedia,
    reference: RawFingerprint,
    candidate: RawFingerprint,
}

/// Aggregate verification decisions.
#[derive(Debug, Default, Serialize)]
pub struct AdoptionLinkReport {
    /// Unique provider objects selected for verification.
    pub selected: u64,
    /// Provider objects safely associated with adopted artifacts.
    pub verified: u64,
    /// Filename candidates contradicted by audio evidence.
    pub rejected: u64,
    /// Candidates retaining insufficient or transient evidence.
    pub deferred: u64,
    /// Provider objects with more than one filename candidate.
    pub ambiguous: u64,
    /// Isolated boundary failures.
    pub failures: Vec<AdoptionLinkFailure>,
}

/// Aggregate duplicate-prevention decisions for unresolved adopted candidates.
#[derive(Debug, Default, Serialize)]
pub struct AdoptionLinkQuarantineReport {
    /// Unique unresolved provider objects with exact filename candidates.
    pub selected: u64,
    /// Pending acquisition jobs newly deferred for explicit operator review.
    pub quarantined: u64,
    /// Jobs already deferred by an earlier identical pass.
    pub already_quarantined: u64,
    /// Active verifications left untouched.
    pub running: u64,
    /// Fingerprint contradictions intentionally left eligible for acquisition.
    pub rejected: u64,
    /// Verified rows observed during a concurrent state transition.
    pub verified: u64,
    /// Rows whose guarded state changed before the transaction committed.
    pub changed_during_run: u64,
}

/// One isolated provider-boundary failure.
#[derive(Debug, Serialize)]
pub struct AdoptionLinkFailure {
    /// Provider-owned object identity.
    pub provider_item_id: String,
    /// Bounded failure diagnostic.
    pub message: String,
}

/// Fatal workflow error.
#[derive(Debug, Error)]
pub enum AdoptionLinkError {
    /// Selection must be explicitly bounded above zero.
    #[error("adoption provider verification limit must be greater than zero")]
    InvalidLimit,
    /// Durable state failed.
    #[error(transparent)]
    Database(#[from] DatabaseError),
    /// Fingerprint evidence could not be serialized.
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

#[derive(Debug, Error)]
enum AdoptionLinkBoundaryError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Download(#[from] crate::yt_dlp::YtDlpError),
    #[error(transparent)]
    Validation(#[from] crate::acquisition::StagedMediaValidationError),
    #[error(transparent)]
    Fingerprint(#[from] crate::fingerprint::FingerprintError),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("stored reference fingerprint is invalid")]
    InvalidReference,
}

#[cfg(test)]
mod tests {
    use super::filename_contains_exact_id;
    use std::path::Path;

    #[test]
    fn filename_id_matching_requires_token_boundaries() {
        assert!(filename_contains_exact_id(
            Path::new("Track [Ka4RGy8H2rs].m4a"),
            "Ka4RGy8H2rs"
        ));
        assert!(filename_contains_exact_id(
            Path::new("Ka4RGy8H2rs.opus"),
            "Ka4RGy8H2rs"
        ));
        assert!(filename_contains_exact_id(
            Path::new("044 - K. [youtube-26Uo_l5Iipo].m4a"),
            "26Uo_l5Iipo"
        ));
        assert!(!filename_contains_exact_id(
            Path::new("xKa4RGy8H2rsy.m4a"),
            "Ka4RGy8H2rs"
        ));
    }
}
