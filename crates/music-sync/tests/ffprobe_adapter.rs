//! Deterministic subprocess-fixture tests for the bounded ffprobe adapter.

use std::fs::{self, File};
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::time::Duration;

use music_sync::media_probe::{Ffprobe, MediaProbe, MediaProbeError};

#[test]
fn parses_fixture_subprocess_output() -> Result<(), Box<dyn std::error::Error>> {
    let (_directory, fixture) = fixture_script(
        r#"printf '%s' '{"streams":[{"codec_type":"audio","codec_name":"opus","sample_rate":"48000","channels":2}],"format":{"duration":"5.25","tags":{"title":"Fixture","ISRC":"USABC2412345"}}}'"#,
    )?;
    let probe = Ffprobe::new(fixture, Duration::from_secs(1));

    let properties = probe.probe(Path::new("ignored.opus"))?;

    assert_eq!(properties.codec, "opus");
    assert_eq!(properties.duration_ms, Some(5_250));
    assert!(properties.tags.has_basic_tags);
    assert!(properties.tags.has_canonical_identity());
    Ok(())
}

#[test]
fn classifies_nonzero_exit_as_failure() -> Result<(), Box<dyn std::error::Error>> {
    let (_directory, fixture) = fixture_script("printf 'invalid media' >&2\nexit 7")?;
    let probe = Ffprobe::new(fixture, Duration::from_secs(1));

    let error = match probe.probe(Path::new("ignored.m4a")) {
        Ok(_) => return Err("fixture unexpectedly succeeded".into()),
        Err(error) => error,
    };

    assert!(matches!(
        error,
        MediaProbeError::Failed { code: Some(7), .. }
    ));
    Ok(())
}

#[test]
fn kills_fixture_after_timeout() -> Result<(), Box<dyn std::error::Error>> {
    let (_directory, fixture) = fixture_script("exec sleep 2")?;
    let probe = Ffprobe::new(fixture, Duration::from_millis(10));

    let error = match probe.probe(Path::new("ignored.m4a")) {
        Ok(_) => return Err("fixture unexpectedly succeeded".into()),
        Err(error) => error,
    };

    assert!(matches!(error, MediaProbeError::Timeout(_)));
    Ok(())
}

fn fixture_script(
    body: &str,
) -> Result<(tempfile::TempDir, std::path::PathBuf), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("ffprobe-fixture");
    let staging = directory.path().join("ffprobe-fixture.staging");
    let mut file = File::create(&staging)?;
    file.write_all(format!("#!/bin/sh\n{body}\n").as_bytes())?;
    file.sync_all()?;
    drop(file);
    let mut permissions = fs::metadata(&staging)?.permissions();
    permissions.set_mode(0o700);
    fs::set_permissions(&staging, permissions)?;
    fs::rename(staging, &path)?;
    Ok((directory, path))
}
