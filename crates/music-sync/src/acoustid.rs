//! Bounded AcoustID lookup and exact MusicBrainz recording corroboration.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::fingerprint::CompressedFingerprint;

const MAX_RESPONSE_BYTES: u64 = 4 * 1024 * 1024;
const MAX_RESULTS: usize = 100;
const MAX_RECORDINGS_PER_RESULT: usize = 100;

/// External boundary that corroborates a staged fingerprint against one expected
/// MusicBrainz recording identity.
pub trait AcoustIdProvider {
    /// Looks up `fingerprint` and evaluates only exact recording MBIDs.
    fn corroborate(
        &self,
        expected_recording_mbid: &str,
        fingerprint: &CompressedFingerprint,
        minimum_score_millionths: u32,
    ) -> Result<AcoustIdEvidence, AcoustIdError>;
}

/// Result of one bounded AcoustID identity lookup.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AcoustIdEvidence {
    /// Conservative identity decision.
    pub decision: AcoustIdDecision,
    /// Highest score attached to the expected MBID, when returned.
    pub expected_score_millionths: Option<u32>,
    /// Distinct high-confidence MBIDs contradicting the expected recording.
    pub contradictory_recording_mbids: Vec<String>,
    /// Complete bounded response retained for audit persistence.
    pub raw_response_json: String,
}

/// Conservative interpretation of AcoustID evidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AcoustIdDecision {
    /// The expected MBID is present above threshold and no other MBID contradicts it.
    Verified,
    /// A different MBID is present above threshold.
    Contradicted,
    /// Coverage or confidence is insufficient to make an identity decision.
    Insufficient,
}

/// Blocking, paced HTTPS adapter for AcoustID `/v2/lookup`.
pub struct AcoustId {
    agent: ureq::Agent,
    endpoint: String,
    client_key: String,
    minimum_interval: Duration,
    previous_request: Mutex<Option<Instant>>,
}

impl AcoustId {
    /// Creates a production adapter using the official service endpoint.
    pub fn new(client_key: &str, timeout: Duration) -> Result<Self, AcoustIdError> {
        Self::with_endpoint(
            "https://api.acoustid.org/v2/lookup",
            client_key,
            timeout,
            Duration::from_millis(334),
            true,
        )
    }

    /// Creates an explicitly configured adapter, primarily for deterministic tests.
    pub fn with_endpoint(
        endpoint: &str,
        client_key: &str,
        timeout: Duration,
        minimum_interval: Duration,
        https_only: bool,
    ) -> Result<Self, AcoustIdError> {
        let endpoint = endpoint.trim();
        if endpoint.is_empty() {
            return Err(AcoustIdError::InvalidEndpoint);
        }
        let client_key = client_key.trim();
        if client_key.is_empty()
            || client_key.len() > 128
            || !client_key.bytes().all(|byte| byte.is_ascii_alphanumeric())
        {
            return Err(AcoustIdError::InvalidClientKey);
        }
        let config = ureq::Agent::config_builder()
            .timeout_global(Some(timeout))
            .https_only(https_only)
            .build();
        Ok(Self {
            agent: config.into(),
            endpoint: endpoint.into(),
            client_key: client_key.into(),
            minimum_interval,
            previous_request: Mutex::new(None),
        })
    }

    fn lookup(&self, fingerprint: &CompressedFingerprint) -> Result<Vec<u8>, AcoustIdError> {
        let duration = fingerprint.duration_seconds.to_string();
        for attempt in 0_u32..=3 {
            let mut prior = self
                .previous_request
                .lock()
                .map_err(|_| AcoustIdError::PacingState)?;
            if let Some(previous) = *prior {
                std::thread::sleep(self.minimum_interval.saturating_sub(previous.elapsed()));
            }
            *prior = Some(Instant::now());
            drop(prior);

            let form = [
                ("client", self.client_key.as_str()),
                ("duration", duration.as_str()),
                ("fingerprint", fingerprint.fingerprint.as_str()),
                ("meta", "recordingids"),
                ("format", "json"),
            ];
            match self.agent.post(&self.endpoint).send_form(form) {
                Ok(mut response) => {
                    return response
                        .body_mut()
                        .with_config()
                        .limit(MAX_RESPONSE_BYTES)
                        .read_to_vec()
                        .map_err(AcoustIdError::Http);
                }
                Err(error)
                    if attempt < 3
                        && matches!(error, ureq::Error::StatusCode(429 | 502 | 503 | 504)) =>
                {
                    std::thread::sleep(Duration::from_secs(2_u64.pow(attempt + 1)));
                }
                Err(error) => return Err(AcoustIdError::Http(error)),
            }
        }
        unreachable!("bounded AcoustID retry loop always returns")
    }
}

impl AcoustIdProvider for AcoustId {
    fn corroborate(
        &self,
        expected_recording_mbid: &str,
        fingerprint: &CompressedFingerprint,
        minimum_score_millionths: u32,
    ) -> Result<AcoustIdEvidence, AcoustIdError> {
        if !is_mbid(expected_recording_mbid) {
            return Err(AcoustIdError::InvalidRecordingMbid);
        }
        if !(1..=1_000_000).contains(&minimum_score_millionths) {
            return Err(AcoustIdError::InvalidScoreThreshold);
        }
        let bytes = self.lookup(fingerprint)?;
        let raw_response_json = std::str::from_utf8(&bytes)
            .map_err(AcoustIdError::Utf8)?
            .to_owned();
        let response: LookupResponse =
            serde_json::from_slice(&bytes).map_err(AcoustIdError::Json)?;
        if response.status != "ok" {
            return Err(AcoustIdError::ProviderStatus(response.status));
        }
        if response.results.len() > MAX_RESULTS {
            return Err(AcoustIdError::TooManyResults(response.results.len()));
        }

        let mut expected_score = None;
        let mut contradictions = Vec::new();
        for result in response.results {
            if result.recordings.len() > MAX_RECORDINGS_PER_RESULT {
                return Err(AcoustIdError::TooManyRecordings(result.recordings.len()));
            }
            let score = score_millionths(result.score)?;
            if score < minimum_score_millionths {
                continue;
            }
            for recording in result.recordings {
                if !is_mbid(&recording.id) {
                    return Err(AcoustIdError::MalformedRecordingMbid(recording.id));
                }
                if recording.id == expected_recording_mbid {
                    expected_score =
                        Some(expected_score.map_or(score, |prior: u32| prior.max(score)));
                } else if !contradictions.contains(&recording.id) {
                    contradictions.push(recording.id);
                }
            }
        }
        contradictions.sort();
        let decision = if !contradictions.is_empty() {
            AcoustIdDecision::Contradicted
        } else if expected_score.is_some() {
            AcoustIdDecision::Verified
        } else {
            AcoustIdDecision::Insufficient
        };
        Ok(AcoustIdEvidence {
            decision,
            expected_score_millionths: expected_score,
            contradictory_recording_mbids: contradictions,
            raw_response_json,
        })
    }
}

fn score_millionths(score: f64) -> Result<u32, AcoustIdError> {
    if !score.is_finite() || !(0.0..=1.0).contains(&score) {
        return Err(AcoustIdError::InvalidProviderScore);
    }
    Ok((score * 1_000_000.0).round() as u32)
}

fn is_mbid(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(index, byte)| match index {
            8 | 13 | 18 | 23 => byte == b'-',
            _ => byte.is_ascii_hexdigit(),
        })
}

#[derive(Debug, Deserialize)]
struct LookupResponse {
    status: String,
    #[serde(default)]
    results: Vec<LookupResult>,
}

#[derive(Debug, Deserialize)]
struct LookupResult {
    score: f64,
    #[serde(default)]
    recordings: Vec<LookupRecording>,
}

#[derive(Debug, Deserialize)]
struct LookupRecording {
    id: String,
}

/// AcoustID configuration, transport, or response failure.
#[derive(Debug, Error)]
pub enum AcoustIdError {
    /// Endpoint is empty or invalid.
    #[error("AcoustID endpoint must not be empty")]
    InvalidEndpoint,
    /// Application key is absent or malformed.
    #[error("AcoustID client key is malformed")]
    InvalidClientKey,
    /// Expected canonical identifier is malformed.
    #[error("expected MusicBrainz recording ID is malformed")]
    InvalidRecordingMbid,
    /// Confidence threshold is outside `(0, 1]`.
    #[error("AcoustID score threshold must be between 1 and 1000000 millionths")]
    InvalidScoreThreshold,
    /// Internal pacing lock was poisoned.
    #[error("AcoustID pacing state is unavailable")]
    PacingState,
    /// HTTP request or bounded response read failed.
    #[error("AcoustID request failed: {0}")]
    Http(ureq::Error),
    /// Response was not UTF-8 JSON text.
    #[error("AcoustID returned non-UTF-8 JSON: {0}")]
    Utf8(std::str::Utf8Error),
    /// Response was malformed JSON.
    #[error("AcoustID returned malformed JSON: {0}")]
    Json(serde_json::Error),
    /// Provider returned a non-success status document.
    #[error("AcoustID returned provider status {0:?}")]
    ProviderStatus(String),
    /// Response result count exceeded the semantic bound.
    #[error("AcoustID returned {0} results")]
    TooManyResults(usize),
    /// One result contained excessive canonical identities.
    #[error("AcoustID returned {0} recordings in one result")]
    TooManyRecordings(usize),
    /// Provider score was non-finite or outside `[0, 1]`.
    #[error("AcoustID returned an invalid score")]
    InvalidProviderScore,
    /// Provider supplied a malformed MusicBrainz recording ID.
    #[error("AcoustID returned malformed recording ID {0:?}")]
    MalformedRecordingMbid(String),
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

    use super::*;

    const EXPECTED: &str = "11111111-1111-1111-1111-111111111111";
    const OTHER: &str = "22222222-2222-2222-2222-222222222222";

    fn serve(
        body: &str,
    ) -> Result<(String, thread::JoinHandle<std::io::Result<String>>), std::io::Error> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let endpoint = format!("http://{}/v2/lookup", listener.local_addr()?);
        let response_body = body.to_owned();
        let handle = thread::spawn(move || -> std::io::Result<String> {
            let (mut stream, _) = listener.accept()?;
            let mut request = Vec::new();
            let mut buffer = [0_u8; 4096];
            loop {
                let read = stream.read(&mut buffer)?;
                request.extend_from_slice(&buffer[..read]);
                if request.windows(4).any(|window| window == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&request);
                    let content_length = headers
                        .lines()
                        .find_map(|line| {
                            line.to_ascii_lowercase()
                                .strip_prefix("content-length: ")
                                .map(str::to_owned)
                        })
                        .and_then(|value| value.trim().parse::<usize>().ok())
                        .unwrap_or(0);
                    let header_end = request
                        .windows(4)
                        .position(|window| window == b"\r\n\r\n")
                        .ok_or_else(|| {
                            std::io::Error::new(
                                std::io::ErrorKind::InvalidData,
                                "fixture request lacks header terminator",
                            )
                        })?
                        + 4;
                    if request.len() >= header_end + content_length {
                        break;
                    }
                }
            }
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                response_body.len(),
                response_body
            )?;
            String::from_utf8(request)
                .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))
        });
        Ok((endpoint, handle))
    }

    fn join_fixture(
        handle: thread::JoinHandle<std::io::Result<String>>,
    ) -> Result<String, Box<dyn std::error::Error>> {
        handle
            .join()
            .map_err(|_| std::io::Error::other("fixture server panicked"))?
            .map_err(Into::into)
    }

    fn fingerprint() -> CompressedFingerprint {
        CompressedFingerprint {
            duration_seconds: 180,
            fingerprint: "AQAD_fixture".into(),
        }
    }

    #[test]
    fn verifies_exact_non_contradictory_mbid() -> Result<(), Box<dyn std::error::Error>> {
        let (endpoint, handle) = serve(&format!(
            r#"{{"status":"ok","results":[{{"score":0.97,"recordings":[{{"id":"{EXPECTED}"}}]}}]}}"#,
        ))?;
        let provider = AcoustId::with_endpoint(
            &endpoint,
            "fixturekey",
            Duration::from_secs(2),
            Duration::ZERO,
            false,
        )?;
        let evidence = provider.corroborate(EXPECTED, &fingerprint(), 950_000)?;
        assert_eq!(evidence.decision, AcoustIdDecision::Verified);
        assert_eq!(evidence.expected_score_millionths, Some(970_000));
        let request = join_fixture(handle)?;
        assert!(request.starts_with("POST /v2/lookup HTTP/1.1"));
        assert!(request.contains("duration=180"));
        assert!(request.contains("fingerprint=AQAD_fixture"));
        assert!(request.contains("meta=recordingids"));
        Ok(())
    }

    #[test]
    fn contradiction_wins_over_expected_match() -> Result<(), Box<dyn std::error::Error>> {
        let (endpoint, handle) = serve(&format!(
            r#"{{"status":"ok","results":[{{"score":0.99,"recordings":[{{"id":"{EXPECTED}"}},{{"id":"{OTHER}"}}]}}]}}"#,
        ))?;
        let provider = AcoustId::with_endpoint(
            &endpoint,
            "fixturekey",
            Duration::from_secs(2),
            Duration::ZERO,
            false,
        )?;
        let evidence = provider.corroborate(EXPECTED, &fingerprint(), 950_000)?;
        assert_eq!(evidence.decision, AcoustIdDecision::Contradicted);
        assert_eq!(evidence.contradictory_recording_mbids, [OTHER]);
        join_fixture(handle)?;
        Ok(())
    }

    #[test]
    fn low_confidence_or_empty_coverage_is_insufficient() -> Result<(), Box<dyn std::error::Error>>
    {
        let (endpoint, handle) = serve(&format!(
            r#"{{"status":"ok","results":[{{"score":0.7,"recordings":[{{"id":"{EXPECTED}"}}]}}]}}"#,
        ))?;
        let provider = AcoustId::with_endpoint(
            &endpoint,
            "fixturekey",
            Duration::from_secs(2),
            Duration::ZERO,
            false,
        )?;
        let evidence = provider.corroborate(EXPECTED, &fingerprint(), 950_000)?;
        assert_eq!(evidence.decision, AcoustIdDecision::Insufficient);
        join_fixture(handle)?;
        Ok(())
    }
}
