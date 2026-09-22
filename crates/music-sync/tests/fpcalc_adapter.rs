//! Deterministic subprocess tests for bounded Chromaprint extraction.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::time::{Duration, Instant};

use music_sync::fingerprint::{CompressedFingerprinter, FingerprintError, Fingerprinter, Fpcalc};

fn executable(directory: &Path, body: &str) -> Result<std::path::PathBuf, std::io::Error> {
    let path = directory.join("fpcalc");
    fs::write(&path, format!("#!/bin/sh\nset -eu\n{body}\n"))?;
    let mut permissions = fs::metadata(&path)?.permissions();
    permissions.set_mode(0o700);
    fs::set_permissions(&path, permissions)?;
    Ok(path)
}

#[test]
fn extracts_compressed_fingerprint_for_external_lookup() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let media = root.path().join("audio.opus");
    fs::write(&media, b"fixture")?;
    let program = executable(
        root.path(),
        "test \"$1\" = -json\ntest \"$2\" = -algorithm\ntest \"$3\" = 2\ntest \"$4\" = -length\ntest \"$5\" = 120\nprintf '%s' '{\"duration\":179.6,\"fingerprint\":\"AQAD_fixture\"}'",
    )?;

    let result =
        Fpcalc::new(program, Duration::from_secs(1), 120).compressed_fingerprint(&media)?;

    assert_eq!(result.duration_seconds, 180);
    assert_eq!(result.fingerprint, "AQAD_fixture");
    Ok(())
}

#[test]
fn extracts_raw_bounded_fingerprint() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let media = root.path().join("audio.opus");
    fs::write(&media, b"fixture")?;
    let program = executable(
        root.path(),
        "test \"$1\" = -raw\ntest \"$2\" = -json\ntest \"$3\" = -algorithm\ntest \"$4\" = 2\ntest \"$5\" = -length\ntest \"$6\" = 60\nprintf '%s' '{\"duration\":2.5,\"fingerprint\":[1,2,3]}'",
    )?;

    let result = Fpcalc::new(program, Duration::from_secs(1), 60).fingerprint(&media)?;

    assert_eq!(result.duration_ms, 2_500);
    assert_eq!(result.values, [1, 2, 3]);
    Ok(())
}

#[test]
fn reports_nonzero_failure() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let media = root.path().join("audio.opus");
    fs::write(&media, b"fixture")?;
    let program = executable(root.path(), "printf '%s' 'decode failed' >&2\nexit 4")?;

    let result = Fpcalc::new(program, Duration::from_secs(1), 60).fingerprint(&media);

    assert!(matches!(
        result,
        Err(FingerprintError::Failed {
            code: Some(4),
            stderr
        }) if stderr == "decode failed"
    ));
    Ok(())
}

#[test]
fn accepts_valid_fingerprint_when_fpcalc_reports_benign_end_of_file()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let media = root.path().join("audio.opus");
    fs::write(&media, b"fixture")?;
    let program = executable(
        root.path(),
        "printf '%s' '{\"duration\":2.5,\"fingerprint\":[1,2,3]}'\nprintf '%s\\n' 'ERROR: Error decoding audio frame (End of file)' >&2\nexit 3",
    )?;

    let result = Fpcalc::new(program, Duration::from_secs(1), 60).fingerprint(&media)?;

    assert_eq!(result.duration_ms, 2_500);
    assert_eq!(result.values, [1, 2, 3]);
    Ok(())
}

#[test]
fn still_validates_output_after_benign_end_of_file() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let media = root.path().join("audio.opus");
    fs::write(&media, b"fixture")?;
    let program = executable(
        root.path(),
        "printf '%s' 'not json'\nprintf '%s\\n' 'ERROR: Error decoding audio frame (End of file)' >&2\nexit 3",
    )?;

    let result = Fpcalc::new(program, Duration::from_secs(1), 60).fingerprint(&media);

    assert!(matches!(result, Err(FingerprintError::Json(_))));
    Ok(())
}

#[test]
fn rejects_benign_end_of_file_without_output() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let media = root.path().join("audio.opus");
    fs::write(&media, b"fixture")?;
    let program = executable(
        root.path(),
        "printf '%s\\n' 'ERROR: Error decoding audio frame (End of file)' >&2\nexit 3",
    )?;

    let result = Fpcalc::new(program, Duration::from_secs(1), 60).fingerprint(&media);

    assert!(matches!(
        result,
        Err(FingerprintError::Failed {
            code: Some(3),
            stderr
        }) if stderr == "ERROR: Error decoding audio frame (End of file)"
    ));
    Ok(())
}

#[test]
fn kills_subprocess_after_deadline() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let media = root.path().join("audio.opus");
    fs::write(&media, b"fixture")?;
    let program = executable(root.path(), "exec sleep 5")?;
    let started = Instant::now();

    let result = Fpcalc::new(program, Duration::from_millis(30), 60).fingerprint(&media);

    assert!(matches!(result, Err(FingerprintError::Timeout(_))));
    assert!(started.elapsed() < Duration::from_secs(2));
    Ok(())
}
