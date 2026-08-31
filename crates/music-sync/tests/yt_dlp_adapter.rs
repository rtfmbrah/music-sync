//! Deterministic subprocess tests for YouTube source enumeration.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

use music_sync::provider::ProviderFailureKind;
use music_sync::yt_dlp::YtDlp;

static FIXTURE_EXECUTION: Mutex<()> = Mutex::new(());

#[test]
fn passes_explicit_cookie_file_without_reading_it() -> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let executable = directory.path().join("yt-dlp");
    let marker = directory.path().join("args");
    let cookies = directory.path().join("cookies.txt");
    let cache = directory.path().join("cache");
    fs::write(&cookies, "secret fixture")?;
    fs::write(
        &executable,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" >{}\nprintf '%s' '{{\"id\":\"one\",\"webpage_url\":\"https://youtu.be/one\"}}'\n",
            marker.display()
        ),
    )?;
    let mut permissions = fs::metadata(&executable)?.permissions();
    permissions.set_mode(0o700);
    fs::set_permissions(&executable, permissions)?;
    YtDlp::new(executable, Duration::from_secs(1))
        .with_cookie_file(Some(cookies.clone()))
        .with_cache_directory(cache.clone())
        .with_pacing(1, 5, 15)
        .enumerate("https://youtu.be/one")?;
    let arguments = fs::read_to_string(marker)?;
    assert!(arguments.contains("--cookies\n"));
    assert!(arguments.contains(&format!("{}\n", cookies.display())));
    assert!(!arguments.contains("secret fixture"));
    assert!(arguments.contains("--sleep-requests\n1\n"));
    assert!(arguments.contains("--extractor-args\nyoutube:player_client=default,web_embedded\n"));
    assert!(arguments.contains(&format!("--cache-dir\n{}\n", cache.display())));
    assert!(!arguments.contains("--sleep-interval"));
    Ok(())
}

#[test]
fn enumerates_stored_playlist_fixture_through_subprocess() -> Result<(), Box<dyn std::error::Error>>
{
    let _execution = FIXTURE_EXECUTION
        .lock()
        .map_err(|_| "fixture execution lock was poisoned")?;
    let json = include_str!("fixtures/yt-dlp/playlist.json");
    let (_directory, executable) = fixture_script(&format!("printf '%s' '{json}'"))?;

    let snapshot = YtDlp::new(executable, Duration::from_secs(1))
        .enumerate("https://example.invalid/playlist")?;

    assert_eq!(snapshot.provider_collection_id.as_deref(), Some("pl001"));
    assert_eq!(snapshot.items.len(), 2);
    assert_eq!(snapshot.items[0].provider_item_id, "vid001");
    Ok(())
}

#[test]
fn captures_complete_single_item_metadata_without_media_download()
-> Result<(), Box<dyn std::error::Error>> {
    let _execution = FIXTURE_EXECUTION
        .lock()
        .map_err(|_| "fixture execution lock was poisoned")?;
    let json = include_str!("fixtures/yt-dlp/video.json");
    let (directory, executable) = fixture_script(&format!("printf '%s' '{json}'"))?;
    let marker = directory.path().join("args");
    let wrapper = directory.path().join("wrapper");
    fs::write(
        &wrapper,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" >{}\nexec {} \"$@\"\n",
            marker.display(),
            executable.display()
        ),
    )?;
    let mut permissions = fs::metadata(&wrapper)?.permissions();
    permissions.set_mode(0o700);
    fs::set_permissions(&wrapper, permissions)?;
    let metadata =
        YtDlp::new(wrapper, Duration::from_secs(1)).metadata("https://youtu.be/vid001")?;
    assert_eq!(metadata["channel"], "Fixture Artist");
    let arguments = fs::read_to_string(marker)?;
    assert!(arguments.contains("--dump-single-json"));
    assert!(!arguments.contains("--flat-playlist"));
    Ok(())
}

#[test]
fn classifies_private_and_malformed_fixture_results() -> Result<(), Box<dyn std::error::Error>> {
    let _execution = FIXTURE_EXECUTION
        .lock()
        .map_err(|_| "fixture execution lock was poisoned")?;
    let stderr = include_str!("fixtures/yt-dlp/private.stderr").trim();
    let (_directory, executable) =
        fixture_script(&format!("printf '%s' \"{stderr}\" >&2\nexit 1"))?;
    let error = require_error(
        YtDlp::new(executable, Duration::from_secs(1)).enumerate("https://example.invalid/private"),
    )?;
    assert_eq!(error.kind(), ProviderFailureKind::PermanentlyUnavailable);

    let (_directory, executable) = fixture_script("printf '%s' 'not-json'")?;
    let error = require_error(
        YtDlp::new(executable, Duration::from_secs(1))
            .enumerate("https://example.invalid/malformed"),
    )?;
    assert_eq!(error.kind(), ProviderFailureKind::Extraction);
    Ok(())
}

#[test]
fn kills_enumeration_after_deadline() -> Result<(), Box<dyn std::error::Error>> {
    let _execution = FIXTURE_EXECUTION
        .lock()
        .map_err(|_| "fixture execution lock was poisoned")?;
    let (_directory, executable) = fixture_script("exec sleep 2")?;
    let error = require_error(
        YtDlp::new(executable, Duration::from_millis(10)).enumerate("https://example.invalid/slow"),
    )?;
    assert_eq!(error.kind(), ProviderFailureKind::Timeout);
    Ok(())
}

fn require_error(
    result: Result<music_sync::provider::SourceSnapshot, music_sync::yt_dlp::YtDlpError>,
) -> Result<music_sync::yt_dlp::YtDlpError, Box<dyn std::error::Error>> {
    match result {
        Ok(_) => Err("fixture unexpectedly succeeded".into()),
        Err(error) => Ok(error),
    }
}

fn fixture_script(body: &str) -> Result<(tempfile::TempDir, PathBuf), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let staging = directory.path().join("yt-dlp.staging");
    let executable = directory.path().join("yt-dlp");
    fs::write(&staging, format!("#!/bin/sh\n{body}\n"))?;
    let mut permissions = fs::metadata(&staging)?.permissions();
    permissions.set_mode(0o700);
    fs::set_permissions(&staging, permissions)?;
    fs::rename(staging, &executable)?;
    Ok((directory, executable))
}
