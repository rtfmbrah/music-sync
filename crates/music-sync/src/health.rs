//! Conservative bounded reconciliation of registered artifact health.

use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;
use thiserror::Error;

use crate::content_hash::ContentHasher;
use crate::media_probe::MediaProbe;
use crate::persistence::{
    ArtifactHealth, ArtifactHealthCandidate, ArtifactHealthObservation, Database, DatabaseError,
};

/// Checks a stable bounded artifact snapshot without modifying media files.
pub fn reconcile_artifact_health(
    database: &mut Database,
    library_directory: &Path,
    probe: &dyn MediaProbe,
    hasher: &dyn ContentHasher,
    maximum_artifacts: usize,
) -> Result<ArtifactHealthReport, ArtifactHealthError> {
    if maximum_artifacts == 0 {
        return Err(ArtifactHealthError::InvalidLimit);
    }
    let library =
        library_directory
            .canonicalize()
            .map_err(|source| ArtifactHealthError::Library {
                path: library_directory.to_path_buf(),
                source,
            })?;
    let candidates = database.artifact_health_candidates(maximum_artifacts)?;
    let mut report = ArtifactHealthReport {
        selected: candidates.len() as u64,
        ..ArtifactHealthReport::default()
    };
    for candidate in candidates {
        match check_one(&candidate, &library, probe, hasher) {
            Ok(observation) => {
                let changed = database.record_artifact_health(&observation)?;
                report.checked += 1;
                if changed {
                    report.changed += 1;
                }
                match observation.health {
                    ArtifactHealth::Unknown => {}
                    ArtifactHealth::Healthy => report.healthy += 1,
                    ArtifactHealth::Missing => report.missing += 1,
                    ArtifactHealth::Corrupt => report.corrupt += 1,
                }
            }
            Err(message) => report.failures.push(ArtifactHealthFailure {
                artifact_id: candidate.id,
                path: candidate.path,
                message,
            }),
        }
    }
    Ok(report)
}

fn check_one(
    candidate: &ArtifactHealthCandidate,
    library: &Path,
    probe: &dyn MediaProbe,
    hasher: &dyn ContentHasher,
) -> Result<ArtifactHealthObservation, String> {
    if !candidate.path.is_absolute() || !candidate.path.starts_with(library) {
        return Err("registered artifact is outside the configured library".into());
    }
    let metadata = match fs::symlink_metadata(&candidate.path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(observation(candidate.id, ArtifactHealth::Missing));
        }
        Err(error) => return Err(format!("failed to inspect registered path: {error}")),
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Ok(observation(candidate.id, ArtifactHealth::Corrupt));
    }
    let canonical = candidate
        .path
        .canonicalize()
        .map_err(|error| format!("failed to resolve registered path: {error}"))?;
    if !canonical.starts_with(library) {
        return Err("registered artifact resolves outside the configured library".into());
    }
    let hash = hasher
        .hash(&canonical)
        .map_err(|error| format!("artifact hash check failed: {error}"))?;
    let sha256 = hex_sha256(hash.sha256);
    if candidate
        .sha256
        .as_deref()
        .is_some_and(|expected| expected != sha256)
    {
        return Ok(observation(candidate.id, ArtifactHealth::Corrupt));
    }
    let properties = probe
        .probe(&canonical)
        .map_err(|error| format!("artifact probe check failed: {error}"))?;
    Ok(ArtifactHealthObservation {
        artifact_id: candidate.id,
        health: ArtifactHealth::Healthy,
        sha256: Some(sha256),
        duration_ms: properties.duration_ms.map(checked_i64).transpose()?,
        codec: Some(properties.codec),
        sample_rate_hz: properties.sample_rate_hz.map(i64::from),
        channels: properties.channels.map(i64::from),
    })
}

fn observation(artifact_id: i64, health: ArtifactHealth) -> ArtifactHealthObservation {
    ArtifactHealthObservation {
        artifact_id,
        health,
        sha256: None,
        duration_ms: None,
        codec: None,
        sample_rate_hz: None,
        channels: None,
    }
}

fn checked_i64(value: u64) -> Result<i64, String> {
    i64::try_from(value).map_err(|_| "observed media value exceeds SQLite range".into())
}

fn hex_sha256(bytes: [u8; 32]) -> String {
    use std::fmt::Write;

    bytes
        .iter()
        .fold(String::with_capacity(64), |mut output, byte| {
            let _ = write!(output, "{byte:02x}");
            output
        })
}

/// Aggregate bounded artifact-health effects.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ArtifactHealthReport {
    /// Registered artifacts snapshotted for this run.
    pub selected: u64,
    /// Artifacts with a persisted conservative observation.
    pub checked: u64,
    /// Artifacts whose constrained health state changed.
    pub changed: u64,
    /// Checked artifacts found healthy.
    pub healthy: u64,
    /// Checked artifacts found absent.
    pub missing: u64,
    /// Checked artifacts with contradictory integrity evidence.
    pub corrupt: u64,
    /// Isolated checks that produced insufficient evidence.
    pub failures: Vec<ArtifactHealthFailure>,
}

/// One isolated artifact check failure that left prior health unchanged.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ArtifactHealthFailure {
    /// Durable artifact ID.
    pub artifact_id: i64,
    /// Registered path.
    pub path: PathBuf,
    /// Conservative diagnostic.
    pub message: String,
}

/// Fatal error preventing bounded artifact reconciliation.
#[derive(Debug, Error)]
pub enum ArtifactHealthError {
    /// Artifact selection must always be bounded by a non-zero limit.
    #[error("maximum artifacts must be greater than zero")]
    InvalidLimit,
    /// Configured library could not be resolved.
    #[error("failed to access library {path}: {source}")]
    Library {
        /// Configured library path.
        path: PathBuf,
        /// Underlying filesystem error.
        source: std::io::Error,
    },
    /// Durable state selection or persistence failed.
    #[error(transparent)]
    Database(#[from] DatabaseError),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adoption::apply_library;
    use crate::content_hash::Sha256FileHasher;
    use crate::media_probe::{
        CanonicalTagStatus, MediaProbeError, MediaProperties, MediaTagPresence,
    };

    struct FixtureProbe;

    impl MediaProbe for FixtureProbe {
        fn probe(&self, _path: &Path) -> Result<MediaProperties, MediaProbeError> {
            Ok(MediaProperties {
                codec: "opus".into(),
                duration_ms: Some(1_500),
                sample_rate_hz: Some(48_000),
                channels: Some(2),
                has_embedded_artwork: false,
                tags: MediaTagPresence {
                    has_basic_tags: false,
                    musicbrainz_recording_id: CanonicalTagStatus::Absent,
                    isrc: CanonicalTagStatus::Absent,
                },
            })
        }
    }

    struct FailingProbe;

    impl MediaProbe for FailingProbe {
        fn probe(&self, _path: &Path) -> Result<MediaProperties, MediaProbeError> {
            Err(MediaProbeError::MissingCodec)
        }
    }

    #[test]
    fn reconciles_healthy_missing_and_hash_mismatch_without_touching_media()
    -> Result<(), Box<dyn std::error::Error>> {
        let library = tempfile::tempdir()?;
        let good = library.path().join("good.opus");
        let gone = library.path().join("gone.opus");
        let changed = library.path().join("changed.opus");
        fs::write(&good, b"good")?;
        fs::write(&gone, b"gone")?;
        fs::write(&changed, b"original")?;
        let mut database = Database::open_in_memory()?;
        apply_library(library.path(), &mut database)?;
        fs::remove_file(&gone)?;

        let first = reconcile_artifact_health(
            &mut database,
            library.path(),
            &FixtureProbe,
            &Sha256FileHasher,
            10,
        )?;
        assert_eq!(first.checked, 3);
        assert_eq!(first.healthy, 2);
        assert_eq!(first.missing, 1);
        assert_eq!(fs::read(&good)?, b"good");
        fs::write(&changed, b"different")?;

        let second = reconcile_artifact_health(
            &mut database,
            library.path(),
            &FixtureProbe,
            &Sha256FileHasher,
            10,
        )?;
        assert_eq!(second.healthy, 1);
        assert_eq!(second.missing, 1);
        assert_eq!(second.corrupt, 1);
        assert_eq!(fs::read(&changed)?, b"different");
        Ok(())
    }

    #[test]
    fn probe_failure_preserves_prior_healthy_state() -> Result<(), Box<dyn std::error::Error>> {
        let library = tempfile::tempdir()?;
        fs::write(library.path().join("track.opus"), b"audio")?;
        let mut database = Database::open_in_memory()?;
        apply_library(library.path(), &mut database)?;
        reconcile_artifact_health(
            &mut database,
            library.path(),
            &FixtureProbe,
            &Sha256FileHasher,
            1,
        )?;

        let report = reconcile_artifact_health(
            &mut database,
            library.path(),
            &FailingProbe,
            &Sha256FileHasher,
            1,
        )?;
        assert_eq!(report.checked, 0);
        assert_eq!(report.failures.len(), 1);
        assert_eq!(
            database.artifact_health_candidates(1)?[0].health,
            ArtifactHealth::Healthy
        );
        Ok(())
    }
}
