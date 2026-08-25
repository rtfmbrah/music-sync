//! Conservative routing from approved discovery identity to provider acquisition.

use serde::Serialize;
use thiserror::Error;

use crate::musicbrainz::{CanonicalLookup, CanonicalMetadataProvider, MusicBrainzError};
use crate::persistence::{Database, DatabaseError, DiscoveryAcquisitionRoute};
use crate::provider::youtube_video_id;

/// Resolves a bounded approved snapshot without text search or speculative identity.
pub fn route_approved_discovery(
    database: &mut Database,
    provider: &dyn CanonicalMetadataProvider,
    maximum: usize,
) -> Result<DiscoveryRoutingReport, DiscoveryRoutingError> {
    let candidates = database.approved_discovery_candidates(maximum)?;
    let mut report = DiscoveryRoutingReport {
        selected: candidates.len() as u64,
        ..Default::default()
    };
    for candidate in candidates {
        let response = match provider.resolve(&CanonicalLookup::RecordingMbid(
            candidate.recording_mbid.clone(),
        )) {
            Ok(response) => response,
            Err(error) => {
                report.deferred += 1;
                report.failures.push(format!(
                    "candidate {} canonical provider deferred: {error}",
                    candidate.id
                ));
                continue;
            }
        };
        let Some(recording) = response
            .recordings
            .into_iter()
            .find(|recording| recording.id == candidate.recording_mbid)
        else {
            database.mark_discovery_unresolved(
                candidate.id,
                "MusicBrainz did not return the exact requested recording",
            )?;
            report.unresolved += 1;
            continue;
        };
        let routes = recording
            .url_relations
            .iter()
            .filter_map(|relation| {
                youtube_video_id(&relation.resource).map(|provider_item_id| {
                    (
                        provider_item_id,
                        relation.resource.clone(),
                        relation.relation_type.clone(),
                    )
                })
            })
            .collect::<Vec<_>>();
        let Some(duration) = recording.length_ms else {
            database.mark_discovery_unresolved(
                candidate.id,
                "canonical recording duration is unavailable",
            )?;
            report.unresolved += 1;
            continue;
        };
        if routes.len() != 1 {
            database.mark_discovery_unresolved(
                candidate.id,
                "exactly one recording-level YouTube relationship is required",
            )?;
            report.unresolved += 1;
            continue;
        }
        let (provider_item_id, url, relation_type) = &routes[0];
        let canonical_isrc = (recording.isrcs.len() == 1).then(|| recording.isrcs[0].clone());
        let assertion_json = serde_json::json!({
            "musicbrainz_recording_id": recording.id,
            "relationship_type": relation_type,
            "relationship_url": url,
            "canonical_duration_ms": duration,
            "canonical_isrc": canonical_isrc,
        })
        .to_string();
        database.queue_discovery_acquisition(
            &candidate,
            &DiscoveryAcquisitionRoute {
                provider_item_id: provider_item_id.clone(),
                url: url.clone(),
                canonical_duration_ms: duration,
                canonical_isrc,
                assertion_json,
            },
        )?;
        report.queued += 1;
    }
    Ok(report)
}

/// Effects of one bounded discovery routing pass.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize)]
pub struct DiscoveryRoutingReport {
    /// Approved candidates considered.
    pub selected: u64,
    /// Strongly evidenced acquisition jobs created.
    pub queued: u64,
    /// Definitively insufficient canonical relationships.
    pub unresolved: u64,
    /// Provider failures left approved for a future pass.
    pub deferred: u64,
    /// Bounded diagnostic messages for deferred provider calls.
    pub failures: Vec<String>,
}

/// Discovery routing failure.
#[derive(Debug, Error)]
pub enum DiscoveryRoutingError {
    /// Durable state failed.
    #[error(transparent)]
    Database(#[from] DatabaseError),
    /// Reserved for explicit provider propagation at single-item boundaries.
    #[error(transparent)]
    Provider(#[from] MusicBrainzError),
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::acquisition::ValidatedStagedMedia;
    use crate::config::DiscoveryConfig;
    use crate::discovery::{DiscoveryLane, Recommendation};
    use crate::musicbrainz::{CanonicalLookupResult, CanonicalRecording, CanonicalUrlRelation};

    struct FixtureProvider;
    impl CanonicalMetadataProvider for FixtureProvider {
        fn resolve(
            &self,
            lookup: &CanonicalLookup,
        ) -> Result<CanonicalLookupResult, MusicBrainzError> {
            let CanonicalLookup::RecordingMbid(id) = lookup else {
                unreachable!()
            };
            Ok(CanonicalLookupResult {
                recordings: vec![CanonicalRecording {
                    id: id.clone(),
                    title: "Exact fixture".into(),
                    length_ms: Some(180_000),
                    isrcs: vec!["USABC2412345".into()],
                    artist_credit: Vec::new(),
                    releases: Vec::new(),
                    url_relations: vec![CanonicalUrlRelation {
                        relation_type: "video".into(),
                        resource: "https://youtu.be/Exact_video".into(),
                    }],
                }],
                raw_response_json: "{}".into(),
            })
        }
    }

    #[test]
    fn exact_relationship_queues_but_staging_still_requires_identity_and_duration()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut database = Database::open_in_memory()?;
        let config = DiscoveryConfig {
            enabled: true,
            target_new_tracks_per_day: 1,
            max_new_tracks_per_day: 1,
            max_tracks_per_artist_per_day: 1,
            exploration_ratio: 0.0,
            wildcard_ratio: 0.0,
            minimum_free_disk_gb: 0,
            listenbrainz_user: Some("fixture".into()),
            navidrome_url: None,
            navidrome_user: None,
        };
        database.record_discovery_candidates(
            "fixture",
            &[Recommendation {
                recording_mbid: "11111111-2222-3333-4444-555555555555".into(),
                artist_mbid: Some("aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee".into()),
                lane: DiscoveryLane::Adjacent,
                provider_score_millionths: 900_000,
                seed_weight_millionths: 900_000,
                reasons: vec!["fixture".into()],
            }],
            &config,
            10 * 1024 * 1024 * 1024,
            0,
        )?;
        let report = route_approved_discovery(&mut database, &FixtureProvider, 10)?;
        assert_eq!(report.queued, 1);
        let work = database
            .claim_next_acquisition()?
            .ok_or("queued acquisition was missing")?;
        let invalid = ValidatedStagedMedia {
            path: PathBuf::from("/tmp/fixture.opus"),
            sha256: "00".repeat(32),
            bytes: 1,
            codec: "opus".into(),
            duration_ms: Some(180_000),
            sample_rate_hz: Some(48_000),
            channels: Some(2),
            musicbrainz_recording_id: Some("99999999-9999-9999-9999-999999999999".into()),
            isrc: None,
        };
        assert!(matches!(
            database.prepare_acquisition_commit(
                work.job_id,
                &invalid,
                PathBuf::from("/tmp/final.opus").as_path()
            ),
            Err(DatabaseError::DiscoveryAcquisitionEvidenceMismatch(_))
        ));
        let valid = ValidatedStagedMedia {
            musicbrainz_recording_id: Some("11111111-2222-3333-4444-555555555555".into()),
            ..invalid
        };
        assert!(database.prepare_acquisition_commit(
            work.job_id,
            &valid,
            PathBuf::from("/tmp/final.opus").as_path()
        )?);
        Ok(())
    }
}
