//! Deterministic subprocess tests for bounded yt-dlp acquisition staging.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

use music_sync::provider::ProviderFailureKind;
use music_sync::yt_dlp::{YtDlp, YtDlpError};

static FIXTURE_EXECUTION: Mutex<()> = Mutex::new(());

#[test]
fn downloads_one_nonempty_file_directly_inside_staging() -> Result<(), Box<dyn std::error::Error>> {
    let _execution = fixture_lock()?;
    let (_tools, executable) = fixture_script(
        "while test \"$1\" != '--paths'; do shift; done\n\
         staging=$2\n\
         printf '%s' 'fixture audio' > \"$staging/media.opus\"\n\
         printf '%s\\n' \"$staging/media.opus\"",
    )?;
    let staging = tempfile::tempdir()?;

    let media = YtDlp::new(executable, Duration::from_secs(1))
        .download("https://youtu.be/fixture", staging.path())?;

    assert_eq!(
        media.path,
        staging.path().canonicalize()?.join("media.opus")
    );
    assert_eq!(media.bytes, 13);
    assert_eq!(fs::read(media.path)?, b"fixture audio");
    Ok(())
}

#[test]
fn passes_request_and_randomized_download_pacing() -> Result<(), Box<dyn std::error::Error>> {
    let _execution = fixture_lock()?;
    let tools = tempfile::tempdir()?;
    let executable = tools.path().join("yt-dlp");
    let marker = tools.path().join("args");
    fs::write(
        &executable,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" >{}\nwhile test \"$1\" != '--paths'; do shift; done\nstaging=$2\nprintf audio >\"$staging/media.opus\"\nprintf '%s\\n' \"$staging/media.opus\"\n",
            marker.display()
        ),
    )?;
    let mut permissions = fs::metadata(&executable)?.permissions();
    permissions.set_mode(0o700);
    fs::set_permissions(&executable, permissions)?;
    let staging = tempfile::tempdir()?;
    let cache = tools.path().join("cache");

    YtDlp::new(executable, Duration::from_secs(1))
        .with_cache_directory(cache.clone())
        .with_pacing(1, 5, 15)
        .download("https://youtu.be/fixture", staging.path())?;
    let arguments = fs::read_to_string(marker)?;
    assert!(arguments.contains("--sleep-requests\n1\n"));
    assert!(arguments.contains("--sleep-interval\n5\n"));
    assert!(arguments.contains("--max-sleep-interval\n15\n"));
    assert!(arguments.contains("--extractor-args\nyoutube:player_client=default,web_embedded\n"));
    assert!(arguments.contains(&format!("--cache-dir\n{}\n", cache.display())));
    assert!(arguments.contains("--format\nbestaudio\n"));
    assert!(arguments.contains("--remux-video\nwebm>opus\n"));
    Ok(())
}

#[test]
fn classifies_provider_failure_without_accepting_media() -> Result<(), Box<dyn std::error::Error>> {
    let _execution = fixture_lock()?;
    let (_tools, executable) =
        fixture_script("printf '%s' 'ERROR: HTTP 429 Too Many Requests' >&2\nexit 1")?;
    let staging = tempfile::tempdir()?;

    let error = require_error(
        YtDlp::new(executable, Duration::from_secs(1))
            .download("https://youtu.be/fixture", staging.path()),
    )?;

    assert_eq!(error.kind(), ProviderFailureKind::RateLimited);
    assert!(fs::read_dir(staging.path())?.next().is_none());
    Ok(())
}

#[test]
fn rejects_missing_and_relative_download_output() -> Result<(), Box<dyn std::error::Error>> {
    let _execution = fixture_lock()?;
    let staging = tempfile::tempdir()?;
    let (_tools, executable) = fixture_script("true")?;
    let missing = require_error(
        YtDlp::new(executable, Duration::from_secs(1))
            .download("https://youtu.be/fixture", staging.path()),
    )?;
    assert!(matches!(missing, YtDlpError::DownloadOutputCount(0)));

    let (_tools, executable) = fixture_script("printf '%s\\n' 'media.opus'")?;
    let relative = require_error(
        YtDlp::new(executable, Duration::from_secs(1))
            .download("https://youtu.be/fixture", staging.path()),
    )?;
    assert!(matches!(relative, YtDlpError::DownloadPathNotAbsolute(_)));
    Ok(())
}

#[test]
fn rejects_path_escape_and_empty_media() -> Result<(), Box<dyn std::error::Error>> {
    let _execution = fixture_lock()?;
    let staging = tempfile::tempdir()?;
    let (_tools, executable) = fixture_script(
        "while test \"$1\" != '--paths'; do shift; done\n\
         outside=$(dirname \"$2\")/escaped.opus\n\
         printf '%s' 'outside' > \"$outside\"\n\
         printf '%s\\n' \"$outside\"",
    )?;
    let escaped = require_error(
        YtDlp::new(executable, Duration::from_secs(1))
            .download("https://youtu.be/fixture", staging.path()),
    )?;
    assert!(matches!(escaped, YtDlpError::DownloadPathEscaped { .. }));

    let (_tools, executable) = fixture_script(
        "while test \"$1\" != '--paths'; do shift; done\n\
         : > \"$2/media.opus\"\n\
         printf '%s\\n' \"$2/media.opus\"",
    )?;
    let empty = require_error(
        YtDlp::new(executable, Duration::from_secs(1))
            .download("https://youtu.be/fixture", staging.path()),
    )?;
    assert!(matches!(empty, YtDlpError::DownloadEmpty(_)));
    Ok(())
}

#[test]
fn kills_download_after_deadline() -> Result<(), Box<dyn std::error::Error>> {
    let _execution = fixture_lock()?;
    let (_tools, executable) = fixture_script("exec sleep 2")?;
    let staging = tempfile::tempdir()?;

    let error = require_error(
        YtDlp::new(executable, Duration::from_millis(10))
            .download("https://youtu.be/fixture", staging.path()),
    )?;

    assert_eq!(error.kind(), ProviderFailureKind::Timeout);
    Ok(())
}

fn fixture_lock() -> Result<std::sync::MutexGuard<'static, ()>, Box<dyn std::error::Error>> {
    FIXTURE_EXECUTION
        .lock()
        .map_err(|_| "fixture execution lock was poisoned".into())
}

fn require_error(
    result: Result<music_sync::yt_dlp::DownloadedMedia, YtDlpError>,
) -> Result<YtDlpError, Box<dyn std::error::Error>> {
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
