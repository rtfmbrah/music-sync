//! Conservative external genre enrichment without treating text as recording identity.

use serde::Serialize;
use thiserror::Error;

use crate::musicbrainz::{GenreMetadataProvider, GenreRecordingCandidate};
use crate::persistence::{Database, DatabaseError, GenreResolutionCandidate, GenreResolutionState};

/// Resolves genres for a stable bounded set of owned recordings.
pub fn resolve_genres(
    database: &mut Database,
    provider: &dyn GenreMetadataProvider,
    limit: usize,
) -> Result<GenreResolutionReport, GenreResolutionError> {
    if limit == 0 {
        return Err(GenreResolutionError::InvalidLimit);
    }
    let candidates = database.genre_resolution_candidates(limit)?;
    let mut report = GenreResolutionReport {
        selected: candidates.len() as u64,
        ..Default::default()
    };
    for candidate in candidates {
        let lookup = match provider.genres(
            candidate.musicbrainz_recording_id.as_deref(),
            &candidate.title,
            &candidate.artist,
        ) {
            Ok(value) => value,
            Err(error) => {
                database.record_genre_resolution(
                    &candidate,
                    GenreResolutionState::Deferred,
                    None,
                    &[],
                    "recording",
                    0,
                    &error.to_string(),
                    "{}",
                )?;
                report.deferred += 1;
                report.failures.push(GenreResolutionFailure {
                    recording_id: candidate.recording_id,
                    message: error.to_string(),
                });
                continue;
            }
        };
        let matching = lookup
            .candidates
            .iter()
            .filter(|value| identity_matches(&candidate, value))
            .collect::<Vec<_>>();
        if matching.len() == 1 && !matching[0].genres.is_empty() {
            let selected = matching[0];
            database.record_genre_resolution(
                &candidate,
                GenreResolutionState::Resolved,
                Some(&selected.id),
                &selected.genres,
                "recording",
                1_000_000,
                "identity-verified MusicBrainz recording genres",
                &lookup.raw_response_json,
            )?;
            report.resolved += 1;
            report.genres_selected += selected.genres.len() as u64;
            continue;
        }
        let artist = match provider.artist_genres(&candidate.artist) {
            Ok(value) => value,
            Err(error) => {
                database.record_genre_resolution(
                    &candidate,
                    GenreResolutionState::Deferred,
                    None,
                    &[],
                    "artist",
                    0,
                    &error.to_string(),
                    "{}",
                )?;
                report.deferred += 1;
                report.failures.push(GenreResolutionFailure {
                    recording_id: candidate.recording_id,
                    message: error.to_string(),
                });
                continue;
            }
        };
        if let Some(artist_id) = artist.artist_id.filter(|_| !artist.genres.is_empty()) {
            database.record_genre_resolution(
                &candidate,
                GenreResolutionState::Resolved,
                Some(&artist_id),
                &artist.genres,
                "artist",
                650_000,
                "exact-name MusicBrainz artist genre fallback",
                &artist.raw_response_json,
            )?;
            report.resolved += 1;
            report.genres_selected += artist.genres.len() as u64;
        } else {
            let state = if matching.len() > 1 {
                GenreResolutionState::Ambiguous
            } else {
                GenreResolutionState::Unavailable
            };
            database.record_genre_resolution(
                &candidate,
                state,
                None,
                &[],
                "artist",
                0,
                "MusicBrainz has neither verified recording genres nor a unique exact artist genre fallback",
                &artist.raw_response_json,
            )?;
            if state == GenreResolutionState::Ambiguous {
                report.ambiguous += 1
            } else {
                report.unavailable += 1
            }
        }
    }
    Ok(report)
}

fn identity_matches(
    request: &GenreResolutionCandidate,
    candidate: &GenreRecordingCandidate,
) -> bool {
    if request
        .musicbrainz_recording_id
        .as_deref()
        .is_some_and(|id| id != candidate.id)
    {
        return false;
    }
    if normalize(&request.title) != normalize(&candidate.title)
        || normalize_artist(&request.artist) != normalize_artist(&candidate.artist_credit)
        || version_signature(&request.title) != version_signature(&candidate.title)
        || candidate.score < 100
    {
        return false;
    }
    match candidate.length_ms {
        Some(length) => request.duration_ms.abs_diff(length) <= 3_000,
        None => request.musicbrainz_recording_id.is_some(),
    }
}

fn normalize(value: &str) -> String {
    value
        .chars()
        .flat_map(char::to_lowercase)
        .filter(|ch| ch.is_alphanumeric())
        .collect()
}

fn normalize_artist(value: &str) -> String {
    let value = value.strip_suffix(" - Topic").unwrap_or(value);
    normalize(value)
}

fn version_signature(value: &str) -> Vec<&'static str> {
    const TERMS: &[&str] = &[
        "remix",
        "live",
        "acoustic",
        "instrumental",
        "karaoke",
        "cover",
        "remaster",
        "sped up",
        "slowed",
        "nightcore",
        "extended",
        "radio edit",
        "demo",
    ];
    let lower = value.to_lowercase();
    TERMS
        .iter()
        .copied()
        .filter(|term| lower.contains(term))
        .collect()
}

#[derive(Debug, Default, Serialize)]
/// Bounded genre-resolution effects.
pub struct GenreResolutionReport {
    /// Recordings selected for lookup.
    pub selected: u64,
    /// Recordings with identity-verified genre evidence.
    pub resolved: u64,
    /// Verified recordings without genres or without a unique match.
    pub unavailable: u64,
    /// Lookups producing multiple verified candidates.
    pub ambiguous: u64,
    /// Retryable provider failures.
    pub deferred: u64,
    /// Individual genre values selected.
    pub genres_selected: u64,
    /// Isolated retryable failures.
    pub failures: Vec<GenreResolutionFailure>,
}

#[derive(Debug, Serialize)]
/// One isolated genre-provider failure.
pub struct GenreResolutionFailure {
    /// Durable recording ID.
    pub recording_id: i64,
    /// Bounded provider diagnostic.
    pub message: String,
}

#[derive(Debug, Error)]
/// Fatal genre orchestration failure.
pub enum GenreResolutionError {
    /// Work requires a non-zero bound.
    #[error("maximum genre recordings must be greater than zero")]
    InvalidLimit,
    /// Durable state failed.
    #[error(transparent)]
    Database(#[from] DatabaseError),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(title: &str, artist: &str) -> GenreResolutionCandidate {
        GenreResolutionCandidate {
            recording_id: 1,
            musicbrainz_recording_id: None,
            title: title.into(),
            artist: artist.into(),
            duration_ms: 180_000,
        }
    }
    fn candidate(title: &str, artist: &str, duration: u64) -> GenreRecordingCandidate {
        GenreRecordingCandidate {
            id: "11111111-1111-1111-1111-111111111111".into(),
            title: title.into(),
            artist_credit: artist.into(),
            length_ms: Some(duration),
            score: 100,
            genres: vec!["metalcore".into()],
        }
    }
    #[test]
    fn requires_exact_normalized_identity_duration_and_version() {
        assert!(identity_matches(
            &request("Song (Remix)", "Artist - Topic"),
            &candidate("Song Remix", "Artist", 181_000)
        ));
        assert!(!identity_matches(
            &request("Song (Remix)", "Artist"),
            &candidate("Song", "Artist", 180_000)
        ));
        assert!(!identity_matches(
            &request("Song", "Artist"),
            &candidate("Song", "Other", 180_000)
        ));
        assert!(!identity_matches(
            &request("Song", "Artist"),
            &candidate("Song", "Artist", 184_000)
        ));
    }
}
