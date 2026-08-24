//! Conservative policy for accepting replacement audio candidates.

use crate::fingerprint::RawFingerprint;

const MINIMUM_FINGERPRINT_OVERLAP: usize = 120;
const MAXIMUM_ALIGNMENT_OFFSET: isize = 120;
const MAXIMUM_AVERAGE_BIT_ERROR: f64 = 2.0;
const MAXIMUM_DURATION_DIFFERENCE_MS: u64 = 3_000;

/// Result produced by a future perceptual fingerprint comparator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FingerprintComparison {
    /// Fingerprint evidence is unavailable or insufficient.
    Unavailable,
    /// Audio identity is sufficiently similar under a documented threshold.
    Match,
    /// Audio identity differs.
    Mismatch,
}

/// Conservatively compares near-identical raw Chromaprint algorithm-2 evidence.
///
/// The policy follows Chromaprint's own matcher constants for 120-value query
/// overlap, 120-value alignment search, and at most two average differing bits. It
/// additionally rejects duration differences above three seconds. This deliberately
/// identifies near-identical recordings, not covers or merely similar music.
#[must_use]
pub fn compare_raw_fingerprints(
    reference: &RawFingerprint,
    candidate: &RawFingerprint,
) -> FingerprintComparison {
    if reference.values.len() < MINIMUM_FINGERPRINT_OVERLAP
        || candidate.values.len() < MINIMUM_FINGERPRINT_OVERLAP
        || reference.duration_ms.abs_diff(candidate.duration_ms) > MAXIMUM_DURATION_DIFFERENCE_MS
    {
        return FingerprintComparison::Unavailable;
    }
    let mut best = f64::INFINITY;
    for offset in -MAXIMUM_ALIGNMENT_OFFSET..=MAXIMUM_ALIGNMENT_OFFSET {
        let (reference_start, candidate_start) = if offset >= 0 {
            (offset as usize, 0)
        } else {
            (0, (-offset) as usize)
        };
        if reference_start >= reference.values.len() || candidate_start >= candidate.values.len() {
            continue;
        }
        let overlap = (reference.values.len() - reference_start)
            .min(candidate.values.len() - candidate_start);
        if overlap < MINIMUM_FINGERPRINT_OVERLAP {
            continue;
        }
        let errors = reference.values[reference_start..reference_start + overlap]
            .iter()
            .zip(&candidate.values[candidate_start..candidate_start + overlap])
            .map(|(left, right)| (left ^ right).count_ones() as u64)
            .sum::<u64>();
        best = best.min(errors as f64 / overlap as f64);
    }
    if !best.is_finite() {
        FingerprintComparison::Unavailable
    } else if best <= MAXIMUM_AVERAGE_BIT_ERROR {
        FingerprintComparison::Match
    } else {
        FingerprintComparison::Mismatch
    }
}

/// Independently derived evidence about a replacement candidate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReplacementEvidence {
    /// Whether a canonical identifier such as recording MBID or ISRC agrees.
    pub canonical_identity_matches: bool,
    /// Whether duration and meaningful version qualifiers are compatible.
    pub metadata_compatible: bool,
    /// Result of comparing reference and candidate audio fingerprints.
    pub fingerprint: FingerprintComparison,
}

/// Conservative result of replacement identity verification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplacementDecision {
    /// Candidate audio may be committed atomically.
    Accept,
    /// Candidate is known to be the wrong recording or version.
    Reject,
    /// Evidence is not strong enough; leave the recording unresolved.
    Unresolved,
}

/// Verifies candidate evidence; text similarity is deliberately not an input.
#[must_use]
pub fn verify_replacement(evidence: ReplacementEvidence) -> ReplacementDecision {
    if !evidence.canonical_identity_matches || !evidence.metadata_compatible {
        return ReplacementDecision::Reject;
    }
    match evidence.fingerprint {
        FingerprintComparison::Match => ReplacementDecision::Accept,
        FingerprintComparison::Mismatch => ReplacementDecision::Reject,
        FingerprintComparison::Unavailable => ReplacementDecision::Unresolved,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matching_metadata_without_audio_evidence_is_unresolved() {
        let decision = verify_replacement(ReplacementEvidence {
            canonical_identity_matches: true,
            metadata_compatible: true,
            fingerprint: FingerprintComparison::Unavailable,
        });
        assert_eq!(decision, ReplacementDecision::Unresolved);
    }

    #[test]
    fn raw_comparison_is_aligned_strict_and_duration_bounded() {
        let values = (0..240).map(|value| value as u32 * 17).collect::<Vec<_>>();
        let reference = RawFingerprint {
            duration_ms: 120_000,
            values: values.clone(),
        };
        let mut shifted = vec![u32::MAX; 3];
        shifted.extend(values.clone());
        assert_eq!(
            compare_raw_fingerprints(
                &reference,
                &RawFingerprint {
                    duration_ms: 121_000,
                    values: shifted,
                }
            ),
            FingerprintComparison::Match
        );
        assert_eq!(
            compare_raw_fingerprints(
                &reference,
                &RawFingerprint {
                    duration_ms: 120_000,
                    values: vec![u32::MAX; 240],
                }
            ),
            FingerprintComparison::Mismatch
        );
        assert_eq!(
            compare_raw_fingerprints(
                &reference,
                &RawFingerprint {
                    duration_ms: 124_000,
                    values,
                }
            ),
            FingerprintComparison::Unavailable
        );
    }
}
