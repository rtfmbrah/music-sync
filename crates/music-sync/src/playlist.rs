//! Atomic Navidrome-compatible M3U8 materialization.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;
use thiserror::Error;

use crate::content_hash::{ContentHashError, ContentHasher, Sha256FileHasher};
use crate::persistence::{Database, DatabaseError, PlaylistSnapshot};

/// Materializes every durable collection independently.
pub fn materialize_playlists(
    database: &mut Database,
    library_directory: &Path,
    playlist_directory: &Path,
) -> Result<PlaylistMaterializationReport, PlaylistError> {
    fs::create_dir_all(playlist_directory).map_err(|source| PlaylistError::Directory {
        path: playlist_directory.to_path_buf(),
        source,
    })?;
    let library = library_directory
        .canonicalize()
        .map_err(|source| PlaylistError::Directory {
            path: library_directory.to_path_buf(),
            source,
        })?;
    let output = playlist_directory
        .canonicalize()
        .map_err(|source| PlaylistError::Directory {
            path: playlist_directory.to_path_buf(),
            source,
        })?;
    let snapshots = database.playlist_snapshots()?;
    let mut report = PlaylistMaterializationReport::default();
    for snapshot in snapshots {
        match materialize_one(database, &snapshot, &library, &output) {
            Ok(result) => report.playlists.push(result),
            Err(error) if error.is_database_failure() => return Err(error),
            Err(error) => report.failures.push(PlaylistFailure {
                collection_id: snapshot.collection_id,
                message: error.to_string(),
            }),
        }
    }
    Ok(report)
}

fn materialize_one(
    database: &mut Database,
    snapshot: &PlaylistSnapshot,
    library: &Path,
    output: &Path,
) -> Result<PlaylistMaterialized, PlaylistError> {
    let mut contents = String::from("#EXTM3U\n");
    let mut omitted = snapshot.unresolved;
    for artifact in &snapshot.entries {
        let artifact = match artifact.canonicalize() {
            Ok(path) if path.is_file() => path,
            _ => {
                omitted += 1;
                continue;
            }
        };
        let relative = artifact
            .strip_prefix(library)
            .map_err(|_| PlaylistError::ArtifactOutsideLibrary(artifact.clone()))?;
        let relative = relative
            .to_str()
            .filter(|path| !path.contains(['\n', '\r']))
            .ok_or_else(|| PlaylistError::UnsafeArtifactPath(relative.to_path_buf()))?;
        contents.push_str(&relative.replace(std::path::MAIN_SEPARATOR, "/"));
        contents.push('\n');
    }
    let final_path = output.join(format!("collection-{}.m3u8", snapshot.collection_id));
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(PlaylistError::Clock)?
        .as_nanos();
    let temporary = output.join(format!(
        ".collection-{}.m3u8.tmp-{}-{nonce}",
        snapshot.collection_id,
        std::process::id()
    ));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .map_err(|source| PlaylistError::Write {
            path: temporary.clone(),
            source,
        })?;
    file.write_all(contents.as_bytes())
        .and_then(|()| file.sync_all())
        .map_err(|source| PlaylistError::Write {
            path: temporary.clone(),
            source,
        })?;
    drop(file);
    let hash = Sha256FileHasher
        .hash(&temporary)
        .map_err(PlaylistError::Hash)?;
    let sha256 = hex_sha256(hash.sha256);
    let ownership = database.prepare_playlist_output(snapshot.collection_id, &final_path)?;
    let effect = if ownership.newly_reserved {
        link_initial(&temporary, &final_path, false, &sha256)?
    } else if ownership.prior_sha256.is_none() {
        link_initial(&temporary, &final_path, true, &sha256)?
    } else {
        let unchanged = Sha256FileHasher
            .hash(&final_path)
            .ok()
            .is_some_and(|hash| hex_sha256(hash.sha256) == sha256);
        if unchanged {
            fs::remove_file(&temporary).map_err(|source| PlaylistError::Write {
                path: temporary.clone(),
                source,
            })?;
            PlaylistWriteEffect::Unchanged
        } else {
            fs::rename(&temporary, &final_path).map_err(|source| PlaylistError::Write {
                path: final_path.clone(),
                source,
            })?;
            PlaylistWriteEffect::Updated
        }
    };
    File::open(output)
        .and_then(|directory| directory.sync_all())
        .map_err(|source| PlaylistError::Write {
            path: output.to_path_buf(),
            source,
        })?;
    database.record_playlist_output(snapshot.collection_id, &sha256)?;
    Ok(PlaylistMaterialized {
        collection_id: snapshot.collection_id,
        name: snapshot.name.clone(),
        path: final_path,
        entries: (snapshot.entries.len() as u64).saturating_sub(omitted - snapshot.unresolved),
        omitted,
        effect,
    })
}

fn link_initial(
    temporary: &Path,
    final_path: &Path,
    allow_recovery: bool,
    sha256: &str,
) -> Result<PlaylistWriteEffect, PlaylistError> {
    match fs::hard_link(temporary, final_path) {
        Ok(()) => {
            fs::remove_file(temporary).map_err(|source| PlaylistError::Write {
                path: temporary.to_path_buf(),
                source,
            })?;
            Ok(PlaylistWriteEffect::Created)
        }
        Err(source) if source.kind() == std::io::ErrorKind::AlreadyExists && allow_recovery => {
            let existing = Sha256FileHasher
                .hash(final_path)
                .map_err(PlaylistError::Hash)?;
            if hex_sha256(existing.sha256) != sha256 {
                return Err(PlaylistError::ExistingMismatch(final_path.to_path_buf()));
            }
            fs::remove_file(temporary).map_err(|source| PlaylistError::Write {
                path: temporary.to_path_buf(),
                source,
            })?;
            Ok(PlaylistWriteEffect::RecoveredExisting)
        }
        Err(source) if source.kind() == std::io::ErrorKind::AlreadyExists => {
            Err(PlaylistError::ExistingUnowned(final_path.to_path_buf()))
        }
        Err(source) => Err(PlaylistError::Link {
            temporary: temporary.to_path_buf(),
            final_path: final_path.to_path_buf(),
            source,
        }),
    }
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

/// Aggregate collection materialization outcome.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct PlaylistMaterializationReport {
    /// Successfully materialized collections.
    pub playlists: Vec<PlaylistMaterialized>,
    /// Independently failed collections.
    pub failures: Vec<PlaylistFailure>,
}

/// One successfully materialized collection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PlaylistMaterialized {
    /// Durable collection ID.
    pub collection_id: i64,
    /// Current collection name.
    pub name: String,
    /// Stable owned output path.
    pub path: PathBuf,
    /// Written artifact entries.
    pub entries: u64,
    /// Active memberships omitted due to unresolved or unhealthy artifacts.
    pub omitted: u64,
    /// Atomic filesystem effect.
    pub effect: PlaylistWriteEffect,
}

/// One isolated collection materialization failure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PlaylistFailure {
    /// Durable failed collection ID.
    pub collection_id: i64,
    /// User-facing failure diagnostic.
    pub message: String,
}

/// Observable atomic playlist write effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PlaylistWriteEffect {
    /// A previously absent owned path was created without clobbering.
    Created,
    /// Existing owned output was atomically replaced.
    Updated,
    /// Existing owned output already contained the exact generated bytes.
    Unchanged,
    /// A prepared first write was recovered after its filesystem commit.
    RecoveredExisting,
}

/// Failure to safely render or atomically commit playlist output.
#[derive(Debug, Error)]
pub enum PlaylistError {
    /// SQLite query or ownership state failed.
    #[error(transparent)]
    Database(#[from] DatabaseError),
    /// A configured directory could not be created or resolved.
    #[error("failed to access playlist directory {path}: {source}")]
    Directory {
        /// Configured directory.
        path: PathBuf,
        /// Underlying filesystem error.
        source: std::io::Error,
    },
    /// A healthy artifact resolved outside the configured managed library.
    #[error("playlist artifact is outside the managed library: {0}")]
    ArtifactOutsideLibrary(PathBuf),
    /// A relative artifact path could not be represented safely in UTF-8 M3U8.
    #[error("playlist artifact path is not safe UTF-8: {0}")]
    UnsafeArtifactPath(PathBuf),
    /// Temporary or final output could not be written or synchronized.
    #[error("failed to write playlist path {path}: {source}")]
    Write {
        /// Affected path.
        path: PathBuf,
        /// Underlying filesystem error.
        source: std::io::Error,
    },
    /// Temporary output hashing failed.
    #[error("failed to hash playlist output: {0}")]
    Hash(ContentHashError),
    /// Initial no-clobber link failed.
    #[error("failed to atomically link playlist {temporary} to {final_path}: {source}")]
    Link {
        /// Synchronized temporary path.
        temporary: PathBuf,
        /// Intended final path.
        final_path: PathBuf,
        /// Underlying filesystem error.
        source: std::io::Error,
    },
    /// An unknown existing output was preserved.
    #[error("existing playlist output is not owned by music-sync: {0}")]
    ExistingUnowned(PathBuf),
    /// A prepared first-write recovery found different bytes.
    #[error("prepared playlist output has different existing bytes: {0}")]
    ExistingMismatch(PathBuf),
    /// System clock could not produce a temporary-name nonce.
    #[error("system clock is before the Unix epoch: {0}")]
    Clock(std::time::SystemTimeError),
}

impl PlaylistError {
    fn is_database_failure(&self) -> bool {
        matches!(self, Self::Database(_))
    }
}
