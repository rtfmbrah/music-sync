//! Optional yt-dlp search and AcoustID verification for approved discovery.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;
use thiserror::Error;

use crate::acoustid::{AcoustIdDecision, AcoustIdProvider};
use crate::acquisition::{StagedMediaValidationError, validate_staged_media};
use crate::content_hash::ContentHasher;
use crate::fingerprint::{CompressedFingerprinter, FingerprintError};
use crate::media_probe::MediaProbe;
use crate::musicbrainz::CanonicalRecording;
use crate::persistence::{
    ApprovedDiscoveryCandidate, Database, DatabaseError, DiscoverySearchCandidateInput,
    DiscoverySearchEvidence,
};
use crate::provider::ProviderItem;
use crate::yt_dlp::{YtDlp, YtDlpError};

const DURATION_TOLERANCE_MS: u64 = 2_000;
const MAX_MESSAGE_CHARS: usize = 4_096;

/// External boundaries and explicit limits for one optional search pass.
pub struct DiscoverySearchBoundaries<'a> {
    /// Durable staging root.
    pub state_directory: &'a Path,
    /// Serial yt-dlp search and download adapter.
    pub yt_dlp: &'a YtDlp,
    /// Structural media probe.
    pub probe: &'a dyn MediaProbe,
    /// Exact-byte hasher.
    pub hasher: &'a dyn ContentHasher,
    /// Compressed Chromaprint extractor.
    pub fingerprinter: &'a dyn CompressedFingerprinter,
    /// Exact MBID corroboration boundary.
    pub acoustid: &'a dyn AcoustIdProvider,
    /// Maximum provider candidates per recommendation.
    pub maximum_candidates: usize,
    /// Minimum exact-MBID confidence.
    pub minimum_score_millionths: u32,
}

/// Runs a bounded serial search, stopping immediately on transient infrastructure
/// failure and queueing at most one positively identified candidate.
pub fn search_and_verify_discovery(
    database: &mut Database,
    discovery_candidate: &ApprovedDiscoveryCandidate,
    canonical: &CanonicalRecording,
    boundaries: &DiscoverySearchBoundaries<'_>,
) -> Result<DiscoverySearchReport, DiscoverySearchError> {
    if boundaries.maximum_candidates == 0 {
        return Err(DiscoverySearchError::InvalidLimit);
    }
    let duration = canonical
        .length_ms
        .ok_or(DiscoverySearchError::MissingCanonicalDuration)?;
    let Some(artist) = canonical_artist(canonical) else {
        database.mark_discovery_unresolved(
            discovery_candidate.id,
            "canonical recording artist credit is unavailable",
        )?;
        return Ok(DiscoverySearchReport {
            unresolved: 1,
            ..DiscoverySearchReport::default()
        });
    };
    let query = format!(
        "ytsearch{}:{artist} - {}",
        boundaries.maximum_candidates, canonical.title
    );
    let snapshot = match boundaries.yt_dlp.enumerate(&query) {
        Ok(snapshot) => snapshot,
        Err(error) => {
            return Ok(DiscoverySearchReport {
                deferred: 1,
                failures: vec![bounded(error.to_string())],
                ..DiscoverySearchReport::default()
            });
        }
    };
    let inputs = snapshot
        .items
        .into_iter()
        .take(boundaries.maximum_candidates)
        .enumerate()
        .map(|(ordinal, item)| {
            let verdict = prefilter(&item, canonical, &artist, duration);
            DiscoverySearchCandidateInput {
                ordinal,
                item,
                state: if verdict.is_ok() {
                    "generated".into()
                } else {
                    "rejected".into()
                },
                message: verdict.unwrap_or_else(|message| message),
            }
        })
        .collect::<Vec<_>>();
    let persisted =
        database.record_discovery_search_candidates(discovery_candidate.id, &query, &inputs)?;
    let mut report = DiscoverySearchReport {
        generated: inputs.len() as u64,
        rejected: inputs
            .iter()
            .filter(|item| item.state == "rejected")
            .count() as u64,
        ..DiscoverySearchReport::default()
    };
    if let Some(verified) = persisted
        .iter()
        .find(|candidate| candidate.state == "verified")
    {
        let canonical_isrc = (canonical.isrcs.len() == 1).then(|| canonical.isrcs[0].as_str());
        database.queue_verified_discovery_search(
            discovery_candidate,
            verified.id,
            duration,
            canonical_isrc,
            &serde_json::json!({
                "musicbrainz_recording_id": canonical.id,
                "identity_source": "acoustid_fingerprint",
                "recovered_verified_search_candidate": verified.id,
            })
            .to_string(),
        )?;
        report.verified += 1;
        report.queued += 1;
        return Ok(report);
    }
    let staging_root = boundaries.state_directory.join("discovery-search-staging");
    fs::create_dir_all(&staging_root).map_err(|source| DiscoverySearchError::CreateStaging {
        path: staging_root.clone(),
        source,
    })?;

    for candidate in persisted
        .into_iter()
        .filter(|candidate| candidate.state == "generated")
    {
        let staging = staging_root.join(format!("candidate-{}", candidate.id));
        fs::create_dir_all(&staging).map_err(|source| DiscoverySearchError::CreateStaging {
            path: staging.clone(),
            source,
        })?;
        let downloaded = match boundaries
            .yt_dlp
            .download(&candidate.provider_url, &staging)
        {
            Ok(downloaded) => downloaded,
            Err(error) => {
                database.mark_discovery_search_candidate(
                    candidate.id,
                    "deferred",
                    &bounded(error.to_string()),
                )?;
                report.deferred += 1;
                report.failures.push(bounded(error.to_string()));
                return Ok(report);
            }
        };
        let validated =
            match validate_staged_media(&downloaded.path, boundaries.probe, boundaries.hasher) {
                Ok(validated) if downloaded.bytes == validated.bytes => validated,
                Ok(_) => {
                    let message = "staged discovery bytes changed during validation";
                    database.mark_discovery_search_candidate(candidate.id, "deferred", message)?;
                    report.deferred += 1;
                    report.failures.push(message.into());
                    return Ok(report);
                }
                Err(error) => {
                    database.mark_discovery_search_candidate(
                        candidate.id,
                        "deferred",
                        &bounded(error.to_string()),
                    )?;
                    report.deferred += 1;
                    report.failures.push(bounded(error.to_string()));
                    return Ok(report);
                }
            };
        if validated
            .duration_ms
            .is_none_or(|actual| actual.abs_diff(duration) > DURATION_TOLERANCE_MS)
        {
            database.mark_discovery_search_candidate(
                candidate.id,
                "rejected",
                "validated media duration contradicts the canonical recording",
            )?;
            report.rejected += 1;
            continue;
        }
        let fingerprint = match boundaries
            .fingerprinter
            .compressed_fingerprint(&validated.path)
        {
            Ok(fingerprint) => fingerprint,
            Err(error) => {
                database.mark_discovery_search_candidate(
                    candidate.id,
                    "deferred",
                    &bounded(error.to_string()),
                )?;
                report.deferred += 1;
                report.failures.push(bounded(error.to_string()));
                return Ok(report);
            }
        };
        let evidence = match boundaries.acoustid.corroborate(
            &discovery_candidate.recording_mbid,
            &fingerprint,
            boundaries.minimum_score_millionths,
        ) {
            Ok(evidence) => evidence,
            Err(error) => {
                database.mark_discovery_search_candidate(
                    candidate.id,
                    "deferred",
                    &bounded(error.to_string()),
                )?;
                report.deferred += 1;
                report.failures.push(bounded(error.to_string()));
                return Ok(report);
            }
        };
        let (state, message) = match evidence.decision {
            AcoustIdDecision::Verified => (
                "verified",
                "AcoustID returned the exact expected MusicBrainz recording without contradiction",
            ),
            AcoustIdDecision::Contradicted => (
                "rejected",
                "AcoustID returned a contradictory high-confidence MusicBrainz recording",
            ),
            AcoustIdDecision::Insufficient => (
                "unresolved",
                "AcoustID coverage or confidence was insufficient",
            ),
        };
        database.record_discovery_search_evidence(&DiscoverySearchEvidence {
            search_candidate_id: candidate.id,
            state: state.into(),
            message: message.into(),
            staged_path: validated.path.clone(),
            staged_sha256: validated.sha256.clone(),
            staged_bytes: validated.bytes,
            fingerprint_duration_seconds: fingerprint.duration_seconds,
            acoustid_decision: match evidence.decision {
                AcoustIdDecision::Verified => "verified",
                AcoustIdDecision::Contradicted => "contradicted",
                AcoustIdDecision::Insufficient => "insufficient",
            }
            .into(),
            acoustid_score_millionths: evidence.expected_score_millionths,
            acoustid_response_json: evidence.raw_response_json,
            provider_metadata: downloaded.raw_metadata,
        })?;
        match evidence.decision {
            AcoustIdDecision::Verified => {
                let canonical_isrc =
                    (canonical.isrcs.len() == 1).then(|| canonical.isrcs[0].as_str());
                let assertion = serde_json::json!({
                    "musicbrainz_recording_id": canonical.id,
                    "canonical_title": canonical.title,
                    "canonical_artist": artist,
                    "canonical_duration_ms": duration,
                    "canonical_isrc": canonical_isrc,
                    "provider_item_id": candidate.provider_item_id,
                    "provider_url": candidate.provider_url,
                    "identity_source": "acoustid_fingerprint",
                    "staged_sha256": validated.sha256,
                    "acoustid_score_millionths": evidence.expected_score_millionths,
                })
                .to_string();
                database.queue_verified_discovery_search(
                    discovery_candidate,
                    candidate.id,
                    duration,
                    canonical_isrc,
                    &assertion,
                )?;
                report.verified += 1;
                report.queued += 1;
                return Ok(report);
            }
            AcoustIdDecision::Contradicted => report.rejected += 1,
            AcoustIdDecision::Insufficient => report.unresolved += 1,
        }
    }
    if report.deferred == 0 {
        database.mark_discovery_unresolved(
            discovery_candidate.id,
            "no searched YouTube candidate had sufficient non-contradictory AcoustID evidence",
        )?;
    }
    Ok(report)
}

fn canonical_artist(recording: &CanonicalRecording) -> Option<String> {
    let value = recording
        .artist_credit
        .iter()
        .map(|credit| format!("{}{}", credit.credited_name, credit.join_phrase))
        .collect::<String>();
    (!value.trim().is_empty()).then(|| value.trim().to_owned())
}

fn prefilter(
    item: &ProviderItem,
    canonical: &CanonicalRecording,
    artist: &str,
    duration_ms: u64,
) -> Result<String, String> {
    let title = item
        .title
        .as_deref()
        .ok_or_else(|| "provider candidate lacks a title".to_owned())?;
    let duration = item
        .duration_ms
        .ok_or_else(|| "provider candidate lacks a duration".to_owned())?;
    if duration.abs_diff(duration_ms) > DURATION_TOLERANCE_MS {
        return Err("provider duration contradicts the canonical recording".into());
    }
    let metadata_text = [
        Some(title),
        item.raw_metadata
            .get("uploader")
            .and_then(serde_json::Value::as_str),
        item.raw_metadata
            .get("channel")
            .and_then(serde_json::Value::as_str),
        item.raw_metadata
            .get("artist")
            .and_then(serde_json::Value::as_str),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" ");
    let normalized = normalize(&metadata_text);
    if !normalized.contains(&normalize(&canonical.title)) {
        return Err("provider title does not contain the canonical title".into());
    }
    let artist_tokens = normalize(artist);
    if !artist_tokens
        .split_whitespace()
        .filter(|token| token.len() > 1)
        .all(|token| normalized.split_whitespace().any(|value| value == token))
    {
        return Err("provider metadata does not contain the canonical artist credit".into());
    }
    if qualifiers(&canonical.title) != qualifiers(title) {
        return Err("provider version qualifiers contradict the canonical recording".into());
    }
    Ok("canonical text, artist, duration, and version prefilter passed".into())
}

fn normalize(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_alphanumeric() {
                character.to_ascii_lowercase()
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn qualifiers(value: &str) -> BTreeSet<&'static str> {
    const QUALIFIERS: &[(&str, &str)] = &[
        ("acoustic", "acoustic"),
        ("bootleg", "bootleg"),
        ("cover", "cover"),
        ("demo", "demo"),
        ("extended", "extended"),
        ("instrumental", "instrumental"),
        ("karaoke", "karaoke"),
        ("live", "live"),
        ("nightcore", "nightcore"),
        ("radio edit", "radio_edit"),
        ("remaster", "remaster"),
        ("remix", "remix"),
        ("slowed", "slowed"),
        ("sped up", "sped_up"),
    ];
    let normalized = normalize(value);
    QUALIFIERS
        .iter()
        .filter_map(|(needle, label)| normalized.contains(needle).then_some(*label))
        .collect()
}

fn bounded(message: String) -> String {
    message.chars().take(MAX_MESSAGE_CHARS).collect()
}

/// Aggregate effects of one bounded discovery search.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct DiscoverySearchReport {
    /// Provider results persisted.
    pub generated: u64,
    /// Candidates rejected by canonical or contradictory evidence.
    pub rejected: u64,
    /// Candidates positively identified by AcoustID.
    pub verified: u64,
    /// Candidates with insufficient public fingerprint coverage.
    pub unresolved: u64,
    /// Retryable provider/infrastructure failures.
    pub deferred: u64,
    /// Verified acquisitions queued for ordinary atomic commit.
    pub queued: u64,
    /// Bounded retryable diagnostics.
    pub failures: Vec<String>,
}

/// Fatal setup or durable-state failure for discovery search.
#[derive(Debug, Error)]
pub enum DiscoverySearchError {
    /// Candidate limit must be positive.
    #[error("discovery search candidate limit must be greater than zero")]
    InvalidLimit,
    /// MusicBrainz duration is required before provider search.
    #[error("canonical recording duration is unavailable")]
    MissingCanonicalDuration,
    /// Durable database operation failed.
    #[error(transparent)]
    Database(#[from] DatabaseError),
    /// Stable discovery staging could not be created.
    #[error("failed to create discovery staging {path}: {source}")]
    CreateStaging {
        /// Requested staging directory.
        path: PathBuf,
        /// Underlying filesystem failure.
        source: std::io::Error,
    },
    /// Reserved for explicit typed boundary propagation in future single-item commands.
    #[error(transparent)]
    Provider(#[from] YtDlpError),
    /// Reserved for explicit typed media validation propagation.
    #[error(transparent)]
    Validation(#[from] StagedMediaValidationError),
    /// Reserved for explicit typed fingerprint propagation.
    #[error(transparent)]
    Fingerprint(#[from] FingerprintError),
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;
    use std::time::Duration;

    use super::*;
    use crate::acoustid::{AcoustIdError, AcoustIdEvidence};
    use crate::acquisition::{AcquisitionRunOutcome, run_one_acquisition};
    use crate::config::DiscoveryConfig;
    use crate::content_hash::Sha256FileHasher;
    use crate::discovery::{DiscoveryLane, Recommendation};
    use crate::fingerprint::CompressedFingerprint;
    use crate::media_probe::{MediaProbeError, MediaProperties, MediaTagPresence};
    use crate::musicbrainz::CanonicalArtistCredit;

    fn recording(title: &str) -> CanonicalRecording {
        CanonicalRecording {
            id: "11111111-1111-1111-1111-111111111111".into(),
            title: title.into(),
            length_ms: Some(180_000),
            isrcs: Vec::new(),
            artist_credit: vec![CanonicalArtistCredit {
                artist_id: "22222222-2222-2222-2222-222222222222".into(),
                artist_name: "Artist".into(),
                sort_name: None,
                disambiguation: None,
                credited_name: "Artist".into(),
                join_phrase: String::new(),
            }],
            releases: Vec::new(),
            url_relations: Vec::new(),
        }
    }

    fn item(title: &str) -> ProviderItem {
        ProviderItem {
            provider_item_id: "video".into(),
            url: "https://youtu.be/video".into(),
            title: Some(title.into()),
            duration_ms: Some(181_000),
            raw_metadata: serde_json::json!({"uploader":"Artist - Topic"}),
        }
    }

    #[test]
    fn prefilter_preserves_remix_identity() {
        assert!(
            prefilter(
                &item("Artist - Song (Frenchcore Remix)"),
                &recording("Song (Frenchcore Remix)"),
                "Artist",
                180_000,
            )
            .is_ok()
        );
        assert_eq!(
            prefilter(
                &item("Artist - Song"),
                &recording("Song (Frenchcore Remix)"),
                "Artist",
                180_000,
            ),
            Err("provider title does not contain the canonical title".into())
        );
    }

    #[test]
    fn prefilter_requires_artist_and_duration() {
        let mut wrong_artist = item("Someone - Song");
        wrong_artist.raw_metadata = serde_json::json!({"uploader":"Someone"});
        assert!(prefilter(&wrong_artist, &recording("Song"), "Artist", 180_000).is_err());
        let mut wrong_duration = item("Artist - Song");
        wrong_duration.duration_ms = Some(190_000);
        assert!(prefilter(&wrong_duration, &recording("Song"), "Artist", 180_000).is_err());
    }

    struct FixtureProbe;
    impl MediaProbe for FixtureProbe {
        fn probe(&self, _path: &Path) -> Result<MediaProperties, MediaProbeError> {
            Ok(MediaProperties {
                codec: "opus".into(),
                duration_ms: Some(180_000),
                sample_rate_hz: Some(48_000),
                channels: Some(2),
                has_embedded_artwork: false,
                tags: MediaTagPresence::default(),
                musicbrainz_recording_id: None,
                isrc: None,
            })
        }
    }

    struct FixtureFingerprinter;
    impl CompressedFingerprinter for FixtureFingerprinter {
        fn compressed_fingerprint(
            &self,
            _path: &Path,
        ) -> Result<CompressedFingerprint, FingerprintError> {
            Ok(CompressedFingerprint {
                duration_seconds: 180,
                fingerprint: "AQAD_fixture".into(),
            })
        }
    }

    struct FixtureAcoustId;
    impl AcoustIdProvider for FixtureAcoustId {
        fn corroborate(
            &self,
            expected_recording_mbid: &str,
            _fingerprint: &CompressedFingerprint,
            _minimum_score_millionths: u32,
        ) -> Result<AcoustIdEvidence, AcoustIdError> {
            Ok(AcoustIdEvidence {
                decision: AcoustIdDecision::Verified,
                expected_score_millionths: Some(990_000),
                contradictory_recording_mbids: Vec::new(),
                raw_response_json: format!(
                    "{{\"status\":\"ok\",\"recording\":\"{expected_recording_mbid}\"}}"
                ),
            })
        }
    }

    #[test]
    fn verified_search_reuses_exact_staging_for_atomic_acquisition()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let state = root.path().join("state");
        let library = root.path().join("library");
        fs::create_dir_all(&state)?;
        fs::create_dir_all(&library)?;
        let executable = root.path().join("yt-dlp");
        fs::write(
            &executable,
            r#"#!/bin/sh
set -eu
for arg in "$@"; do
  if test "$arg" = "--flat-playlist"; then
    printf '%s' '{"id":"search","title":"search","entries":[{"id":"video1","webpage_url":"https://youtu.be/video1","title":"Artist - Song (Frenchcore Remix)","duration":180,"uploader":"Artist - Topic"}]}'
    exit 0
  fi
done
staging=
prior=
for arg in "$@"; do
  if test "$prior" = "--paths"; then staging=$arg; fi
  prior=$arg
done
test -n "$staging"
printf '%s' 'verified audio fixture' > "$staging/media.opus"
printf '%s' '{"id":"video1","webpage_url":"https://youtu.be/video1","title":"Song (Frenchcore Remix)","duration":180,"artist":"Artist","uploader":"Artist - Topic"}' > "$staging/media.info.json"
printf '%s\n' "$staging/media.opus"
"#,
        )?;
        let mut permissions = fs::metadata(&executable)?.permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(&executable, permissions)?;
        let yt_dlp = YtDlp::new(executable, Duration::from_secs(2));

        let mut database = Database::open(&state.join("music-sync.sqlite3"))?;
        let config = DiscoveryConfig {
            enabled: true,
            target_new_tracks_per_day: 1,
            max_new_tracks_per_day: 1,
            max_tracks_per_artist_per_day: 1,
            exploration_ratio: 0.0,
            wildcard_ratio: 0.0,
            minimum_free_disk_gb: 0,
            listenbrainz_user: Some("fixture".into()),
            ..DiscoveryConfig::default()
        };
        database.record_discovery_candidates(
            "fixture",
            &[Recommendation {
                recording_mbid: "11111111-1111-1111-1111-111111111111".into(),
                artist_mbid: Some("22222222-2222-2222-2222-222222222222".into()),
                lane: DiscoveryLane::Adjacent,
                provider_score_millionths: 990_000,
                seed_weight_millionths: 990_000,
                reasons: vec!["fixture".into()],
            }],
            &config,
            10_000_000_000,
            0,
        )?;
        let candidate = database
            .approved_discovery_candidates(1)?
            .into_iter()
            .next()
            .ok_or("approved fixture candidate missing")?;
        let canonical = recording("Song (Frenchcore Remix)");
        let report = search_and_verify_discovery(
            &mut database,
            &candidate,
            &canonical,
            &DiscoverySearchBoundaries {
                state_directory: &state,
                yt_dlp: &yt_dlp,
                probe: &FixtureProbe,
                hasher: &Sha256FileHasher,
                fingerprinter: &FixtureFingerprinter,
                acoustid: &FixtureAcoustId,
                maximum_candidates: 3,
                minimum_score_millionths: 950_000,
            },
        )?;
        assert_eq!(report.queued, 1);

        let outcome = run_one_acquisition(
            &mut database,
            &state,
            &library,
            &yt_dlp,
            &FixtureProbe,
            &Sha256FileHasher,
        )?;
        assert!(matches!(outcome, AcquisitionRunOutcome::Committed { .. }));
        assert_eq!(
            fs::read(library.join("youtube/video1.opus"))?,
            b"verified audio fixture"
        );
        assert!(database.approved_discovery_candidates(1)?.is_empty());
        Ok(())
    }
}
