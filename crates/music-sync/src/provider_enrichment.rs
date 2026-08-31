//! Bounded full provider-metadata enrichment for managed media.

use serde::Serialize;
use thiserror::Error;

use crate::persistence::{Database, DatabaseError};
use crate::yt_dlp::YtDlp;

/// Resolves complete single-item metadata for a stable bounded candidate set.
pub fn enrich_provider_metadata(
    database: &mut Database,
    provider: &YtDlp,
    maximum_items: usize,
) -> Result<ProviderMetadataReport, ProviderMetadataError> {
    if maximum_items == 0 {
        return Err(ProviderMetadataError::InvalidLimit);
    }
    let candidates = database.provider_metadata_candidates(maximum_items)?;
    let mut report = ProviderMetadataReport {
        selected: candidates.len() as u64,
        ..ProviderMetadataReport::default()
    };
    for candidate in candidates {
        match provider.metadata(&candidate.original_url) {
            Ok(raw) => {
                database.record_provider_metadata(candidate.provider_item_id, &raw)?;
                report.resolved += 1;
            }
            Err(error) => report.failures.push(ProviderMetadataFailure {
                provider_item_id: candidate.provider_item_id,
                message: error.to_string(),
            }),
        }
    }
    Ok(report)
}

/// Aggregate provider-enrichment effects.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize)]
pub struct ProviderMetadataReport {
    /// Candidates selected at the beginning of this pass.
    pub selected: u64,
    /// Complete payloads stored.
    pub resolved: u64,
    /// Isolated provider failures.
    pub failures: Vec<ProviderMetadataFailure>,
}

/// One isolated provider-metadata failure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProviderMetadataFailure {
    /// Durable provider item.
    pub provider_item_id: i64,
    /// Bounded provider diagnostic.
    pub message: String,
}

/// Provider-enrichment workflow failure.
#[derive(Debug, Error)]
pub enum ProviderMetadataError {
    /// Work limit must be positive.
    #[error("maximum provider metadata items must be greater than zero")]
    InvalidLimit,
    /// Persistence failed.
    #[error(transparent)]
    Database(#[from] DatabaseError),
}
