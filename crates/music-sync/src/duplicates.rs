//! Read-only, evidence-separated duplicate reporting.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;

use crate::fingerprint::RawFingerprint;
use crate::identity::{FingerprintComparison, compare_raw_fingerprints};
use crate::persistence::{DuplicateArtifactEvidence, ProviderObjectReference};

/// Builds a deterministic report without changing database or media state.
#[must_use]
pub fn analyze_duplicates(evidence: Vec<DuplicateArtifactEvidence>) -> DuplicateReport {
    let mut groups = Vec::new();
    let mut exact = BTreeMap::<String, Vec<usize>>::new();
    let mut provider = BTreeMap::<ProviderObjectReference, Vec<usize>>::new();
    for (index, artifact) in evidence.iter().enumerate() {
        if let Some(sha256) = &artifact.sha256 {
            exact.entry(sha256.clone()).or_default().push(index);
        }
        for object in &artifact.provider_objects {
            provider.entry(object.clone()).or_default().push(index);
        }
    }
    for (sha256, indices) in exact.into_iter().filter(|(_, values)| values.len() > 1) {
        groups.push(DuplicateGroup {
            group_id: 0,
            evidence: DuplicateEvidenceKind::ExactBytes,
            evidence_value: Some(sha256),
            provider_object: None,
            artifacts: artifacts_for_indices(&evidence, &indices),
        });
    }
    for (object, indices) in provider
        .into_iter()
        .filter(|(_, values)| unique_indices(values).len() > 1)
    {
        let indices = unique_indices(&indices);
        groups.push(DuplicateGroup {
            group_id: 0,
            evidence: DuplicateEvidenceKind::SameProviderObject,
            evidence_value: None,
            provider_object: Some(object),
            artifacts: artifacts_for_indices(&evidence, &indices),
        });
    }

    let mut fingerprints = Vec::<(usize, RawFingerprint)>::new();
    let mut insufficient_fingerprint_artifacts = 0_u64;
    for (index, artifact) in evidence.iter().enumerate() {
        let parsed = artifact
            .fingerprint_json
            .as_deref()
            .and_then(|json| serde_json::from_str::<Vec<u32>>(json).ok());
        let duration = artifact
            .fingerprint_duration_ms
            .and_then(|value| u64::try_from(value).ok());
        match (parsed, duration) {
            (Some(values), Some(duration_ms)) if !values.is_empty() => fingerprints.push((
                index,
                RawFingerprint {
                    duration_ms,
                    values,
                },
            )),
            _ => insufficient_fingerprint_artifacts += 1,
        }
    }
    fingerprints.sort_by_key(|(_, fingerprint)| fingerprint.duration_ms);
    let mut union = UnionFind::new(evidence.len());
    let mut matched = BTreeSet::new();
    for left in 0..fingerprints.len() {
        for right in left + 1..fingerprints.len() {
            if fingerprints[right]
                .1
                .duration_ms
                .saturating_sub(fingerprints[left].1.duration_ms)
                > 3_000
            {
                break;
            }
            let left_index = fingerprints[left].0;
            let right_index = fingerprints[right].0;
            if evidence[left_index].sha256.is_some()
                && evidence[left_index].sha256 == evidence[right_index].sha256
            {
                continue;
            }
            if compare_raw_fingerprints(&fingerprints[left].1, &fingerprints[right].1)
                == FingerprintComparison::Match
            {
                union.join(left_index, right_index);
                matched.insert(left_index);
                matched.insert(right_index);
            }
        }
    }
    let mut audio = BTreeMap::<usize, Vec<usize>>::new();
    for index in matched {
        audio.entry(union.root(index)).or_default().push(index);
    }
    for indices in audio.into_values().filter(|values| values.len() > 1) {
        groups.push(DuplicateGroup {
            group_id: 0,
            evidence: DuplicateEvidenceKind::AudioMatchCandidate,
            evidence_value: None,
            provider_object: None,
            artifacts: artifacts_for_indices(&evidence, &indices),
        });
    }
    for (offset, group) in groups.iter_mut().enumerate() {
        group.group_id = u64::try_from(offset + 1).unwrap_or(u64::MAX);
    }
    DuplicateReport {
        healthy_artifacts: evidence.len() as u64,
        fingerprinted_artifacts: fingerprints.len() as u64,
        insufficient_fingerprint_artifacts,
        groups,
    }
}

fn unique_indices(indices: &[usize]) -> Vec<usize> {
    indices
        .iter()
        .copied()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn artifacts_for_indices(
    evidence: &[DuplicateArtifactEvidence],
    indices: &[usize],
) -> Vec<DuplicateArtifact> {
    let mut artifacts = indices
        .iter()
        .map(|index| {
            let value = &evidence[*index];
            DuplicateArtifact {
                artifact_id: value.artifact_id,
                recording_id: value.recording_id,
                path: value.path.to_string_lossy().into_owned(),
                sha256: value.sha256.clone(),
                duration_ms: value.duration_ms,
                canonical_title: value.canonical_title.clone(),
                canonical_artist: value.canonical_artist.clone(),
                provider_objects: value.provider_objects.clone(),
            }
        })
        .collect::<Vec<_>>();
    artifacts.sort_by_key(|artifact| artifact.artifact_id);
    artifacts
}

#[derive(Debug)]
struct UnionFind {
    parents: Vec<usize>,
}

impl UnionFind {
    fn new(size: usize) -> Self {
        Self {
            parents: (0..size).collect(),
        }
    }

    fn root(&mut self, mut value: usize) -> usize {
        while self.parents[value] != value {
            self.parents[value] = self.parents[self.parents[value]];
            value = self.parents[value];
        }
        value
    }

    fn join(&mut self, left: usize, right: usize) {
        let left = self.root(left);
        let right = self.root(right);
        if left != right {
            self.parents[right] = left;
        }
    }
}

/// Complete immutable duplicate analysis.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DuplicateReport {
    /// Healthy local artifacts considered.
    pub healthy_artifacts: u64,
    /// Considered artifacts carrying parseable raw fingerprints.
    pub fingerprinted_artifacts: u64,
    /// Healthy artifacts lacking usable raw fingerprint evidence.
    pub insufficient_fingerprint_artifacts: u64,
    /// Deterministic evidence-separated groups.
    pub groups: Vec<DuplicateGroup>,
}

/// One evidence-homogeneous duplicate group.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DuplicateGroup {
    /// Stable ordinal within this generated report.
    pub group_id: u64,
    /// Evidence responsible for forming this group.
    pub evidence: DuplicateEvidenceKind,
    /// Exact digest for an exact-byte group.
    pub evidence_value: Option<String>,
    /// Shared provider object for a provider-identity group.
    pub provider_object: Option<ProviderObjectReference>,
    /// Physical artifacts implicated by this evidence.
    pub artifacts: Vec<DuplicateArtifact>,
}

/// Evidence class; classes are never silently collapsed into identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DuplicateEvidenceKind {
    /// Exact SHA-256 byte equality.
    ExactBytes,
    /// Several healthy artifacts associated with one provider object.
    SameProviderObject,
    /// Strict duration-aware raw Chromaprint match requiring review.
    AudioMatchCandidate,
}

/// One physical artifact shown in a duplicate group.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DuplicateArtifact {
    /// Durable physical artifact ID.
    pub artifact_id: i64,
    /// Current durable recording association.
    pub recording_id: i64,
    /// Registered local media path.
    pub path: String,
    /// Exact byte digest when known.
    pub sha256: Option<String>,
    /// Structurally observed duration when known.
    pub duration_ms: Option<i64>,
    /// Independently selected canonical title when available.
    pub canonical_title: Option<String>,
    /// Independently selected canonical artist credit when available.
    pub canonical_artist: Option<String>,
    /// Provider objects associated through the current recording.
    pub provider_objects: Vec<ProviderObjectReference>,
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn evidence(id: i64, sha256: &str, values: Vec<u32>) -> DuplicateArtifactEvidence {
        let fingerprint_json = format!(
            "[{}]",
            values
                .iter()
                .map(u32::to_string)
                .collect::<Vec<_>>()
                .join(",")
        );
        DuplicateArtifactEvidence {
            artifact_id: id,
            recording_id: id,
            path: PathBuf::from(format!("/music/{id}.opus")),
            sha256: Some(sha256.to_owned()),
            duration_ms: Some(120_000),
            fingerprint_duration_ms: Some(120_000),
            fingerprint_json: Some(fingerprint_json),
            canonical_title: None,
            canonical_artist: None,
            provider_objects: Vec::new(),
        }
    }

    #[test]
    fn separates_exact_bytes_from_strict_audio_candidates() {
        let values = (0..130).map(|value| value as u32).collect::<Vec<_>>();
        let report = analyze_duplicates(vec![
            evidence(1, "same", values.clone()),
            evidence(2, "same", values.clone()),
            evidence(3, "different", values),
        ]);
        assert_eq!(report.groups.len(), 2);
        assert_eq!(report.groups[0].evidence, DuplicateEvidenceKind::ExactBytes);
        assert_eq!(
            report.groups[1].evidence,
            DuplicateEvidenceKind::AudioMatchCandidate
        );
        assert_eq!(report.groups[1].artifacts.len(), 3);
    }
}
