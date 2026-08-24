//! Cross-module behavioral regressions for non-negotiable safety properties.

use music_sync::identity::{
    CanonicalIdentityComparison, FingerprintComparison, MetadataCompatibility, ReplacementDecision,
    ReplacementEvidence, verify_replacement,
};
use music_sync::preservation::{
    ArtifactHealth, ReconciliationAction, ReconciliationState, SourceAvailability, reconcile,
};

#[test]
fn removed_playlist_membership_never_deletes_acquired_audio() {
    let action = reconcile(ReconciliationState {
        membership_active: false,
        artifact: ArtifactHealth::Healthy,
        source: SourceAvailability::Available,
    });
    assert_eq!(action, ReconciliationAction::DeactivateMembership);
}

#[test]
fn remote_disappearance_does_not_replace_healthy_audio() {
    let action = reconcile(ReconciliationState {
        membership_active: true,
        artifact: ArtifactHealth::Healthy,
        source: SourceAvailability::PermanentlyUnavailable,
    });
    assert_eq!(action, ReconciliationAction::PreserveArtifact);
}

#[test]
fn transient_provider_failure_is_deferred_without_replacement() {
    let action = reconcile(ReconciliationState {
        membership_active: true,
        artifact: ArtifactHealth::Missing,
        source: SourceAvailability::TransientFailure,
    });
    assert_eq!(action, ReconciliationAction::Defer);
}

#[test]
fn same_title_wrong_recording_is_rejected_by_audio_identity() {
    // Titles are intentionally absent from ReplacementEvidence: text can generate
    // candidates, but cannot prove that candidate audio is the desired recording.
    let decision = verify_replacement(ReplacementEvidence {
        canonical_identity: CanonicalIdentityComparison::Match,
        metadata: MetadataCompatibility::Match,
        fingerprint: FingerprintComparison::Mismatch,
    });
    assert_eq!(decision, ReplacementDecision::Reject);
}

#[test]
fn repeated_reconciliation_is_deterministic_and_download_free() {
    let state = ReconciliationState {
        membership_active: true,
        artifact: ArtifactHealth::Healthy,
        source: SourceAvailability::Available,
    };
    assert_eq!(reconcile(state), ReconciliationAction::PreserveArtifact);
    assert_eq!(reconcile(state), ReconciliationAction::PreserveArtifact);
}
