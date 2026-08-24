//! Conservative policy for accepting replacement audio candidates.

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
}
