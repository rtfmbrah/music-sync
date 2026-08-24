//! Read-only inspection of arbitrary existing music libraries.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::path::{Path, PathBuf};

use serde::Serialize;
use thiserror::Error;
use walkdir::WalkDir;

use crate::content_hash::ContentHasher;
use crate::media_probe::{MediaProbe, MediaProperties};
use crate::persistence::{AdoptionPersistenceSummary, Database, DatabaseError};

/// Aggregate result of a read-only library scan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AdoptionReport {
    /// Scanned root, rendered lossily so non-Unicode paths cannot break JSON output.
    pub root: String,
    /// Number of directories visited, including the root.
    pub directories_scanned: u64,
    /// Number of regular files visited.
    pub files_scanned: u64,
    /// Recognized, non-empty audio files.
    pub media_files: u64,
    /// Recognized lyric sidecars.
    pub lyric_files: u64,
    /// Recognized artwork candidates.
    pub artwork_files: u64,
    /// Generic M3U/M3U8 playlist files.
    pub playlist_files: u64,
    /// Files whose purpose is not recognized. They must still be preserved.
    pub unknown_files: u64,
    /// Symbolic links observed but deliberately not followed.
    pub symbolic_links: u64,
    /// Empty media files that require future review rather than adoption as healthy.
    pub corrupt_media: u64,
    /// Paths that could not be inspected or opened.
    pub unreadable_paths: u64,
    /// Conservative relationships observed among media, sidecars, and playlists.
    pub relationships: AdoptionRelationshipSummary,
    /// Bounded details for noteworthy paths.
    pub issues: Vec<AdoptionIssue>,
    /// Guaranteed side-effect counts for this dry-run operation.
    pub effects: AdoptionEffects,
    /// Optional structural media-probe summary.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub probe: Option<MediaProbeSummary>,
    /// Optional exact physical-artifact hash summary.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hash: Option<ArtifactHashSummary>,
}

impl AdoptionReport {
    /// Reports the guaranteed side-effect counts for a dry-run scan.
    #[must_use]
    pub const fn effects(&self) -> AdoptionEffects {
        self.effects
    }
}

/// Guaranteed filesystem effects of the scanner.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct AdoptionEffects {
    /// Files modified by the scan.
    pub files_modified: u64,
    /// Files deleted by the scan.
    pub files_deleted: u64,
    /// Files downloaded by the scan.
    pub files_downloaded: u64,
}

/// One noteworthy path found while scanning.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AdoptionIssue {
    /// Path rendered relative to the scan root where possible.
    pub path: String,
    /// Stable issue classification.
    pub kind: AdoptionIssueKind,
    /// Human-readable English explanation.
    pub message: String,
}

/// Class of noteworthy adoption observation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AdoptionIssueKind {
    /// A media file has zero bytes and cannot be considered healthy.
    EmptyMedia,
    /// A path could not be read or inspected.
    Unreadable,
    /// A media file could not be structurally inspected by the configured probe.
    ProbeFailed,
    /// A playlist points inside the scan root but not to recognized healthy media.
    MissingPlaylistReference,
    /// A playlist reference points outside the scan root or uses a URI.
    ExternalPlaylistReference,
    /// A playlist could not be parsed safely within the configured bound.
    InvalidPlaylist,
    /// A recognized media file could not be hashed completely.
    HashFailed,
}

/// Conservative relationship counts discovered without modifying the library.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct AdoptionRelationshipSummary {
    /// LRC files with recognized same-directory, same-stem media.
    pub lyrics_associated: u64,
    /// LRC files without recognized same-stem media.
    pub lyrics_orphaned: u64,
    /// Artwork candidates in a directory containing recognized media.
    pub artwork_beside_media: u64,
    /// Artwork candidates in a directory without recognized media.
    pub artwork_orphaned: u64,
    /// Non-comment, non-empty entries read from valid playlists.
    pub playlist_entries: u64,
    /// Playlist entries resolving to recognized media inside the scan.
    pub playlist_references_resolved: u64,
    /// Playlist entries targeting the scan root but not recognized media.
    pub playlist_references_missing: u64,
    /// URI or filesystem references outside the scan root.
    pub playlist_references_external: u64,
    /// Playlists rejected because they exceed the size bound or are not UTF-8.
    pub playlist_files_invalid: u64,
}

/// Scans `root` recursively without following links or changing any file.
pub fn scan_library(root: &Path) -> Result<AdoptionReport, AdoptionError> {
    let (report, _) = scan_files(root)?;
    Ok(report)
}

/// Explicitly registers recognized media in SQLite without modifying library files.
pub fn apply_library(
    root: &Path,
    database: &mut Database,
) -> Result<AdoptionApplyReport, AdoptionApplyError> {
    if !root.is_absolute() {
        return Err(AdoptionApplyError::RelativeRoot(root.to_path_buf()));
    }
    let (scan, media_paths) = scan_files(root)?;
    let database = database.register_adopted_artifacts(&media_paths)?;
    Ok(AdoptionApplyReport { scan, database })
}

/// Result of an explicit transactional adoption apply.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AdoptionApplyReport {
    /// Read-only filesystem observations used for the apply.
    pub scan: AdoptionReport,
    /// Rows inserted or observed in music-sync-owned SQLite.
    pub database: AdoptionPersistenceSummary,
}

/// Failure during explicit adoption apply.
#[derive(Debug, Error)]
pub enum AdoptionApplyError {
    /// Apply requires a stable absolute artifact namespace.
    #[error("adoption apply requires an absolute library path: {0}")]
    RelativeRoot(PathBuf),
    /// The read-only library scan failed.
    #[error(transparent)]
    Scan(#[from] AdoptionError),
    /// The transactional SQLite registration failed.
    #[error(transparent)]
    Database(#[from] DatabaseError),
}

/// Scans a library and applies a bounded read-only media probe.
pub fn scan_library_with_probe(
    root: &Path,
    probe: &dyn MediaProbe,
    limit: ProbeLimit,
) -> Result<AdoptionReport, AdoptionError> {
    let (mut report, media_paths) = scan_files(root)?;
    apply_probe(root, &media_paths, probe, limit, &mut report);
    Ok(report)
}

/// Scans a library and derives bounded exact-artifact hash evidence.
pub fn scan_library_with_hash(
    root: &Path,
    hasher: &dyn ContentHasher,
    limit: HashLimit,
) -> Result<AdoptionReport, AdoptionError> {
    let (mut report, media_paths) = scan_files(root)?;
    apply_hash(root, &media_paths, hasher, limit, &mut report);
    Ok(report)
}

/// Scans once, then applies independently bounded probing and hashing.
pub fn scan_library_with_probe_and_hash(
    root: &Path,
    probe: &dyn MediaProbe,
    probe_limit: ProbeLimit,
    hasher: &dyn ContentHasher,
    hash_limit: HashLimit,
) -> Result<AdoptionReport, AdoptionError> {
    let (mut report, media_paths) = scan_files(root)?;
    apply_probe(root, &media_paths, probe, probe_limit, &mut report);
    apply_hash(root, &media_paths, hasher, hash_limit, &mut report);
    Ok(report)
}

fn apply_probe(
    root: &Path,
    media_paths: &[PathBuf],
    probe: &dyn MediaProbe,
    limit: ProbeLimit,
    report: &mut AdoptionReport,
) {
    let maximum = match limit {
        ProbeLimit::All => media_paths.len(),
        ProbeLimit::Files(maximum) => maximum.get().min(media_paths.len()),
    };
    let mut summary = MediaProbeSummary {
        attempted: maximum as u64,
        succeeded: 0,
        failed: 0,
        skipped_due_to_limit: (media_paths.len() - maximum) as u64,
        duration_known: 0,
        total_duration_ms: 0,
        with_embedded_artwork: 0,
        with_basic_tags: 0,
        with_canonical_identity: 0,
        with_valid_musicbrainz_recording_id: 0,
        with_valid_isrc: 0,
        with_malformed_canonical_tag: 0,
        codecs: BTreeMap::new(),
    };
    for path in media_paths.iter().take(maximum) {
        match probe.probe(path) {
            Ok(properties) => record_probe_success(&mut summary, &properties),
            Err(error) => {
                summary.failed += 1;
                push_issue(
                    report,
                    AdoptionIssue {
                        path: display_relative(root, path),
                        kind: AdoptionIssueKind::ProbeFailed,
                        message: error.to_string(),
                    },
                );
            }
        }
    }
    report.probe = Some(summary);
}

/// Maximum number of recognized media files to inspect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeLimit {
    /// Probe every recognized, non-empty media file.
    All,
    /// Probe at most this many media files.
    Files(std::num::NonZeroUsize),
}

/// Maximum number of recognized media files to hash.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HashLimit {
    /// Hash every recognized, non-empty media file.
    All,
    /// Hash at most this many media files.
    Files(std::num::NonZeroUsize),
}

/// Aggregate SHA-256 evidence for exact physical artifacts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ArtifactHashSummary {
    /// Number of hash attempts.
    pub attempted: u64,
    /// Files hashed completely.
    pub succeeded: u64,
    /// Files that could not be hashed completely.
    pub failed: u64,
    /// Recognized media skipped by the configured limit.
    pub skipped_due_to_limit: u64,
    /// Saturating total of file bytes hashed successfully.
    pub bytes_hashed: u64,
    /// Distinct digest groups containing at least two files.
    pub exact_duplicate_groups: u64,
    /// Files belonging to an exact-duplicate group, including one original per group.
    pub exact_duplicate_files: u64,
}

fn apply_hash(
    root: &Path,
    media_paths: &[PathBuf],
    hasher: &dyn ContentHasher,
    limit: HashLimit,
    report: &mut AdoptionReport,
) {
    let maximum = match limit {
        HashLimit::All => media_paths.len(),
        HashLimit::Files(maximum) => maximum.get().min(media_paths.len()),
    };
    let mut digest_counts = BTreeMap::<[u8; 32], u64>::new();
    let mut summary = ArtifactHashSummary {
        attempted: maximum as u64,
        succeeded: 0,
        failed: 0,
        skipped_due_to_limit: (media_paths.len() - maximum) as u64,
        bytes_hashed: 0,
        exact_duplicate_groups: 0,
        exact_duplicate_files: 0,
    };
    for path in media_paths.iter().take(maximum) {
        match hasher.hash(path) {
            Ok(content_hash) => {
                summary.succeeded += 1;
                summary.bytes_hashed = summary
                    .bytes_hashed
                    .saturating_add(content_hash.bytes_hashed);
                *digest_counts.entry(content_hash.sha256).or_default() += 1;
            }
            Err(error) => {
                summary.failed += 1;
                push_issue(
                    report,
                    AdoptionIssue {
                        path: display_relative(root, path),
                        kind: AdoptionIssueKind::HashFailed,
                        message: error.to_string(),
                    },
                );
            }
        }
    }
    for count in digest_counts.values().copied().filter(|count| *count > 1) {
        summary.exact_duplicate_groups += 1;
        summary.exact_duplicate_files = summary.exact_duplicate_files.saturating_add(count);
    }
    report.hash = Some(summary);
}

/// Aggregate structural properties from optional media probing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MediaProbeSummary {
    /// Number of probe attempts.
    pub attempted: u64,
    /// Successful structural probes.
    pub succeeded: u64,
    /// Failed, malformed, or timed-out probes.
    pub failed: u64,
    /// Recognized media intentionally skipped by the configured limit.
    pub skipped_due_to_limit: u64,
    /// Successful probes with a known duration.
    pub duration_known: u64,
    /// Saturating sum of known durations.
    pub total_duration_ms: u64,
    /// Probed files containing attached artwork streams.
    pub with_embedded_artwork: u64,
    /// Probed files containing title, artist, or album tags.
    pub with_basic_tags: u64,
    /// Probed files containing a recording MBID or ISRC.
    pub with_canonical_identity: u64,
    /// Probed files containing a structurally valid recording MBID.
    pub with_valid_musicbrainz_recording_id: u64,
    /// Probed files containing a structurally valid ISRC.
    pub with_valid_isrc: u64,
    /// Probed files containing at least one malformed canonical tag.
    pub with_malformed_canonical_tag: u64,
    /// Audio codec counts in deterministic key order.
    pub codecs: BTreeMap<String, u64>,
}

fn scan_files(root: &Path) -> Result<(AdoptionReport, Vec<PathBuf>), AdoptionError> {
    validate_root(root)?;
    let mut report = AdoptionReport {
        root: root.to_string_lossy().into_owned(),
        directories_scanned: 0,
        files_scanned: 0,
        media_files: 0,
        lyric_files: 0,
        artwork_files: 0,
        playlist_files: 0,
        unknown_files: 0,
        symbolic_links: 0,
        corrupt_media: 0,
        unreadable_paths: 0,
        relationships: AdoptionRelationshipSummary::default(),
        issues: Vec::new(),
        effects: AdoptionEffects {
            files_modified: 0,
            files_deleted: 0,
            files_downloaded: 0,
        },
        probe: None,
        hash: None,
    };
    let mut paths = ScanPaths::default();

    for entry in WalkDir::new(root).follow_links(false).sort_by_file_name() {
        match entry {
            Ok(entry) => inspect_entry(
                root,
                entry.path(),
                entry.file_type(),
                &mut report,
                &mut paths,
            ),
            Err(error) => record_walk_error(root, &error, &mut report),
        }
    }
    validate_relationships(root, &paths, &mut report);
    Ok((report, paths.media))
}

#[derive(Debug, Default)]
struct ScanPaths {
    media: Vec<PathBuf>,
    lyrics: Vec<PathBuf>,
    artwork: Vec<PathBuf>,
    playlists: Vec<PathBuf>,
}

fn record_probe_success(summary: &mut MediaProbeSummary, properties: &MediaProperties) {
    summary.succeeded += 1;
    *summary.codecs.entry(properties.codec.clone()).or_default() += 1;
    if let Some(duration_ms) = properties.duration_ms {
        summary.duration_known += 1;
        summary.total_duration_ms = summary.total_duration_ms.saturating_add(duration_ms);
    }
    summary.with_embedded_artwork += u64::from(properties.has_embedded_artwork);
    summary.with_basic_tags += u64::from(properties.tags.has_basic_tags);
    summary.with_canonical_identity += u64::from(properties.tags.has_canonical_identity());
    summary.with_valid_musicbrainz_recording_id += u64::from(matches!(
        properties.tags.musicbrainz_recording_id,
        crate::media_probe::CanonicalTagStatus::Valid
    ));
    summary.with_valid_isrc += u64::from(matches!(
        properties.tags.isrc,
        crate::media_probe::CanonicalTagStatus::Valid
    ));
    summary.with_malformed_canonical_tag +=
        u64::from(properties.tags.has_malformed_canonical_tag());
}

fn validate_root(root: &Path) -> Result<(), AdoptionError> {
    let metadata = fs::metadata(root).map_err(|source| AdoptionError::ReadRoot {
        path: root.to_path_buf(),
        source,
    })?;
    if !metadata.is_dir() {
        return Err(AdoptionError::NotDirectory(root.to_path_buf()));
    }
    Ok(())
}

fn inspect_entry(
    root: &Path,
    path: &Path,
    file_type: fs::FileType,
    report: &mut AdoptionReport,
    paths: &mut ScanPaths,
) {
    if file_type.is_dir() {
        report.directories_scanned += 1;
        return;
    }
    if file_type.is_symlink() {
        report.symbolic_links += 1;
        return;
    }
    if !file_type.is_file() {
        report.unknown_files += 1;
        return;
    }

    report.files_scanned += 1;
    if let Err(error) = File::open(path) {
        report.unreadable_paths += 1;
        push_issue(
            report,
            AdoptionIssue {
                path: display_relative(root, path),
                kind: AdoptionIssueKind::Unreadable,
                message: format!("File could not be opened: {error}"),
            },
        );
        return;
    }

    match classify_path(path) {
        FileClass::Media => match fs::metadata(path) {
            Ok(metadata) if metadata.len() == 0 => {
                report.corrupt_media += 1;
                push_issue(
                    report,
                    AdoptionIssue {
                        path: display_relative(root, path),
                        kind: AdoptionIssueKind::EmptyMedia,
                        message: "Media file is empty and requires review".into(),
                    },
                );
            }
            Ok(_) => {
                report.media_files += 1;
                paths.media.push(path.to_path_buf());
            }
            Err(error) => {
                report.unreadable_paths += 1;
                push_issue(
                    report,
                    AdoptionIssue {
                        path: display_relative(root, path),
                        kind: AdoptionIssueKind::Unreadable,
                        message: format!("File metadata could not be read: {error}"),
                    },
                );
            }
        },
        FileClass::Lyrics => {
            report.lyric_files += 1;
            paths.lyrics.push(path.to_path_buf());
        }
        FileClass::Artwork => {
            report.artwork_files += 1;
            paths.artwork.push(path.to_path_buf());
        }
        FileClass::Playlist => {
            report.playlist_files += 1;
            paths.playlists.push(path.to_path_buf());
        }
        FileClass::Unknown => report.unknown_files += 1,
    }
}

const MAX_PLAYLIST_BYTES: u64 = 8 * 1024 * 1024;

fn validate_relationships(root: &Path, paths: &ScanPaths, report: &mut AdoptionReport) {
    let media: BTreeSet<PathBuf> = paths
        .media
        .iter()
        .map(|path| lexical_normalize(path))
        .collect();
    let media_stems: BTreeSet<PathBuf> = paths
        .media
        .iter()
        .map(|path| path.with_extension(""))
        .collect();
    let media_directories: BTreeSet<PathBuf> = paths
        .media
        .iter()
        .filter_map(|path| path.parent().map(Path::to_path_buf))
        .collect();

    for lyrics in &paths.lyrics {
        if media_stems.contains(&lyrics.with_extension("")) {
            report.relationships.lyrics_associated += 1;
        } else {
            report.relationships.lyrics_orphaned += 1;
        }
    }
    for artwork in &paths.artwork {
        if artwork
            .parent()
            .is_some_and(|parent| media_directories.contains(parent))
        {
            report.relationships.artwork_beside_media += 1;
        } else {
            report.relationships.artwork_orphaned += 1;
        }
    }
    for playlist in &paths.playlists {
        validate_playlist(root, playlist, &media, report);
    }
}

fn validate_playlist(
    root: &Path,
    playlist: &Path,
    media: &BTreeSet<PathBuf>,
    report: &mut AdoptionReport,
) {
    let contents = match fs::metadata(playlist) {
        Ok(metadata) if metadata.len() > MAX_PLAYLIST_BYTES => {
            record_invalid_playlist(
                root,
                playlist,
                "Playlist exceeds the 8 MiB validation limit",
                report,
            );
            return;
        }
        Ok(_) => match fs::read(playlist) {
            Ok(contents) => contents,
            Err(error) => {
                record_invalid_playlist(
                    root,
                    playlist,
                    &format!("Playlist could not be read: {error}"),
                    report,
                );
                return;
            }
        },
        Err(error) => {
            record_invalid_playlist(
                root,
                playlist,
                &format!("Playlist metadata could not be read: {error}"),
                report,
            );
            return;
        }
    };
    let text = match std::str::from_utf8(&contents) {
        Ok(text) => text,
        Err(error) => {
            record_invalid_playlist(
                root,
                playlist,
                &format!("Playlist is not valid UTF-8: {error}"),
                report,
            );
            return;
        }
    };
    let normalized_root = lexical_normalize(root);
    let parent = playlist.parent().unwrap_or(root);
    for (line_index, raw_line) in text.lines().enumerate() {
        let entry = raw_line.trim().trim_start_matches('\u{feff}');
        if entry.is_empty() || entry.starts_with('#') {
            continue;
        }
        report.relationships.playlist_entries += 1;
        let issue_path = format!("{}:{}", display_relative(root, playlist), line_index + 1);
        if entry.contains("://") {
            record_external_reference(report, issue_path, entry);
            continue;
        }
        let entry_path = Path::new(entry);
        let resolved = if entry_path.is_absolute() {
            lexical_normalize(entry_path)
        } else {
            lexical_normalize(&parent.join(entry_path))
        };
        if !resolved.starts_with(&normalized_root) {
            record_external_reference(report, issue_path, entry);
        } else if media.contains(&resolved) {
            report.relationships.playlist_references_resolved += 1;
        } else {
            report.relationships.playlist_references_missing += 1;
            push_issue(
                report,
                AdoptionIssue {
                    path: issue_path,
                    kind: AdoptionIssueKind::MissingPlaylistReference,
                    message: format!(
                        "Playlist reference does not resolve to recognized media: {entry}"
                    ),
                },
            );
        }
    }
}

fn record_external_reference(report: &mut AdoptionReport, path: String, entry: &str) {
    report.relationships.playlist_references_external += 1;
    push_issue(
        report,
        AdoptionIssue {
            path,
            kind: AdoptionIssueKind::ExternalPlaylistReference,
            message: format!("Playlist reference is external to the scanned root: {entry}"),
        },
    );
}

fn record_invalid_playlist(
    root: &Path,
    playlist: &Path,
    message: &str,
    report: &mut AdoptionReport,
) {
    report.relationships.playlist_files_invalid += 1;
    push_issue(
        report,
        AdoptionIssue {
            path: display_relative(root, playlist),
            kind: AdoptionIssueKind::InvalidPlaylist,
            message: message.into(),
        },
    );
}

fn lexical_normalize(path: &Path) -> PathBuf {
    use std::path::Component;
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
}

fn record_walk_error(root: &Path, error: &walkdir::Error, report: &mut AdoptionReport) {
    report.unreadable_paths += 1;
    let path = error.path().map_or_else(
        || "[unknown path]".into(),
        |path| display_relative(root, path),
    );
    push_issue(
        report,
        AdoptionIssue {
            path,
            kind: AdoptionIssueKind::Unreadable,
            message: format!("Path could not be traversed: {error}"),
        },
    );
}

const MAX_REPORTED_ISSUES: usize = 100;

fn push_issue(report: &mut AdoptionReport, issue: AdoptionIssue) {
    if report.issues.len() < MAX_REPORTED_ISSUES {
        report.issues.push(issue);
    }
}

fn display_relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FileClass {
    Media,
    Lyrics,
    Artwork,
    Playlist,
    Unknown,
}

fn classify_path(path: &Path) -> FileClass {
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase);
    match extension.as_deref() {
        Some(
            "aac" | "aif" | "aiff" | "alac" | "flac" | "m4a" | "mka" | "mp3" | "ogg" | "opus"
            | "wav" | "wma",
        ) => FileClass::Media,
        Some("lrc") => FileClass::Lyrics,
        Some("jpg" | "jpeg" | "png" | "webp") => FileClass::Artwork,
        Some("m3u" | "m3u8") => FileClass::Playlist,
        _ => FileClass::Unknown,
    }
}

/// Failure to start a library scan.
#[derive(Debug, Error)]
pub enum AdoptionError {
    /// Root metadata could not be read.
    #[error("failed to inspect library root {path}: {source}")]
    ReadRoot {
        /// Requested library root.
        path: PathBuf,
        /// Underlying I/O error.
        source: std::io::Error,
    },
    /// The requested root is not a directory.
    #[error("library root is not a directory: {0}")]
    NotDirectory(PathBuf),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_extensions_case_insensitively() {
        assert_eq!(classify_path(Path::new("track.M4A")), FileClass::Media);
        assert_eq!(classify_path(Path::new("track.LRC")), FileClass::Lyrics);
        assert_eq!(classify_path(Path::new("list.M3U8")), FileClass::Playlist);
        assert_eq!(classify_path(Path::new("notes.txt")), FileClass::Unknown);
    }
}
