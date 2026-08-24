//! Canonical metadata resolution and deterministic release selection.

use serde::Serialize;
use thiserror::Error;

use crate::musicbrainz::{CanonicalLookup, CanonicalMetadataProvider, CanonicalRelease};
use crate::persistence::{
    Database, DatabaseError, MetadataResolutionMethod, MetadataResolutionState,
};

/// Resolves a bounded stable set of strong recording identities.
pub fn resolve_canonical_metadata(
    database: &mut Database,
    provider: &dyn CanonicalMetadataProvider,
    maximum_recordings: usize,
) -> Result<MetadataResolutionReport, MetadataResolutionError> {
    if maximum_recordings == 0 {
        return Err(MetadataResolutionError::InvalidLimit);
    }
    let candidates = database.metadata_resolution_candidates(maximum_recordings)?;
    let mut report = MetadataResolutionReport {
        selected: candidates.len() as u64,
        ..MetadataResolutionReport::default()
    };
    for candidate in candidates {
        let (lookup, method) = if let Some(id) = &candidate.musicbrainz_recording_id {
            (
                CanonicalLookup::RecordingMbid(id.clone()),
                MetadataResolutionMethod::RecordingMbid,
            )
        } else if let Some(isrc) = &candidate.isrc {
            (
                CanonicalLookup::Isrc(isrc.clone()),
                MetadataResolutionMethod::Isrc,
            )
        } else {
            continue;
        };
        let result = match provider.resolve(&lookup) {
            Ok(result) => result,
            Err(error) => {
                database.record_metadata_resolution_state(
                    candidate.recording_id,
                    MetadataResolutionState::Deferred,
                    method,
                    0,
                    &bounded_message(&error.to_string()),
                    "{}",
                )?;
                report.deferred += 1;
                report.failures.push(MetadataResolutionFailure {
                    recording_id: candidate.recording_id,
                    message: error.to_string(),
                });
                continue;
            }
        };
        let matching = result
            .recordings
            .iter()
            .filter(|recording| match &lookup {
                CanonicalLookup::RecordingMbid(id) => recording.id == *id,
                CanonicalLookup::Isrc(isrc) => recording.isrcs.contains(isrc),
            })
            .collect::<Vec<_>>();
        if matching.len() != 1 {
            database.record_metadata_resolution_state(
                candidate.recording_id,
                MetadataResolutionState::Ambiguous,
                method,
                matching.len(),
                "strong identifier did not produce exactly one recording",
                &result.raw_response_json,
            )?;
            report.ambiguous += 1;
            continue;
        }
        let recording = matching[0];
        let release = select_release(&recording.releases);
        database.record_resolved_metadata(
            &candidate,
            method,
            recording,
            release,
            &result.raw_response_json,
        )?;
        report.resolved += 1;
    }
    Ok(report)
}

/// Deterministically selects release context without changing recording identity.
#[must_use]
pub fn select_release(releases: &[CanonicalRelease]) -> Option<&CanonicalRelease> {
    releases.iter().min_by_key(|release| {
        let official = u8::from(
            !release
                .status
                .as_deref()
                .is_some_and(|value| value.eq_ignore_ascii_case("official")),
        );
        let compilation = u8::from(
            release
                .secondary_types
                .iter()
                .any(|value| value.eq_ignore_ascii_case("compilation")),
        );
        let primary_type = match release.primary_type.as_deref() {
            Some(value) if value.eq_ignore_ascii_case("album") => 0_u8,
            Some(value) if value.eq_ignore_ascii_case("ep") => 1,
            Some(value) if value.eq_ignore_ascii_case("single") => 2,
            Some(_) => 3,
            None => 4,
        };
        (
            official,
            compilation,
            primary_type,
            release.date.as_deref().unwrap_or("9999"),
            release.id.as_str(),
        )
    })
}

fn bounded_message(message: &str) -> String {
    message.chars().take(4_096).collect()
}

/// Bounded canonical metadata resolution effects.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct MetadataResolutionReport {
    /// Strong-identity recordings selected in stable order.
    pub selected: u64,
    /// Recordings resolved unambiguously.
    pub resolved: u64,
    /// Strong identifiers with zero or multiple matching recordings.
    pub ambiguous: u64,
    /// Retryable provider/infrastructure failures.
    pub deferred: u64,
    /// Isolated provider failures.
    pub failures: Vec<MetadataResolutionFailure>,
}

/// One retryable provider failure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MetadataResolutionFailure {
    /// Durable target recording.
    pub recording_id: i64,
    /// Bounded provider diagnostic.
    pub message: String,
}

/// Fatal bounded metadata orchestration failure.
#[derive(Debug, Error)]
pub enum MetadataResolutionError {
    /// Work must have a non-zero explicit bound.
    #[error("maximum metadata recordings must be greater than zero")]
    InvalidLimit,
    /// Durable state failed.
    #[error(transparent)]
    Database(#[from] DatabaseError),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release(
        id: &str,
        status: &str,
        primary_type: &str,
        secondary_types: &[&str],
        date: &str,
    ) -> CanonicalRelease {
        CanonicalRelease {
            id: id.into(),
            title: id.into(),
            date: Some(date.into()),
            country: None,
            status: Some(status.into()),
            release_group_id: None,
            primary_type: Some(primary_type.into()),
            secondary_types: secondary_types
                .iter()
                .map(|value| (*value).into())
                .collect(),
        }
    }

    #[test]
    fn release_selection_prefers_official_non_compilation_album_deterministically() {
        let releases = vec![
            release("compilation", "Official", "Album", &["Compilation"], "1990"),
            release("bootleg", "Bootleg", "Album", &[], "1980"),
            release("single", "Official", "Single", &[], "1985"),
            release("album", "Official", "Album", &[], "2000"),
        ];
        assert_eq!(
            select_release(&releases).map(|release| release.id.as_str()),
            Some("album")
        );
    }
}
