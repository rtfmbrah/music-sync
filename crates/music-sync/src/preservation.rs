//! Pure reconciliation decisions that enforce media preservation.

/// Health of an already acquired local artifact.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtifactHealth {
    /// No artifact has been recorded.
    Absent,
    /// The local artifact exists and passed validation.
    Healthy,
    /// The recorded artifact is missing.
    Missing,
    /// The local artifact failed validation.
    Corrupt,
}

/// Classified availability of the original provider item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceAvailability {
    /// The provider item is currently usable.
    Available,
    /// Availability is unknown because infrastructure or authentication failed.
    TransientFailure,
    /// The provider definitively reports the item as permanently unavailable.
    PermanentlyUnavailable,
}

/// Inputs used to decide the next safe action for one source item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReconciliationState {
    /// Whether this item remains in the remote collection.
    pub membership_active: bool,
    /// Health of any associated local artifact.
    pub artifact: ArtifactHealth,
    /// Classified provider availability.
    pub source: SourceAvailability,
}

/// A safe, explicit reconciliation action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReconciliationAction {
    /// Retain the local artifact and make no acquisition attempt.
    PreserveArtifact,
    /// Update collection membership only; never delete media.
    DeactivateMembership,
    /// Acquire from the known available provider item.
    AcquireOriginal,
    /// Retry later without searching for a replacement.
    Defer,
    /// Candidate generation may begin, but candidates still require identity verification.
    SearchForVerifiedReplacement,
}

/// Computes a deterministic preservation decision without side effects.
#[must_use]
pub fn reconcile(state: ReconciliationState) -> ReconciliationAction {
    if !state.membership_active {
        return ReconciliationAction::DeactivateMembership;
    }
    if state.artifact == ArtifactHealth::Healthy {
        return ReconciliationAction::PreserveArtifact;
    }
    match state.source {
        SourceAvailability::Available => ReconciliationAction::AcquireOriginal,
        SourceAvailability::TransientFailure => ReconciliationAction::Defer,
        SourceAvailability::PermanentlyUnavailable => {
            ReconciliationAction::SearchForVerifiedReplacement
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removed_membership_preserves_artifact_by_only_deactivating_membership() {
        let action = reconcile(ReconciliationState {
            membership_active: false,
            artifact: ArtifactHealth::Healthy,
            source: SourceAvailability::Available,
        });
        assert_eq!(action, ReconciliationAction::DeactivateMembership);
    }

    #[test]
    fn unavailable_remote_with_healthy_artifact_does_nothing() {
        let action = reconcile(ReconciliationState {
            membership_active: true,
            artifact: ArtifactHealth::Healthy,
            source: SourceAvailability::PermanentlyUnavailable,
        });
        assert_eq!(action, ReconciliationAction::PreserveArtifact);
    }

    #[test]
    fn transient_failure_never_searches_for_replacement() {
        let action = reconcile(ReconciliationState {
            membership_active: true,
            artifact: ArtifactHealth::Missing,
            source: SourceAvailability::TransientFailure,
        });
        assert_eq!(action, ReconciliationAction::Defer);
    }
}
