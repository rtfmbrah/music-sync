//! Filesystem integration tests for read-only generic library adoption.

use std::fs;
use std::os::unix::fs::symlink;

use std::collections::BTreeMap;
use std::num::NonZeroUsize;
use std::path::Path;

use music_sync::adoption::{
    AdoptionIssueKind, HashLimit, ProbeLimit, apply_library, scan_library, scan_library_with_hash,
    scan_library_with_probe, scan_library_with_probe_and_hash,
};
use music_sync::content_hash::{ContentHash, ContentHashError, ContentHasher, Sha256FileHasher};
use music_sync::media_probe::{
    CanonicalTagStatus, MediaProbe, MediaProbeError, MediaProperties, MediaTagPresence,
};
use music_sync::persistence::Database;

#[test]
fn scan_classifies_files_without_changing_them() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let album = root.path().join("Album");
    fs::create_dir(&album)?;
    fs::write(album.join("track.m4a"), b"audio fixture")?;
    fs::write(album.join("empty.mp3"), b"")?;
    fs::write(album.join("track.lrc"), b"[00:01.00]Lyrics")?;
    fs::write(album.join("cover.JPG"), b"artwork fixture")?;
    fs::write(root.path().join("playlist.M3U8"), b"Album/track.m4a\n")?;
    fs::write(root.path().join("notes.txt"), b"preserve this")?;
    symlink(album.join("track.m4a"), root.path().join("track-link"))?;

    let report = scan_library(root.path())?;

    assert_eq!(report.directories_scanned, 2);
    assert_eq!(report.files_scanned, 6);
    assert_eq!(report.media_files, 1);
    assert_eq!(report.corrupt_media, 1);
    assert_eq!(report.lyric_files, 1);
    assert_eq!(report.artwork_files, 1);
    assert_eq!(report.playlist_files, 1);
    assert_eq!(report.unknown_files, 1);
    assert_eq!(report.symbolic_links, 1);
    assert_eq!(report.unreadable_paths, 0);
    assert_eq!(report.relationships.lyrics_associated, 1);
    assert_eq!(report.relationships.artwork_beside_media, 1);
    assert_eq!(report.relationships.playlist_entries, 1);
    assert_eq!(report.relationships.playlist_references_resolved, 1);
    assert_eq!(report.issues.len(), 1);
    assert_eq!(report.issues[0].kind, AdoptionIssueKind::EmptyMedia);
    assert_eq!(report.effects().files_modified, 0);
    assert_eq!(report.effects().files_deleted, 0);
    assert_eq!(report.effects().files_downloaded, 0);
    assert_eq!(fs::read(album.join("track.m4a"))?, b"audio fixture");
    assert_eq!(fs::read(root.path().join("notes.txt"))?, b"preserve this");
    Ok(())
}

#[test]
fn scan_reports_orphaned_sidecars_and_playlist_reference_classes()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let album = root.path().join("Album");
    fs::create_dir(&album)?;
    fs::write(album.join("track.flac"), b"audio")?;
    fs::write(root.path().join("orphan.lrc"), b"lyrics")?;
    fs::write(root.path().join("cover.png"), b"art")?;
    let playlist =
        b"#EXTM3U\nAlbum/track.flac\nmissing.mp3\nhttps://example.test/stream\n../outside.mp3\n";
    fs::write(root.path().join("mixed.m3u8"), playlist)?;
    fs::write(root.path().join("invalid.m3u"), [0xff, 0xfe])?;

    let report = scan_library(root.path())?;

    assert_eq!(report.relationships.lyrics_orphaned, 1);
    assert_eq!(report.relationships.artwork_orphaned, 1);
    assert_eq!(report.relationships.playlist_entries, 4);
    assert_eq!(report.relationships.playlist_references_resolved, 1);
    assert_eq!(report.relationships.playlist_references_missing, 1);
    assert_eq!(report.relationships.playlist_references_external, 2);
    assert_eq!(report.relationships.playlist_files_invalid, 1);
    assert_eq!(
        report
            .issues
            .iter()
            .filter(|issue| issue.kind == AdoptionIssueKind::ExternalPlaylistReference)
            .count(),
        2
    );
    assert_eq!(fs::read(root.path().join("mixed.m3u8"))?, playlist);
    Ok(())
}

#[test]
fn scan_rejects_a_file_as_the_root() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let file = root.path().join("track.m4a");
    fs::write(&file, b"audio")?;
    assert!(scan_library(&file).is_err());
    Ok(())
}

#[test]
fn apply_registers_media_idempotently_without_changing_library_files()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::write(root.path().join("track.flac"), b"preserved audio")?;
    fs::write(root.path().join("unknown.bin"), b"preserved unknown")?;
    let mut database = Database::open_in_memory()?;

    let first = apply_library(root.path(), &mut database)?;
    let second = apply_library(root.path(), &mut database)?;

    assert_eq!(first.database.artifacts_inserted, 1);
    assert_eq!(first.database.recordings_inserted, 1);
    assert_eq!(second.database.artifacts_inserted, 0);
    assert_eq!(second.database.artifacts_existing, 1);
    assert_eq!(first.scan.effects().files_modified, 0);
    assert_eq!(
        fs::read(root.path().join("track.flac"))?,
        b"preserved audio"
    );
    assert_eq!(
        fs::read(root.path().join("unknown.bin"))?,
        b"preserved unknown"
    );
    Ok(())
}

#[test]
fn apply_rejects_relative_library_roots() -> Result<(), Box<dyn std::error::Error>> {
    let mut database = Database::open_in_memory()?;
    assert!(apply_library(Path::new("relative-library"), &mut database).is_err());
    Ok(())
}

#[test]
fn bounded_probe_aggregates_properties_and_skips_remaining_media()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::write(root.path().join("a.m4a"), b"audio a")?;
    fs::write(root.path().join("b.flac"), b"audio b")?;
    let probe = FixedProbe;

    let report = scan_library_with_probe(
        root.path(),
        &probe,
        ProbeLimit::Files(NonZeroUsize::new(1).ok_or("non-zero fixture limit")?),
    )?;
    let summary = report.probe.ok_or("missing probe summary")?;

    assert_eq!(summary.attempted, 1);
    assert_eq!(summary.succeeded, 1);
    assert_eq!(summary.failed, 0);
    assert_eq!(summary.skipped_due_to_limit, 1);
    assert_eq!(summary.total_duration_ms, 90_000);
    assert_eq!(summary.with_embedded_artwork, 1);
    assert_eq!(summary.with_basic_tags, 1);
    assert_eq!(summary.with_canonical_identity, 1);
    assert_eq!(summary.with_valid_musicbrainz_recording_id, 0);
    assert_eq!(summary.with_valid_isrc, 1);
    assert_eq!(summary.with_malformed_canonical_tag, 0);
    assert_eq!(summary.codecs, BTreeMap::from([("aac".into(), 1)]));
    Ok(())
}

#[test]
fn bounded_hashing_reports_exact_duplicates_without_changing_files()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::write(root.path().join("a.flac"), b"same bytes")?;
    fs::write(root.path().join("b.flac"), b"same bytes")?;
    fs::write(root.path().join("c.flac"), b"different")?;

    let report = scan_library_with_hash(root.path(), &Sha256FileHasher, HashLimit::All)?;
    let summary = report.hash.ok_or("missing hash summary")?;

    assert_eq!(summary.attempted, 3);
    assert_eq!(summary.succeeded, 3);
    assert_eq!(summary.failed, 0);
    assert_eq!(summary.skipped_due_to_limit, 0);
    assert_eq!(summary.bytes_hashed, 29);
    assert_eq!(summary.exact_duplicate_groups, 1);
    assert_eq!(summary.exact_duplicate_files, 2);
    assert_eq!(fs::read(root.path().join("a.flac"))?, b"same bytes");
    Ok(())
}

#[test]
fn hash_limit_uses_deterministic_media_path_order() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::write(root.path().join("a.flac"), b"a")?;
    fs::write(root.path().join("b.flac"), b"bb")?;

    let report = scan_library_with_hash(
        root.path(),
        &Sha256FileHasher,
        HashLimit::Files(NonZeroUsize::new(1).ok_or("non-zero fixture limit")?),
    )?;
    let summary = report.hash.ok_or("missing hash summary")?;

    assert_eq!(summary.attempted, 1);
    assert_eq!(summary.bytes_hashed, 1);
    assert_eq!(summary.skipped_due_to_limit, 1);
    Ok(())
}

#[test]
fn combined_probe_and_hash_populates_independent_summaries()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::write(root.path().join("track.m4a"), b"audio")?;

    let report = scan_library_with_probe_and_hash(
        root.path(),
        &FixedProbe,
        ProbeLimit::All,
        &Sha256FileHasher,
        HashLimit::All,
    )?;

    assert_eq!(report.probe.ok_or("missing probe summary")?.succeeded, 1);
    assert_eq!(report.hash.ok_or("missing hash summary")?.succeeded, 1);
    Ok(())
}

#[test]
fn hash_failure_is_reported_without_mutation() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let path = root.path().join("track.flac");
    fs::write(&path, b"preserved")?;

    let report = scan_library_with_hash(root.path(), &FailingHasher, HashLimit::All)?;
    let summary = report.hash.ok_or("missing hash summary")?;

    assert_eq!(summary.failed, 1);
    assert!(
        report
            .issues
            .iter()
            .any(|issue| issue.kind == AdoptionIssueKind::HashFailed)
    );
    assert_eq!(fs::read(path)?, b"preserved");
    Ok(())
}

struct FixedProbe;

struct FailingHasher;

impl ContentHasher for FailingHasher {
    fn hash(&self, path: &Path) -> Result<ContentHash, ContentHashError> {
        Err(ContentHashError::Read {
            path: path.to_path_buf(),
            source: std::io::Error::other("fixture read failure"),
        })
    }
}

impl MediaProbe for FixedProbe {
    fn probe(&self, _path: &Path) -> Result<MediaProperties, MediaProbeError> {
        Ok(MediaProperties {
            codec: "aac".into(),
            duration_ms: Some(90_000),
            sample_rate_hz: Some(44_100),
            channels: Some(2),
            has_embedded_artwork: true,
            tags: MediaTagPresence {
                has_basic_tags: true,
                musicbrainz_recording_id: CanonicalTagStatus::Absent,
                isrc: CanonicalTagStatus::Valid,
            },
        })
    }
}
