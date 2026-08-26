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
    let mut report = PlaylistMaterializationReport::default();
    for (collection_id, output_state) in database.non_playlist_outputs()? {
        match retire_non_playlist_output(database, collection_id, &output_state) {
            Ok(()) => {}
            Err(error) if error.is_database_failure() => return Err(error),
            Err(error) => report.failures.push(PlaylistFailure {
                collection_id,
                message: error.to_string(),
            }),
        }
    }
    let snapshots = database.playlist_snapshots()?;
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

fn retire_non_playlist_output(
    database: &mut Database,
    collection_id: i64,
    output: &crate::persistence::PlaylistOutputState,
) -> Result<(), PlaylistError> {
    if output.path.exists() {
        let observed = hex_sha256(
            Sha256FileHasher
                .hash(&output.path)
                .map_err(PlaylistError::Hash)?
                .sha256,
        );
        if output.sha256.as_deref() != Some(observed.as_str()) {
            return Err(PlaylistError::ExistingMismatch(output.path.clone()));
        }
        fs::remove_file(&output.path).map_err(|source| PlaylistError::Write {
            path: output.path.clone(),
            source,
        })?;
    }
    database.forget_playlist_output(collection_id)?;
    Ok(())
}

fn materialize_one(
    database: &mut Database,
    snapshot: &PlaylistSnapshot,
    library: &Path,
    output: &Path,
) -> Result<PlaylistMaterialized, PlaylistError> {
    let mut contents = String::from("#EXTM3U\n");
    let mut omitted = snapshot.unresolved;
    let final_path = named_playlist_path(database, output, &snapshot.name, snapshot.collection_id)?;
    migrate_or_adopt_output(database, snapshot, library, &final_path)?;
    let snapshot = database
        .playlist_snapshots()?
        .into_iter()
        .find(|candidate| candidate.collection_id == snapshot.collection_id)
        .ok_or(PlaylistError::MissingCollection(snapshot.collection_id))?;
    let mut rendered = std::collections::BTreeSet::new();
    for entry in &snapshot.preserved_entries {
        if !entry.contains(['\n', '\r']) && rendered.insert(entry.clone()) {
            contents.push_str(entry);
            contents.push('\n');
        }
    }
    for artifact in &snapshot.entries {
        let artifact = match artifact.canonicalize() {
            Ok(path) if path.is_file() => path,
            _ => {
                omitted += 1;
                continue;
            }
        };
        artifact
            .strip_prefix(library)
            .map_err(|_| PlaylistError::ArtifactOutsideLibrary(artifact.clone()))?;
        let relative = relative_path(output, &artifact);
        let relative = relative
            .to_str()
            .filter(|path| !path.contains(['\n', '\r']))
            .ok_or_else(|| PlaylistError::UnsafeArtifactPath(relative.to_path_buf()))?;
        let relative = relative.replace(std::path::MAIN_SEPARATOR, "/");
        if rendered.insert(relative.clone()) {
            contents.push_str(&relative);
            contents.push('\n');
        }
    }
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
        entries: rendered.len() as u64,
        omitted,
        effect,
    })
}

fn migrate_or_adopt_output(
    database: &mut Database,
    snapshot: &PlaylistSnapshot,
    library: &Path,
    desired: &Path,
) -> Result<(), PlaylistError> {
    let ownership = database.playlist_output(snapshot.collection_id)?;
    if ownership
        .as_ref()
        .is_some_and(|state| state.path == desired)
    {
        return Ok(());
    }
    if desired.exists() {
        let desired_hash = hex_sha256(
            Sha256FileHasher
                .hash(desired)
                .map_err(PlaylistError::Hash)?
                .sha256,
        );
        let moved_recovery = ownership
            .as_ref()
            .and_then(|state| state.sha256.as_deref())
            .is_some_and(|sha256| sha256 == desired_hash);
        let preserved = if moved_recovery {
            Vec::new()
        } else {
            existing_playlist_entries(desired, library, &snapshot.entries)?
        };
        if let Some(old) = &ownership {
            retire_owned_output(&old.path, old.sha256.as_deref(), desired)?;
        }
        database.adopt_playlist_output(
            snapshot.collection_id,
            desired,
            &desired_hash,
            &preserved,
        )?;
        return Ok(());
    }
    if let Some(old) = ownership
        && old.path.exists()
    {
        let observed = hex_sha256(
            Sha256FileHasher
                .hash(&old.path)
                .map_err(PlaylistError::Hash)?
                .sha256,
        );
        if old.sha256.as_deref() != Some(observed.as_str()) {
            return Err(PlaylistError::ExistingMismatch(old.path));
        }
        fs::rename(&old.path, desired).map_err(|source| PlaylistError::Write {
            path: desired.to_path_buf(),
            source,
        })?;
        database.adopt_playlist_output(snapshot.collection_id, desired, &observed, &[])?;
        return Ok(());
    }
    Ok(())
}

fn existing_playlist_entries(
    path: &Path,
    library: &Path,
    managed: &[PathBuf],
) -> Result<Vec<String>, PlaylistError> {
    let bytes = fs::read(path).map_err(|source| PlaylistError::Write {
        path: path.to_path_buf(),
        source,
    })?;
    let text = std::str::from_utf8(&bytes)
        .map_err(|_| PlaylistError::UnsafeExistingPlaylist(path.to_path_buf()))?;
    let managed = managed
        .iter()
        .filter_map(|entry| entry.canonicalize().ok())
        .collect::<std::collections::BTreeSet<_>>();
    let parent = path
        .parent()
        .ok_or_else(|| PlaylistError::UnsafeExistingPlaylist(path.into()))?;
    let mut preserved = Vec::new();
    for line in text.lines().map(str::trim) {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let candidate = if Path::new(line).is_absolute() {
            PathBuf::from(line)
        } else {
            parent.join(line)
        };
        let managed_entry = candidate.canonicalize().ok().is_some_and(|candidate| {
            candidate.starts_with(library) && managed.contains(&candidate)
        });
        if !managed_entry && !preserved.iter().any(|entry| entry == line) {
            preserved.push(line.to_owned());
        }
    }
    Ok(preserved)
}

fn retire_owned_output(
    old: &Path,
    expected_sha256: Option<&str>,
    desired: &Path,
) -> Result<(), PlaylistError> {
    if old == desired || !old.exists() {
        return Ok(());
    }
    let observed = hex_sha256(
        Sha256FileHasher
            .hash(old)
            .map_err(PlaylistError::Hash)?
            .sha256,
    );
    if expected_sha256 != Some(observed.as_str()) {
        return Err(PlaylistError::ExistingMismatch(old.to_path_buf()));
    }
    fs::remove_file(old).map_err(|source| PlaylistError::Write {
        path: old.to_path_buf(),
        source,
    })
}

fn playlist_stem(name: &str, collection_id: i64) -> String {
    let stem = name
        .trim()
        .chars()
        .map(|character| match character {
            '/' | '\\' | '\0' | '\n' | '\r' => '_',
            character => character,
        })
        .collect::<String>();
    let stem = stem.trim_matches(['.', ' ']);
    if stem.is_empty() {
        format!("collection-{collection_id}")
    } else {
        stem.to_owned()
    }
}

fn named_playlist_path(
    database: &Database,
    output: &Path,
    name: &str,
    collection_id: i64,
) -> Result<PathBuf, PlaylistError> {
    let stem = playlist_stem(name, collection_id);
    let m3u8 = output.join(format!("{stem}.m3u8"));
    let m3u = output.join(format!("{stem}.m3u"));
    let ownership = database.playlist_output(collection_id)?;
    if ownership.as_ref().is_some_and(|state| state.path == m3u8) {
        return Ok(m3u8);
    }
    if ownership.as_ref().is_some_and(|state| state.path == m3u) {
        return Ok(m3u);
    }
    match (m3u8.exists(), m3u.exists()) {
        (true, true) => Err(PlaylistError::AmbiguousNamedOutputs { m3u8, m3u }),
        (true, false) => Ok(m3u8),
        (false, true) => Ok(m3u),
        (false, false) => Ok(m3u8),
    }
}

fn relative_path(from: &Path, to: &Path) -> PathBuf {
    let from = from.components().collect::<Vec<_>>();
    let to = to.components().collect::<Vec<_>>();
    let common = from
        .iter()
        .zip(&to)
        .take_while(|(left, right)| left == right)
        .count();
    let mut relative = PathBuf::new();
    for _ in common..from.len() {
        relative.push("..");
    }
    for component in &to[common..] {
        relative.push(component.as_os_str());
    }
    relative
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
    /// A durable collection disappeared during one materialization operation.
    #[error("playlist collection disappeared during materialization: {0}")]
    MissingCollection(i64),
    /// An existing named playlist could not be safely parsed for adoption.
    #[error("existing playlist is not safe UTF-8 M3U8: {0}")]
    UnsafeExistingPlaylist(PathBuf),
    /// Both supported filename extensions already exist without owned disambiguation.
    #[error("both named playlist outputs exist and ownership cannot disambiguate: {m3u8}, {m3u}")]
    AmbiguousNamedOutputs {
        /// Existing UTF-8-extension candidate.
        m3u8: PathBuf,
        /// Existing legacy-extension candidate.
        m3u: PathBuf,
    },
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
