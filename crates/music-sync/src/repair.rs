//! Conservative assessment and durable eligibility for lost-media repair.

use serde::Serialize;
use thiserror::Error;

use crate::persistence::{Database, DatabaseError, ProviderAvailability, RepairEligibilitySummary};
use crate::provider::ProviderFailureKind;
use crate::yt_dlp::YtDlp;

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
