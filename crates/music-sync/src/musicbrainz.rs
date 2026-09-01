//! Bounded MusicBrainz recording lookup with provider-neutral canonical results.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::Deserialize;
use thiserror::Error;

const MAX_RESPONSE_BYTES: u64 = 4 * 1024 * 1024;

/// Strong identifier allowed to initiate canonical recording resolution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CanonicalLookup {
    /// Exact MusicBrainz recording identity.
    RecordingMbid(String),
    /// Structurally valid normalized ISRC.
    Isrc(String),
}

/// External boundary for resolving strong recording identifiers.
pub trait CanonicalMetadataProvider {
    /// Resolves candidates without selecting an ambiguous result.
    fn resolve(&self, lookup: &CanonicalLookup) -> Result<CanonicalLookupResult, MusicBrainzError>;
}

/// External boundary for conservative recording-level genre lookup.
pub trait GenreMetadataProvider {
    /// Looks up either an exact MBID or a bounded title/artist candidate set.
    fn genres(
        &self,
        recording_mbid: Option<&str>,
        title: &str,
        artist: &str,
    ) -> Result<GenreLookupResult, MusicBrainzError>;

    /// Resolves a unique exact artist name and returns its MusicBrainz genres.
    fn artist_genres(&self, artist: &str) -> Result<ArtistGenreResult, MusicBrainzError>;
}

/// Exact artist-level genre fallback, kept distinct from recording evidence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtistGenreResult {
    /// Unique exact MusicBrainz artist identity, if one exists.
    pub artist_id: Option<String>,
    /// Positive MusicBrainz artist genres.
    pub genres: Vec<String>,
    /// Bounded search and entity responses retained for audit.
    pub raw_response_json: String,
}

/// Bounded MusicBrainz candidates carrying community genre evidence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GenreLookupResult {
    /// Bounded candidates returned by the provider.
    pub candidates: Vec<GenreRecordingCandidate>,
    /// Exact bounded response retained for audit.
    pub raw_response_json: String,
}

/// One candidate; callers must independently verify identity before using genres.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GenreRecordingCandidate {
    /// MusicBrainz recording identity.
    pub id: String,
    /// Canonical candidate title.
    pub title: String,
    /// Composed ordered artist credit.
    pub artist_credit: String,
    /// Recording duration when MusicBrainz provides it.
    pub length_ms: Option<u64>,
    /// MusicBrainz search score; exact lookup is 100.
    pub score: u16,
    /// Positive recording genres and tags in provider rank order.
    pub genres: Vec<String>,
}

/// Complete bounded provider response retained for selection and audit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanonicalLookupResult {
    /// Parsed recording candidates.
    pub recordings: Vec<CanonicalRecording>,
    /// Exact bounded JSON response.
    pub raw_response_json: String,
}

/// Provider-neutral canonical recording metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanonicalRecording {
    /// MusicBrainz recording MBID.
    pub id: String,
    /// Canonical recording title.
    pub title: String,
    /// Canonical duration when present.
    pub length_ms: Option<u64>,
    /// Associated normalized ISRCs.
    pub isrcs: Vec<String>,
    /// Ordered canonical artist credit.
    pub artist_credit: Vec<CanonicalArtistCredit>,
    /// Releases exposed by the recording lookup.
    pub releases: Vec<CanonicalRelease>,
    /// Recording-level external URL relationships retained for verified routing.
    pub url_relations: Vec<CanonicalUrlRelation>,
}

/// One canonical recording-to-URL relationship.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanonicalUrlRelation {
    /// MusicBrainz relationship type.
    pub relation_type: String,
    /// Exact external resource URL.
    pub resource: String,
}

/// One ordered artist-credit component.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanonicalArtistCredit {
    /// MusicBrainz artist MBID.
    pub artist_id: String,
    /// Canonical artist name.
    pub artist_name: String,
    /// MusicBrainz sort name.
    pub sort_name: Option<String>,
    /// Artist disambiguation comment.
    pub disambiguation: Option<String>,
    /// Credited display name for this recording.
    pub credited_name: String,
    /// Phrase joining this component to the next.
    pub join_phrase: String,
}

/// Canonical release candidate linked to a recording.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanonicalRelease {
    /// MusicBrainz release MBID.
    pub id: String,
    /// Release title.
    pub title: String,
    /// Release date at available precision.
    pub date: Option<String>,
    /// ISO country code when present.
    pub country: Option<String>,
    /// MusicBrainz release status.
    pub status: Option<String>,
    /// Release-group MBID.
    pub release_group_id: Option<String>,
    /// Release-group primary type.
    pub primary_type: Option<String>,
    /// Release-group secondary types.
    pub secondary_types: Vec<String>,
}

/// Blocking bounded HTTPS adapter for MusicBrainz `/ws/2`.
pub struct MusicBrainz {
    agent: ureq::Agent,
    base_url: String,
    minimum_interval: Duration,
    previous_request: Mutex<Option<Instant>>,
}

impl MusicBrainz {
    /// Creates a production adapter with official endpoint and one-call-per-second pacing.
    pub fn new(user_agent: &str, timeout: Duration) -> Result<Self, MusicBrainzError> {
        Self::with_endpoint(
            "https://musicbrainz.org/ws/2",
            user_agent,
            timeout,
            Duration::from_secs(1),
            true,
        )
    }

    /// Creates an explicitly configured endpoint, primarily for deterministic tests.
    pub fn with_endpoint(
        base_url: &str,
        user_agent: &str,
        timeout: Duration,
        minimum_interval: Duration,
        https_only: bool,
    ) -> Result<Self, MusicBrainzError> {
        if user_agent.trim().is_empty() {
            return Err(MusicBrainzError::MissingUserAgent);
        }
        let base_url = base_url.trim_end_matches('/');
        if base_url.is_empty() {
            return Err(MusicBrainzError::InvalidEndpoint);
        }
        let config = ureq::Agent::config_builder()
            .timeout_global(Some(timeout))
            .https_only(https_only)
            .user_agent(user_agent)
            .build();
        Ok(Self {
            agent: config.into(),
            base_url: base_url.into(),
            minimum_interval,
            previous_request: Mutex::new(None),
        })
    }

    fn paced_get(&self, url: &str) -> Result<Vec<u8>, MusicBrainzError> {
        for attempt in 0_u32..=3 {
            let mut prior = self
                .previous_request
                .lock()
                .map_err(|_| MusicBrainzError::PacingState)?;
            if let Some(previous) = *prior {
                std::thread::sleep(self.minimum_interval.saturating_sub(previous.elapsed()));
            }
            *prior = Some(Instant::now());
            drop(prior);
            match self.agent.get(url).call() {
                Ok(mut response) => {
                    return response
                        .body_mut()
                        .with_config()
                        .limit(MAX_RESPONSE_BYTES)
                        .read_to_vec()
                        .map_err(MusicBrainzError::Http);
                }
                Err(error)
                    if attempt < 3
                        && matches!(error, ureq::Error::StatusCode(429 | 502 | 503 | 504)) =>
                {
                    std::thread::sleep(Duration::from_secs(2_u64.pow(attempt + 1)));
                }
                Err(error) => return Err(MusicBrainzError::Http(error)),
            }
        }
        unreachable!("bounded MusicBrainz retry loop always returns")
    }
}

impl CanonicalMetadataProvider for MusicBrainz {
    fn resolve(&self, lookup: &CanonicalLookup) -> Result<CanonicalLookupResult, MusicBrainzError> {
        let (path, is_isrc) = match lookup {
            CanonicalLookup::RecordingMbid(id) if is_mbid(id) => (format!("recording/{id}"), false),
            CanonicalLookup::Isrc(isrc) if is_normalized_isrc(isrc) => {
                (format!("isrc/{isrc}"), true)
            }
            _ => return Err(MusicBrainzError::InvalidIdentifier),
        };
        let url = format!(
            "{}/{path}?inc=artist-credits+isrcs+releases+release-groups+url-rels&fmt=json",
            self.base_url
        );
        let bytes = self.paced_get(&url)?;
        let raw_response_json = std::str::from_utf8(&bytes)
            .map_err(MusicBrainzError::Utf8)?
            .to_owned();
        let recordings = if is_isrc {
            let response: IsrcResponse =
                serde_json::from_slice(&bytes).map_err(MusicBrainzError::Json)?;
            response.recordings
        } else {
            vec![serde_json::from_slice(&bytes).map_err(MusicBrainzError::Json)?]
        };
        let recordings = recordings
            .into_iter()
            .map(CanonicalRecording::try_from)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(CanonicalLookupResult {
            recordings,
            raw_response_json,
        })
    }
}

impl GenreMetadataProvider for MusicBrainz {
    fn genres(
        &self,
        recording_mbid: Option<&str>,
        title: &str,
        artist: &str,
    ) -> Result<GenreLookupResult, MusicBrainzError> {
        let exact = recording_mbid.is_some();
        let url = if let Some(id) = recording_mbid {
            if !is_mbid(id) {
                return Err(MusicBrainzError::InvalidIdentifier);
            }
            format!(
                "{}/recording/{id}?inc=artist-credits+genres&fmt=json",
                self.base_url
            )
        } else {
            let query = format!("recording:\"{}\" AND artist:\"{}\"", title, artist);
            format!(
                "{}/recording?query={}&limit=10&fmt=json",
                self.base_url,
                percent_encode(&query)
            )
        };
        let bytes = self.paced_get(&url)?;
        let raw_response_json = std::str::from_utf8(&bytes)
            .map_err(MusicBrainzError::Utf8)?
            .to_owned();
        let values = if exact {
            vec![
                serde_json::from_slice::<GenreRecordingResponse>(&bytes)
                    .map_err(MusicBrainzError::Json)?,
            ]
        } else {
            serde_json::from_slice::<GenreSearchResponse>(&bytes)
                .map_err(MusicBrainzError::Json)?
                .recordings
        };
        let candidates = values
            .into_iter()
            .filter(|value| is_mbid(&value.id) && !value.title.trim().is_empty())
            .map(|value| GenreRecordingCandidate {
                id: value.id,
                title: value.title,
                artist_credit: value
                    .artist_credit
                    .into_iter()
                    .map(|credit| format!("{}{}", credit.name, credit.join_phrase))
                    .collect(),
                length_ms: value.length,
                score: if exact {
                    100
                } else {
                    value.score.unwrap_or_default()
                },
                genres: ranked_genres(value.genres),
            })
            .collect();
        Ok(GenreLookupResult {
            candidates,
            raw_response_json,
        })
    }

    fn artist_genres(&self, artist: &str) -> Result<ArtistGenreResult, MusicBrainzError> {
        let query = format!(
            "artist:\"{}\"",
            artist.strip_suffix(" - Topic").unwrap_or(artist)
        );
        let url = format!(
            "{}/artist?query={}&limit=5&fmt=json",
            self.base_url,
            percent_encode(&query)
        );
        let search_bytes = self.paced_get(&url)?;
        let search_raw = std::str::from_utf8(&search_bytes)
            .map_err(MusicBrainzError::Utf8)?
            .to_owned();
        let search: ArtistSearchResponse =
            serde_json::from_slice(&search_bytes).map_err(MusicBrainzError::Json)?;
        let expected = normalize_artist_name(artist);
        let matching = search
            .artists
            .into_iter()
            .filter(|candidate| {
                candidate.score == Some(100)
                    && normalize_artist_name(&candidate.name) == expected
                    && is_mbid(&candidate.id)
            })
            .collect::<Vec<_>>();
        if matching.len() != 1 {
            return Ok(ArtistGenreResult {
                artist_id: None,
                genres: Vec::new(),
                raw_response_json: search_raw,
            });
        }
        let artist_id = matching[0].id.clone();
        let entity_url = format!("{}/artist/{artist_id}?inc=genres&fmt=json", self.base_url);
        let entity_bytes = self.paced_get(&entity_url)?;
        let entity_raw = std::str::from_utf8(&entity_bytes)
            .map_err(MusicBrainzError::Utf8)?
            .to_owned();
        let entity: ArtistGenreResponse =
            serde_json::from_slice(&entity_bytes).map_err(MusicBrainzError::Json)?;
        Ok(ArtistGenreResult {
            artist_id: Some(artist_id),
            genres: ranked_genres(entity.genres),
            raw_response_json: serde_json::json!({"search":serde_json::from_str::<serde_json::Value>(&search_raw).map_err(MusicBrainzError::Json)?,"artist":serde_json::from_str::<serde_json::Value>(&entity_raw).map_err(MusicBrainzError::Json)?}).to_string(),
        })
    }
}

fn normalize_artist_name(value: &str) -> String {
    value
        .strip_suffix(" - Topic")
        .unwrap_or(value)
        .chars()
        .flat_map(char::to_lowercase)
        .filter(|character| character.is_alphanumeric())
        .collect()
}

fn percent_encode(value: &str) -> String {
    value
        .bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
                (byte as char).to_string()
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect()
}

fn ranked_genres(genres: Vec<TagResponse>) -> Vec<String> {
    let mut values = genres
        .into_iter()
        .filter(|tag| tag.count.unwrap_or(1) > 0 && !tag.name.trim().is_empty())
        .collect::<Vec<_>>();
    values.sort_by_key(|tag| {
        (
            std::cmp::Reverse(tag.count.unwrap_or_default()),
            tag.name.to_lowercase(),
        )
    });
    values.dedup_by(|left, right| left.name.eq_ignore_ascii_case(&right.name));
    values.into_iter().take(8).map(|tag| tag.name).collect()
}

impl TryFrom<RecordingResponse> for CanonicalRecording {
    type Error = MusicBrainzError;

    fn try_from(value: RecordingResponse) -> Result<Self, Self::Error> {
        if !is_mbid(&value.id) || value.title.trim().is_empty() {
            return Err(MusicBrainzError::InvalidResponseIdentity);
        }
        let artist_credit = value
            .artist_credit
            .into_iter()
            .map(|credit| {
                if !is_mbid(&credit.artist.id)
                    || credit.artist.name.trim().is_empty()
                    || credit.name.trim().is_empty()
                {
                    return Err(MusicBrainzError::InvalidResponseIdentity);
                }
                Ok(CanonicalArtistCredit {
                    artist_id: credit.artist.id,
                    artist_name: credit.artist.name,
                    sort_name: credit.artist.sort_name,
                    disambiguation: credit.artist.disambiguation,
                    credited_name: credit.name,
                    join_phrase: credit.join_phrase,
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let releases = value
            .releases
            .into_iter()
            .filter(|release| is_mbid(&release.id) && !release.title.trim().is_empty())
            .map(|release| CanonicalRelease {
                id: release.id,
                title: release.title,
                date: release.date,
                country: release.country,
                status: release.status,
                release_group_id: release
                    .release_group
                    .as_ref()
                    .filter(|group| is_mbid(&group.id))
                    .map(|group| group.id.clone()),
                primary_type: release
                    .release_group
                    .as_ref()
                    .and_then(|group| group.primary_type.clone()),
                secondary_types: release
                    .release_group
                    .map(|group| group.secondary_types)
                    .unwrap_or_default(),
            })
            .collect();
        Ok(Self {
            id: value.id,
            title: value.title,
            length_ms: value.length,
            isrcs: value
                .isrcs
                .into_iter()
                .filter(|isrc| is_normalized_isrc(isrc))
                .collect(),
            artist_credit,
            releases,
            url_relations: value
                .relations
                .into_iter()
                .filter(|relation| relation.target_type == "url")
                .filter_map(|relation| {
                    let resource = relation.url?.resource;
                    (!relation.relation_type.trim().is_empty() && !resource.trim().is_empty())
                        .then_some(CanonicalUrlRelation {
                            relation_type: relation.relation_type,
                            resource,
                        })
                })
                .collect(),
        })
    }
}

fn is_mbid(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(index, byte)| match index {
            8 | 13 | 18 | 23 => byte == b'-',
            _ => byte.is_ascii_hexdigit(),
        })
}

fn is_normalized_isrc(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 12
        && bytes[..2].iter().all(u8::is_ascii_uppercase)
        && bytes[2..5].iter().all(u8::is_ascii_alphanumeric)
        && bytes[5..].iter().all(u8::is_ascii_digit)
}

#[derive(Deserialize)]
struct IsrcResponse {
    #[serde(default)]
    recordings: Vec<RecordingResponse>,
}

#[derive(Deserialize)]
struct RecordingResponse {
    id: String,
    title: String,
    length: Option<u64>,
    #[serde(default)]
    isrcs: Vec<String>,
    #[serde(default, rename = "artist-credit")]
    artist_credit: Vec<ArtistCreditResponse>,
    #[serde(default)]
    releases: Vec<ReleaseResponse>,
    #[serde(default)]
    relations: Vec<UrlRelationResponse>,
}

#[derive(Deserialize)]
struct GenreSearchResponse {
    #[serde(default)]
    recordings: Vec<GenreRecordingResponse>,
}

#[derive(Deserialize)]
struct ArtistSearchResponse {
    #[serde(default)]
    artists: Vec<ArtistSearchCandidate>,
}

#[derive(Deserialize)]
struct ArtistSearchCandidate {
    id: String,
    name: String,
    score: Option<u16>,
}

#[derive(Deserialize)]
struct ArtistGenreResponse {
    #[serde(default)]
    genres: Vec<TagResponse>,
}

#[derive(Deserialize)]
struct GenreRecordingResponse {
    id: String,
    title: String,
    length: Option<u64>,
    score: Option<u16>,
    #[serde(default, rename = "artist-credit")]
    artist_credit: Vec<ArtistCreditResponse>,
    #[serde(default)]
    genres: Vec<TagResponse>,
}

#[derive(Deserialize)]
struct TagResponse {
    name: String,
    count: Option<i64>,
}

#[derive(Deserialize)]
struct UrlRelationResponse {
    #[serde(rename = "type")]
    relation_type: String,
    #[serde(rename = "target-type")]
    target_type: String,
    url: Option<UrlTargetResponse>,
}

#[derive(Deserialize)]
struct UrlTargetResponse {
    resource: String,
}

#[derive(Deserialize)]
struct ArtistCreditResponse {
    name: String,
    #[serde(default, rename = "joinphrase")]
    join_phrase: String,
    artist: ArtistResponse,
}

#[derive(Deserialize)]
struct ArtistResponse {
    id: String,
    name: String,
    #[serde(rename = "sort-name")]
    sort_name: Option<String>,
    disambiguation: Option<String>,
}

#[derive(Deserialize)]
struct ReleaseResponse {
    id: String,
    title: String,
    date: Option<String>,
    country: Option<String>,
    status: Option<String>,
    #[serde(rename = "release-group")]
    release_group: Option<ReleaseGroupResponse>,
}

#[derive(Deserialize)]
struct ReleaseGroupResponse {
    id: String,
    #[serde(rename = "primary-type")]
    primary_type: Option<String>,
    #[serde(default, rename = "secondary-types")]
    secondary_types: Vec<String>,
}

/// Bounded MusicBrainz adapter or response failure.
#[derive(Debug, Error)]
pub enum MusicBrainzError {
    /// API policy requires a meaningful application identifier.
    #[error("MusicBrainz User-Agent must not be empty")]
    MissingUserAgent,
    /// Endpoint must not be empty.
    #[error("MusicBrainz endpoint is invalid")]
    InvalidEndpoint,
    /// Only structurally valid strong identifiers may be queried.
    #[error("canonical lookup identifier is invalid")]
    InvalidIdentifier,
    /// Shared pacing state was poisoned.
    #[error("MusicBrainz request pacing state is unavailable")]
    PacingState,
    /// HTTP, status, timeout, TLS, or response-body failure.
    #[error("MusicBrainz request failed: {0}")]
    Http(ureq::Error),
    /// Response bytes were not UTF-8 JSON.
    #[error("MusicBrainz response was not UTF-8: {0}")]
    Utf8(std::str::Utf8Error),
    /// Response JSON did not match the bounded schema.
    #[error("MusicBrainz returned malformed JSON: {0}")]
    Json(serde_json::Error),
    /// Response contained malformed canonical entity identity.
    #[error("MusicBrainz response contained invalid canonical identity")]
    InvalidResponseIdentity,
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpListener;

    use super::*;

    #[test]
    fn retries_temporary_gateway_failures_with_a_bound() -> Result<(), Box<dyn std::error::Error>> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let endpoint = format!("http://{}", listener.local_addr()?);
        let server = std::thread::spawn(move || -> std::io::Result<()> {
            for status in [503, 200] {
                let (mut stream, _) = listener.accept()?;
                let mut request = [0_u8; 2048];
                let _ = stream.read(&mut request)?;
                let body = if status == 200 { "{}" } else { "temporary" };
                write!(
                    stream,
                    "HTTP/1.1 {status} Fixture\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )?;
            }
            Ok(())
        });
        let provider = MusicBrainz::with_endpoint(
            &endpoint,
            "fixture@example.invalid",
            Duration::from_secs(10),
            Duration::ZERO,
            false,
        )?;
        assert_eq!(provider.paced_get(&format!("{endpoint}/fixture"))?, b"{}");
        server
            .join()
            .map_err(|_| std::io::Error::other("fixture server panicked"))??;
        Ok(())
    }

    #[test]
    fn artist_genres_require_one_exact_perfect_score_name() -> Result<(), Box<dyn std::error::Error>>
    {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let endpoint = format!("http://{}", listener.local_addr()?);
        let server = std::thread::spawn(move || -> std::io::Result<()> {
            let bodies = [
                r#"{"artists":[{"id":"11111111-1111-1111-1111-111111111111","name":"Fixture Artist","score":100}]}"#,
                r#"{"genres":[{"name":"Metalcore","count":8},{"name":"Rock","count":3}]}"#,
            ];
            for body in bodies {
                let (mut stream, _) = listener.accept()?;
                let mut request = [0_u8; 2048];
                let _ = stream.read(&mut request)?;
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )?;
            }
            Ok(())
        });
        let provider = MusicBrainz::with_endpoint(
            &endpoint,
            "fixture@example.invalid",
            Duration::from_secs(10),
            Duration::ZERO,
            false,
        )?;
        let result = provider.artist_genres("Fixture Artist - Topic")?;
        assert_eq!(
            result.artist_id.as_deref(),
            Some("11111111-1111-1111-1111-111111111111")
        );
        assert_eq!(result.genres, ["Metalcore", "Rock"]);
        server
            .join()
            .map_err(|_| std::io::Error::other("fixture server panicked"))??;
        Ok(())
    }

    #[test]
    fn parses_recording_artist_credit_release_and_isrc() -> Result<(), MusicBrainzError> {
        let raw: RecordingResponse =
            serde_json::from_str(include_str!("../tests/fixtures/musicbrainz/recording.json"))
                .map_err(MusicBrainzError::Json)?;
        let recording = CanonicalRecording::try_from(raw)?;

        assert_eq!(recording.title, "Fixture Track");
        assert_eq!(recording.length_ms, Some(180_000));
        assert_eq!(recording.isrcs, ["USABC2412345"]);
        assert_eq!(recording.artist_credit.len(), 2);
        assert_eq!(recording.artist_credit[0].join_phrase, " feat. ");
        assert_eq!(recording.releases.len(), 1);
        assert_eq!(recording.url_relations.len(), 1);
        assert_eq!(recording.url_relations[0].relation_type, "video");
        assert_eq!(recording.releases[0].primary_type.as_deref(), Some("Album"));
        Ok(())
    }

    #[test]
    fn parses_multiple_isrc_recordings_without_selecting_one() -> Result<(), MusicBrainzError> {
        let raw: IsrcResponse = serde_json::from_str(include_str!(
            "../tests/fixtures/musicbrainz/isrc-ambiguous.json"
        ))
        .map_err(MusicBrainzError::Json)?;
        let recordings = raw
            .recordings
            .into_iter()
            .map(CanonicalRecording::try_from)
            .collect::<Result<Vec<_>, _>>()?;
        assert_eq!(recordings.len(), 2);
        Ok(())
    }
}
