//! Black-box tests for truthful command-line behavior.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::process::Command;

#[test]
fn discovery_run_and_route_require_canonical_relationship_evidence()
-> Result<(), Box<dyn std::error::Error>> {
    use std::io::{Read, Write};
    use std::net::TcpListener;

    let root = tempfile::tempdir()?;
    let library = root.path().join("library");
    fs::create_dir(&library)?;
    let config = root.path().join("config.toml");
    fs::write(
        &config,
        format!(
            "state_directory = {:?}\nlibrary_directory = {:?}\nplaylist_directory = {:?}\n[discovery]\nenabled = true\ntarget_new_tracks_per_day = 1\nmax_new_tracks_per_day = 1\nmax_tracks_per_artist_per_day = 1\nexploration_ratio = 0.0\nwildcard_ratio = 0.0\nminimum_free_disk_gb = 0\nlistenbrainz_user = \"fixture\"\n",
            root.path().join("state"),
            library,
            root.path().join("playlists")
        ),
    )?;
    let listen = TcpListener::bind("127.0.0.1:0")?;
    let listen_url = format!("http://{}", listen.local_addr()?);
    let listen_thread = std::thread::spawn(move || {
        if let Ok((mut stream, _)) = listen.accept() {
            let mut request = [0_u8; 4096];
            let _ = stream.read(&mut request);
            let body = r#"{"payload":{"mbids":[{"recording_mbid":"11111111-2222-3333-4444-555555555555","score":1.0}]}}"#;
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
        }
    });
    let discovery = Command::new(env!("CARGO_BIN_EXE_music-sync"))
        .args(["--json", "discovery", "run", "--config"])
        .arg(&config)
        .args([
            "--user-agent",
            "fixture@example.invalid",
            "--listenbrainz-endpoint",
            &listen_url,
        ])
        .output()?;
    listen_thread
        .join()
        .map_err(|_| "ListenBrainz fixture thread panicked")?;
    assert!(
        discovery.status.success(),
        "{}",
        String::from_utf8_lossy(&discovery.stderr)
    );

    let musicbrainz = TcpListener::bind("127.0.0.1:0")?;
    let musicbrainz_url = format!("http://{}", musicbrainz.local_addr()?);
    let mb_thread = std::thread::spawn(move || {
        if let Ok((mut stream, _)) = musicbrainz.accept() {
            let mut request = [0_u8; 4096];
            let _ = stream.read(&mut request);
            let body = r#"{"id":"11111111-2222-3333-4444-555555555555","title":"Exact","length":180000,"isrcs":["USABC2412345"],"relations":[{"type":"video","target-type":"url","url":{"resource":"https://youtu.be/Exact_video"}}]}"#;
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
        }
    });
    let route = Command::new(env!("CARGO_BIN_EXE_music-sync"))
        .args(["--json", "discovery", "route", "--config"])
        .arg(&config)
        .args([
            "--user-agent",
            "fixture@example.invalid",
            "--endpoint",
            &musicbrainz_url,
        ])
        .output()?;
    mb_thread
        .join()
        .map_err(|_| "MusicBrainz fixture thread panicked")?;
    assert!(
        route.status.success(),
        "{}",
        String::from_utf8_lossy(&route.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&route.stdout)?;
    assert_eq!(report["queued"], 1);
    let connection = rusqlite::Connection::open(root.path().join("state/music-sync.sqlite3"))?;
    assert_eq!(
        connection.query_row(
            "SELECT COUNT(*) FROM discovery_acquisition_assertions",
            [],
            |row| row.get::<_, u64>(0)
        )?,
        1
    );
    assert_eq!(
        connection.query_row(
            "SELECT COUNT(*) FROM jobs WHERE kind='acquire' AND status='pending'",
            [],
            |row| row.get::<_, u64>(0)
        )?,
        1
    );
    Ok(())
}

#[test]
fn help_describes_only_implemented_commands() -> Result<(), Box<dyn std::error::Error>> {
    let output = Command::new(env!("CARGO_BIN_EXE_music-sync"))
        .arg("--help")
        .output()?;
    let stdout = String::from_utf8(output.stdout)?;
    assert!(output.status.success());
    assert!(stdout.contains("doctor"));
    assert!(stdout.contains("library"));
    assert!(!stdout.contains("sync\n"));
    Ok(())
}

#[test]
fn library_adopt_reports_zero_effects_as_json() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::write(root.path().join("track.m4a"), b"audio")?;
    fs::write(root.path().join("keep.bin"), b"unknown")?;

    let output = Command::new(env!("CARGO_BIN_EXE_music-sync"))
        .args(["--json", "library", "adopt"])
        .arg(root.path())
        .output()?;
    let report: serde_json::Value = serde_json::from_slice(&output.stdout)?;

    assert!(output.status.success());
    assert_eq!(report["media_files"], 1);
    assert_eq!(report["unknown_files"], 1);
    assert_eq!(report["effects"]["files_modified"], 0);
    assert_eq!(report["effects"]["files_deleted"], 0);
    assert_eq!(report["effects"]["files_downloaded"], 0);
    assert_eq!(fs::read(root.path().join("keep.bin"))?, b"unknown");
    Ok(())
}

#[test]
fn doctor_emits_machine_readable_offline_report() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let library = root.path().join("library");
    fs::create_dir(&library)?;
    let config_path = root.path().join("config.toml");
    fs::write(
        &config_path,
        format!(
            "state_directory = {:?}\nlibrary_directory = {:?}\nplaylist_directory = {:?}\n",
            root.path().join("state"),
            library,
            root.path().join("playlists")
        ),
    )?;

    let output = Command::new(env!("CARGO_BIN_EXE_music-sync"))
        .args(["--json", "doctor", "--config"])
        .arg(config_path)
        .output()?;
    let report: serde_json::Value = serde_json::from_slice(&output.stdout)?;

    assert!(output.status.success());
    assert!(report["checks"].is_array());
    assert!(
        report["checks"]
            .as_array()
            .is_some_and(|checks| { checks.iter().any(|check| check["name"] == "database") })
    );
    Ok(())
}

#[test]
fn library_adopt_can_hash_without_claiming_filesystem_effects()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::write(root.path().join("a.m4a"), b"same")?;
    fs::write(root.path().join("b.m4a"), b"same")?;

    let output = Command::new(env!("CARGO_BIN_EXE_music-sync"))
        .args(["--json", "library", "adopt"])
        .arg(root.path())
        .arg("--hash")
        .output()?;
    let report: serde_json::Value = serde_json::from_slice(&output.stdout)?;

    assert!(output.status.success());
    assert_eq!(report["hash"]["succeeded"], 2);
    assert_eq!(report["hash"]["exact_duplicate_groups"], 1);
    assert_eq!(report["effects"]["files_modified"], 0);
    Ok(())
}

#[test]
fn library_adopt_reports_valid_and_malformed_canonical_tags()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::write(root.path().join("track.m4a"), b"audio")?;
    let tools = tempfile::tempdir()?;
    let ffprobe = tools.path().join("ffprobe");
    fs::write(
        &ffprobe,
        "#!/bin/sh\nprintf '%s' '{\"streams\":[{\"codec_type\":\"audio\",\"codec_name\":\"aac\"}],\"format\":{\"tags\":{\"ISRC\":\"USABC2412345\",\"MusicBrainz_Recording_Id\":\"bad\"}}}'\n",
    )?;
    let mut permissions = fs::metadata(&ffprobe)?.permissions();
    permissions.set_mode(0o700);
    fs::set_permissions(&ffprobe, permissions)?;

    let output = Command::new(env!("CARGO_BIN_EXE_music-sync"))
        .args(["--json", "library", "adopt"])
        .arg(root.path())
        .arg("--probe")
        .env("PATH", tools.path())
        .output()?;
    let report: serde_json::Value = serde_json::from_slice(&output.stdout)?;

    assert!(output.status.success());
    assert_eq!(report["probe"]["with_canonical_identity"], 1);
    assert_eq!(report["probe"]["with_valid_musicbrainz_recording_id"], 0);
    assert_eq!(report["probe"]["with_valid_isrc"], 1);
    assert_eq!(report["probe"]["with_malformed_canonical_tag"], 1);
    Ok(())
}

#[test]
fn library_adopt_apply_is_explicit_and_idempotent() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::write(root.path().join("track.m4a"), b"preserved")?;
    let state = tempfile::tempdir()?;
    let database = state.path().join("state.sqlite3");

    let run = || {
        Command::new(env!("CARGO_BIN_EXE_music-sync"))
            .args(["--json", "library", "adopt"])
            .arg(root.path())
            .args(["--apply", "--database"])
            .arg(&database)
            .output()
    };
    let first = run()?;
    let second = run()?;
    let first_report: serde_json::Value = serde_json::from_slice(&first.stdout)?;
    let second_report: serde_json::Value = serde_json::from_slice(&second.stdout)?;

    assert!(first.status.success());
    assert!(second.status.success());
    assert_eq!(first_report["database"]["artifacts_inserted"], 1);
    assert_eq!(second_report["database"]["artifacts_inserted"], 0);
    assert_eq!(second_report["database"]["artifacts_existing"], 1);
    assert_eq!(first_report["scan"]["effects"]["files_modified"], 0);
    assert_eq!(fs::read(root.path().join("track.m4a"))?, b"preserved");
    Ok(())
}

#[test]
fn library_adopt_apply_requires_a_database() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let output = Command::new(env!("CARGO_BIN_EXE_music-sync"))
        .args(["library", "adopt"])
        .arg(root.path())
        .arg("--apply")
        .output()?;

    assert!(!output.status.success());
    assert!(String::from_utf8(output.stderr)?.contains("--database"));
    Ok(())
}

#[test]
fn source_enumerate_emits_provider_snapshot_as_json() -> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let executable = directory.path().join("yt-dlp");
    fs::write(
        &executable,
        "#!/bin/sh\nprintf '%s' '{\"id\":\"video1\",\"webpage_url\":\"https://www.youtube.com/watch?v=video1\",\"title\":\"Fixture\"}'\n",
    )?;
    let mut permissions = fs::metadata(&executable)?.permissions();
    permissions.set_mode(0o700);
    fs::set_permissions(&executable, permissions)?;

    let output = Command::new(env!("CARGO_BIN_EXE_music-sync"))
        .args([
            "--json",
            "source",
            "enumerate",
            "https://example.invalid/video",
            "--yt-dlp",
        ])
        .arg(executable)
        .output()?;
    let snapshot: serde_json::Value = serde_json::from_slice(&output.stdout)?;

    assert!(output.status.success());
    assert_eq!(snapshot["provider"], "youtube");
    assert_eq!(snapshot["items"][0]["provider_item_id"], "video1");
    Ok(())
}

#[test]
fn source_management_is_persistent_idempotent_and_non_destructive()
-> Result<(), Box<dyn std::error::Error>> {
    let state = tempfile::tempdir()?;
    let database = state.path().join("state.sqlite3");
    let url = "https://www.youtube.com/playlist?list=fixture";
    let command = |arguments: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_music-sync"))
            .arg("--json")
            .args(arguments)
            .arg("--database")
            .arg(&database)
            .output()
    };

    let first = command(&["source", "add", url, "--name", "Fixture"])?;
    let repeated = command(&["source", "add", url])?;
    let first_json: serde_json::Value = serde_json::from_slice(&first.stdout)?;
    let repeated_json: serde_json::Value = serde_json::from_slice(&repeated.stdout)?;
    assert!(first.status.success());
    assert_eq!(first_json["inserted"], true);
    assert_eq!(repeated_json["inserted"], false);
    assert_eq!(repeated_json["id"], first_json["id"]);

    let list = command(&["source", "list"])?;
    let sources: serde_json::Value = serde_json::from_slice(&list.stdout)?;
    assert_eq!(sources.as_array().map(Vec::len), Some(1));
    assert_eq!(sources[0]["name"], "Fixture");

    let id = first_json["id"].as_i64().ok_or("missing source ID")?;
    let id_string = id.to_string();
    let removed = command(&["source", "remove", &id_string])?;
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&removed.stdout)?["deactivated"],
        true
    );
    let active = command(&["source", "list"])?;
    assert!(
        serde_json::from_slice::<serde_json::Value>(&active.stdout)?
            .as_array()
            .is_some_and(Vec::is_empty)
    );
    let all = command(&["source", "list", "--all"])?;
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&all.stdout)?[0]["active"],
        false
    );

    let reactivated = command(&["source", "add", url])?;
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&reactivated.stdout)?["reactivated"],
        true
    );
    Ok(())
}

#[test]
fn source_reconcile_is_idempotent_and_provider_failure_preserves_membership()
-> Result<(), Box<dyn std::error::Error>> {
    let state = tempfile::tempdir()?;
    let database = state.path().join("state.sqlite3");
    let tools = tempfile::tempdir()?;
    let yt_dlp = tools.path().join("yt-dlp");
    let full_snapshot = "#!/bin/sh\nprintf '%s' '{\"id\":\"fixture-playlist\",\"title\":\"Fixture\",\"entries\":[{\"id\":\"one\",\"url\":\"https://www.youtube.com/watch?v=one\"},{\"id\":\"two\",\"url\":\"https://www.youtube.com/watch?v=two\"}]}'\n";
    fs::write(&yt_dlp, full_snapshot)?;
    let mut permissions = fs::metadata(&yt_dlp)?.permissions();
    permissions.set_mode(0o700);
    fs::set_permissions(&yt_dlp, permissions)?;

    let run = |arguments: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_music-sync"))
            .arg("--json")
            .args(arguments)
            .arg("--database")
            .arg(&database)
            .output()
    };
    let url = "https://www.youtube.com/playlist?list=fixture-playlist";
    let added = run(&["source", "add", url])?;
    let added_json: serde_json::Value = serde_json::from_slice(&added.stdout)?;
    let id = added_json["id"].as_i64().ok_or("missing source ID")?;
    let id = id.to_string();
    let executable = yt_dlp.to_str().ok_or("non-Unicode fixture path")?;

    let first = run(&["source", "reconcile", &id, "--yt-dlp", executable])?;
    let repeated = run(&["source", "reconcile", &id, "--yt-dlp", executable])?;
    let first: serde_json::Value = serde_json::from_slice(&first.stdout)?;
    let repeated: serde_json::Value = serde_json::from_slice(&repeated.stdout)?;
    assert_eq!(first["provider_items_inserted"], 2);
    assert_eq!(first["memberships_activated"], 2);
    assert_eq!(first["jobs_created"], 2);
    assert_eq!(repeated["memberships_unchanged"], 2);
    assert_eq!(repeated["jobs_created"], 0);

    fs::write(
        &yt_dlp,
        "#!/bin/sh\nprintf '%s' 'ERROR: temporary network failure' >&2\nexit 1\n",
    )?;
    let failed = run(&["source", "reconcile", &id, "--yt-dlp", executable])?;
    assert!(!failed.status.success());

    fs::write(
        &yt_dlp,
        "#!/bin/sh\nprintf '%s' '{\"id\":\"fixture-playlist\",\"title\":\"Fixture\",\"entries\":[{\"id\":\"two\",\"url\":\"https://www.youtube.com/watch?v=two\"}]}'\n",
    )?;
    let removal = run(&["source", "reconcile", &id, "--yt-dlp", executable])?;
    let removal: serde_json::Value = serde_json::from_slice(&removal.stdout)?;
    assert_eq!(removal["memberships_deactivated"], 1);
    assert_eq!(removal["memberships_unchanged"], 1);
    assert_eq!(removal["jobs_created"], 0);
    Ok(())
}

#[test]
fn source_add_rejects_non_youtube_urls_before_database_creation()
-> Result<(), Box<dyn std::error::Error>> {
    let state = tempfile::tempdir()?;
    let database = state.path().join("state.sqlite3");
    let output = Command::new(env!("CARGO_BIN_EXE_music-sync"))
        .args(["source", "add", "https://example.com/video", "--database"])
        .arg(&database)
        .output()?;

    assert!(!output.status.success());
    assert!(!database.exists());
    Ok(())
}

#[test]
fn acquisition_run_one_defers_failure_then_commits_and_becomes_idle()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let state = root.path().join("state");
    let library = root.path().join("library");
    let playlists = root.path().join("playlists");
    fs::create_dir_all(&state)?;
    fs::create_dir_all(&library)?;
    fs::create_dir_all(&playlists)?;
    let database = state.join("music-sync.sqlite3");
    let config = root.path().join("music-sync.toml");
    fs::write(
        &config,
        format!(
            "state_directory = {:?}\nlibrary_directory = {:?}\nplaylist_directory = {:?}\n",
            state, library, playlists
        ),
    )?;
    let yt_dlp = root.path().join("yt-dlp");
    let ffprobe = root.path().join("ffprobe");
    let enumeration = "{\"id\":\"video1\",\"webpage_url\":\"https://www.youtube.com/watch?v=video1\",\"title\":\"Fixture\"}";
    fs::write(
        &yt_dlp,
        format!(
            "#!/bin/sh\ncase \" $* \" in *' --dump-single-json '*) printf '%s' '{enumeration}' ;; *) printf '%s' 'temporary failure' >&2; exit 1 ;; esac\n"
        ),
    )?;
    fs::write(
        &ffprobe,
        "#!/bin/sh\nprintf '%s' '{\"streams\":[{\"codec_type\":\"audio\",\"codec_name\":\"opus\",\"sample_rate\":\"48000\",\"channels\":2}],\"format\":{\"duration\":\"1.5\"}}'\n",
    )?;
    for executable in [&yt_dlp, &ffprobe] {
        let mut permissions = fs::metadata(executable)?.permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(executable, permissions)?;
    }

    let add = Command::new(env!("CARGO_BIN_EXE_music-sync"))
        .args([
            "source",
            "add",
            "https://www.youtube.com/watch?v=video1",
            "--database",
        ])
        .arg(&database)
        .output()?;
    assert!(add.status.success());
    let reconcile = Command::new(env!("CARGO_BIN_EXE_music-sync"))
        .args(["source", "reconcile", "1", "--database"])
        .arg(&database)
        .arg("--yt-dlp")
        .arg(&yt_dlp)
        .output()?;
    assert!(reconcile.status.success());
    let run = || {
        Command::new(env!("CARGO_BIN_EXE_music-sync"))
            .args(["--json", "acquisition", "run-one", "--config"])
            .arg(&config)
            .arg("--yt-dlp")
            .arg(&yt_dlp)
            .arg("--ffprobe")
            .arg(&ffprobe)
            .output()
    };

    let failed = run()?;
    assert!(!failed.status.success());
    assert!(!library.join("youtube/video1.opus").exists());

    let retry = Command::new(env!("CARGO_BIN_EXE_music-sync"))
        .args(["--json", "acquisition", "retry", "1", "--config"])
        .arg(&config)
        .output()?;
    let retry_json: serde_json::Value = serde_json::from_slice(&retry.stdout)?;
    assert!(retry.status.success());
    assert_eq!(retry_json["released"], true);

    fs::write(
        &yt_dlp,
        format!(
            "#!/bin/sh\ncase \" $* \" in *' --dump-single-json '*) printf '%s' '{enumeration}' ;; *) while test \"$1\" != '--paths'; do shift; done; printf '%s' 'fixture audio' > \"$2/media.opus\"; printf '%s\\n' \"$2/media.opus\" ;; esac\n"
        ),
    )?;
    let committed = run()?;
    let committed_json: serde_json::Value = serde_json::from_slice(&committed.stdout)?;
    assert!(committed.status.success());
    assert_eq!(committed_json["status"], "committed");
    assert_eq!(committed_json["work"]["attempt"], 2);
    assert_eq!(
        fs::read(library.join("youtube/video1.opus"))?,
        b"fixture audio"
    );

    let idle = run()?;
    let idle_json: serde_json::Value = serde_json::from_slice(&idle.stdout)?;
    assert!(idle.status.success());
    assert_eq!(idle_json["status"], "idle");

    let succeeded_retry = Command::new(env!("CARGO_BIN_EXE_music-sync"))
        .args(["--json", "acquisition", "retry", "1", "--config"])
        .arg(&config)
        .output()?;
    let succeeded_retry_json: serde_json::Value = serde_json::from_slice(&succeeded_retry.stdout)?;
    assert_eq!(succeeded_retry.status.code(), Some(1));
    assert_eq!(succeeded_retry_json["released"], false);
    let history = Command::new(env!("CARGO_BIN_EXE_music-sync"))
        .args(["--json", "acquisition", "history", "--config"])
        .arg(&config)
        .output()?;
    let history_json: serde_json::Value = serde_json::from_slice(&history.stdout)?;
    assert!(history.status.success());
    assert_eq!(history_json[0]["status"], "succeeded");
    assert_eq!(history_json[0]["attempt_count"], 2);
    assert!(history_json[0]["latest_message"].as_str().is_some());
    let recover = Command::new(env!("CARGO_BIN_EXE_music-sync"))
        .args(["--json", "acquisition", "recover-running", "--config"])
        .arg(&config)
        .output()?;
    let recover_json: serde_json::Value = serde_json::from_slice(&recover.stdout)?;
    assert!(recover.status.success());
    assert_eq!(recover_json["recovered"], 0);
    Ok(())
}

#[test]
fn acquisition_run_pending_is_bounded_and_one_failure_does_not_block_others()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let state = root.path().join("state");
    let library = root.path().join("library");
    let playlists = root.path().join("playlists");
    fs::create_dir_all(&state)?;
    fs::create_dir_all(&library)?;
    fs::create_dir_all(&playlists)?;
    let database = state.join("music-sync.sqlite3");
    let config = root.path().join("music-sync.toml");
    fs::write(
        &config,
        format!(
            "state_directory = {:?}\nlibrary_directory = {:?}\nplaylist_directory = {:?}\n",
            state, library, playlists
        ),
    )?;
    let yt_dlp = root.path().join("yt-dlp");
    let ffprobe = root.path().join("ffprobe");
    fs::write(
        &yt_dlp,
        "#!/bin/sh\ncase \" $* \" in\n  *' --dump-single-json '*) printf '%s' '{\"id\":\"batch\",\"title\":\"Batch\",\"entries\":[{\"id\":\"good1\",\"url\":\"https://youtu.be/good1\"},{\"id\":\"bad\",\"url\":\"https://youtu.be/bad\"},{\"id\":\"good2\",\"url\":\"https://youtu.be/good2\"}]}' ;;\n  *' https://youtu.be/bad '*) printf '%s' 'temporary failure' >&2; exit 1 ;;\n  *) while test \"$1\" != '--paths'; do shift; done; printf '%s' 'fixture audio' > \"$2/media.opus\"; printf '%s\\n' \"$2/media.opus\" ;;\nesac\n",
    )?;
    fs::write(
        &ffprobe,
        "#!/bin/sh\nprintf '%s' '{\"streams\":[{\"codec_type\":\"audio\",\"codec_name\":\"opus\"}],\"format\":{\"duration\":\"1.5\"}}'\n",
    )?;
    for executable in [&yt_dlp, &ffprobe] {
        let mut permissions = fs::metadata(executable)?.permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(executable, permissions)?;
    }
    assert!(
        Command::new(env!("CARGO_BIN_EXE_music-sync"))
            .args([
                "source",
                "add",
                "https://www.youtube.com/playlist?list=batch",
                "--database",
            ])
            .arg(&database)
            .output()?
            .status
            .success()
    );
    assert!(
        Command::new(env!("CARGO_BIN_EXE_music-sync"))
            .args(["source", "reconcile", "1", "--database"])
            .arg(&database)
            .arg("--yt-dlp")
            .arg(&yt_dlp)
            .output()?
            .status
            .success()
    );
    let run = || {
        Command::new(env!("CARGO_BIN_EXE_music-sync"))
            .args([
                "--json",
                "acquisition",
                "run-pending",
                "--max-jobs",
                "3",
                "--config",
            ])
            .arg(&config)
            .arg("--yt-dlp")
            .arg(&yt_dlp)
            .arg("--ffprobe")
            .arg(&ffprobe)
            .output()
    };

    let first = run()?;
    let first_json: serde_json::Value = serde_json::from_slice(&first.stdout)?;
    assert!(first.status.success());
    assert_eq!(first_json["selected"], 3);
    assert_eq!(first_json["committed"], 2);
    assert_eq!(first_json["failures"].as_array().map(Vec::len), Some(1));
    assert_eq!(
        fs::read(library.join("youtube/good1.opus"))?,
        b"fixture audio"
    );
    assert_eq!(
        fs::read(library.join("youtube/good2.opus"))?,
        b"fixture audio"
    );
    assert!(!library.join("youtube/bad.opus").exists());

    let repeated = run()?;
    let repeated_json: serde_json::Value = serde_json::from_slice(&repeated.stdout)?;
    assert!(repeated.status.success());
    assert_eq!(repeated_json["selected"], 0);
    assert_eq!(repeated_json["committed"], 0);
    assert_eq!(repeated_json["failures"].as_array().map(Vec::len), Some(0));

    let retry = Command::new(env!("CARGO_BIN_EXE_music-sync"))
        .args(["acquisition", "retry", "2", "--config"])
        .arg(&config)
        .output()?;
    assert!(retry.status.success());
    let retried = run()?;
    let retried_json: serde_json::Value = serde_json::from_slice(&retried.stdout)?;
    assert_eq!(retried_json["selected"], 1);
    assert_eq!(retried_json["failures"].as_array().map(Vec::len), Some(1));
    Ok(())
}

#[test]
fn playlist_materialization_is_atomic_idempotent_and_tracks_active_membership()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let state = root.path().join("state");
    let library = root.path().join("library");
    let playlists = root.path().join("playlists");
    fs::create_dir_all(&state)?;
    fs::create_dir_all(&library)?;
    fs::create_dir_all(&playlists)?;
    let database = state.join("music-sync.sqlite3");
    let config = root.path().join("music-sync.toml");
    fs::write(
        &config,
        format!(
            "state_directory = {:?}\nlibrary_directory = {:?}\nplaylist_directory = {:?}\n",
            state, library, playlists
        ),
    )?;
    let yt_dlp = root.path().join("yt-dlp");
    let ffprobe = root.path().join("ffprobe");
    let write_enumerator = |entries: &str| {
        fs::write(
            &yt_dlp,
            format!(
                "#!/bin/sh\ncase \" $* \" in\n  *' --dump-single-json '*) printf '%s' '{{\"id\":\"playlist\",\"title\":\"Fixture\",\"entries\":[{entries}]}}' ;;\n  *) while test \"$1\" != '--paths'; do shift; done; output=$2; shift 2; printf '%s' 'fixture audio' > \"$output/media.opus\"; printf '%s\\n' \"$output/media.opus\" ;;\nesac\n"
            ),
        )?;
        let mut permissions = fs::metadata(&yt_dlp)?.permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(&yt_dlp, permissions)
    };
    let both = "{\"id\":\"one\",\"url\":\"https://youtu.be/one\"},{\"id\":\"two\",\"url\":\"https://youtu.be/two\"}";
    write_enumerator(both)?;
    fs::write(
        &ffprobe,
        "#!/bin/sh\nprintf '%s' '{\"streams\":[{\"codec_type\":\"audio\",\"codec_name\":\"opus\"}],\"format\":{\"duration\":\"1.5\"}}'\n",
    )?;
    let mut permissions = fs::metadata(&ffprobe)?.permissions();
    permissions.set_mode(0o700);
    fs::set_permissions(&ffprobe, permissions)?;
    let run = |arguments: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_music-sync"))
            .arg("--json")
            .args(arguments)
            .output()
    };
    assert!(
        Command::new(env!("CARGO_BIN_EXE_music-sync"))
            .args([
                "source",
                "add",
                "https://www.youtube.com/playlist?list=fixture",
                "--database",
            ])
            .arg(&database)
            .output()?
            .status
            .success()
    );
    let reconcile = Command::new(env!("CARGO_BIN_EXE_music-sync"))
        .args(["source", "reconcile", "1", "--database"])
        .arg(&database)
        .arg("--yt-dlp")
        .arg(&yt_dlp)
        .output()?;
    assert!(reconcile.status.success());
    let acquired = Command::new(env!("CARGO_BIN_EXE_music-sync"))
        .args(["acquisition", "run-pending", "--config"])
        .arg(&config)
        .args(["--max-jobs", "2", "--yt-dlp"])
        .arg(&yt_dlp)
        .arg("--ffprobe")
        .arg(&ffprobe)
        .output()?;
    assert!(acquired.status.success());

    let config_string = config.to_str().ok_or("non-UTF-8 config path")?;
    let first = run(&["playlist", "materialize", "--config", config_string])?;
    let first_json: serde_json::Value = serde_json::from_slice(&first.stdout)?;
    assert!(first.status.success());
    assert_eq!(first_json["playlists"][0]["effect"], "created");
    assert_eq!(first_json["playlists"][0]["entries"], 2);
    let output = playlists.join("collection-1.m3u8");
    assert_eq!(
        fs::read_to_string(&output)?,
        "#EXTM3U\nyoutube/one.opus\nyoutube/two.opus\n"
    );

    let repeated = run(&["playlist", "materialize", "--config", config_string])?;
    let repeated_json: serde_json::Value = serde_json::from_slice(&repeated.stdout)?;
    assert!(repeated.status.success());
    assert_eq!(repeated_json["playlists"][0]["effect"], "unchanged");

    let only_two = "{\"id\":\"two\",\"url\":\"https://youtu.be/two\"}";
    write_enumerator(only_two)?;
    let removal = Command::new(env!("CARGO_BIN_EXE_music-sync"))
        .args(["source", "reconcile", "1", "--database"])
        .arg(&database)
        .arg("--yt-dlp")
        .arg(&yt_dlp)
        .output()?;
    assert!(removal.status.success());
    let updated = run(&["playlist", "materialize", "--config", config_string])?;
    let updated_json: serde_json::Value = serde_json::from_slice(&updated.stdout)?;
    assert!(updated.status.success());
    assert_eq!(updated_json["playlists"][0]["effect"], "updated");
    assert_eq!(fs::read_to_string(output)?, "#EXTM3U\nyoutube/two.opus\n");
    assert!(library.join("youtube/one.opus").exists());
    assert!(library.join("youtube/two.opus").exists());
    Ok(())
}

#[test]
fn playlist_materialization_preserves_unknown_existing_output()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let state = root.path().join("state");
    let library = root.path().join("library");
    let playlists = root.path().join("playlists");
    fs::create_dir_all(&state)?;
    fs::create_dir_all(&library)?;
    fs::create_dir_all(&playlists)?;
    let database = state.join("music-sync.sqlite3");
    let config = root.path().join("music-sync.toml");
    fs::write(
        &config,
        format!(
            "state_directory = {:?}\nlibrary_directory = {:?}\nplaylist_directory = {:?}\n",
            state, library, playlists
        ),
    )?;
    let yt_dlp = root.path().join("yt-dlp");
    fs::write(
        &yt_dlp,
        "#!/bin/sh\nprintf '%s' '{\"id\":\"one\",\"webpage_url\":\"https://youtu.be/one\",\"title\":\"One\"}'\n",
    )?;
    let mut permissions = fs::metadata(&yt_dlp)?.permissions();
    permissions.set_mode(0o700);
    fs::set_permissions(&yt_dlp, permissions)?;
    assert!(
        Command::new(env!("CARGO_BIN_EXE_music-sync"))
            .args(["source", "add", "https://youtu.be/one", "--database",])
            .arg(&database)
            .output()?
            .status
            .success()
    );
    assert!(
        Command::new(env!("CARGO_BIN_EXE_music-sync"))
            .args(["source", "reconcile", "1", "--database"])
            .arg(&database)
            .arg("--yt-dlp")
            .arg(&yt_dlp)
            .output()?
            .status
            .success()
    );
    let output_path = playlists.join("collection-1.m3u8");
    fs::write(&output_path, b"user-owned\n")?;
    let output = Command::new(env!("CARGO_BIN_EXE_music-sync"))
        .args(["--json", "playlist", "materialize", "--config"])
        .arg(&config)
        .output()?;
    let report: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(report["failures"].as_array().map(Vec::len), Some(1));
    assert_eq!(fs::read(output_path)?, b"user-owned\n");
    Ok(())
}

#[test]
fn sync_run_completes_all_phases_and_isolates_one_source_failure()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let state = root.path().join("state");
    let library = root.path().join("library");
    let playlists = root.path().join("playlists");
    fs::create_dir_all(&state)?;
    fs::create_dir_all(&library)?;
    fs::create_dir_all(&playlists)?;
    let config = root.path().join("music-sync.toml");
    fs::write(
        &config,
        format!(
            "state_directory = {:?}\nlibrary_directory = {:?}\nplaylist_directory = {:?}\n",
            state, library, playlists
        ),
    )?;
    let database = state.join("music-sync.sqlite3");
    let marker = root.path().join("fail-source-two");
    let yt_dlp = root.path().join("yt-dlp");
    let ffprobe = root.path().join("ffprobe");
    let fpcalc = root.path().join("fpcalc");
    fs::write(
        &yt_dlp,
        format!(
            "#!/bin/sh\ncase \" $* \" in\n  *' --dump-single-json '*) case \" $* \" in *source-two*) if test -f {:?}; then printf '%s' 'temporary failure' >&2; exit 1; fi; printf '%s' '{{\"id\":\"two\",\"webpage_url\":\"https://youtu.be/two\",\"title\":\"Two\"}}' ;; *) printf '%s' '{{\"id\":\"one\",\"webpage_url\":\"https://youtu.be/one\",\"title\":\"One\"}}' ;; esac ;;\n  *) while test \"$1\" != '--paths'; do shift; done; output=$2; printf '%s' 'fixture audio' > \"$output/media.opus\"; printf '%s\\n' \"$output/media.opus\" ;;\nesac\n",
            marker
        ),
    )?;
    fs::write(
        &ffprobe,
        "#!/bin/sh\nprintf '%s' '{\"streams\":[{\"codec_type\":\"audio\",\"codec_name\":\"opus\"}],\"format\":{\"duration\":\"1.5\"}}'\n",
    )?;
    fs::write(
        &fpcalc,
        "#!/bin/sh\nprintf '%s' '{\"duration\":1.5,\"fingerprint\":[1,2,3]}'\n",
    )?;
    for executable in [&yt_dlp, &ffprobe, &fpcalc] {
        let mut permissions = fs::metadata(executable)?.permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(executable, permissions)?;
    }
    for url in ["https://youtu.be/source-one", "https://youtu.be/source-two"] {
        assert!(
            Command::new(env!("CARGO_BIN_EXE_music-sync"))
                .args(["source", "add", url, "--database"])
                .arg(&database)
                .output()?
                .status
                .success()
        );
    }
    let run = || {
        Command::new(env!("CARGO_BIN_EXE_music-sync"))
            .args(["--json", "sync", "run", "--config"])
            .arg(&config)
            .args(["--max-jobs", "2", "--yt-dlp"])
            .arg(&yt_dlp)
            .arg("--ffprobe")
            .arg(&ffprobe)
            .arg("--fpcalc")
            .arg(&fpcalc)
            .output()
    };

    let first = run()?;
    let first_report: serde_json::Value = serde_json::from_slice(&first.stdout)?;
    assert!(first.status.success());
    assert_eq!(first_report["sources"].as_array().map(Vec::len), Some(2));
    assert_eq!(first_report["acquisitions"]["committed"], 2);
    assert_eq!(first_report["fingerprints"]["recorded"], 2);
    assert_eq!(
        first_report["playlists"]["playlists"]
            .as_array()
            .map(Vec::len),
        Some(2)
    );
    assert!(library.join("youtube/one.opus").exists());
    assert!(library.join("youtube/two.opus").exists());

    fs::write(&marker, b"")?;
    let partial = run()?;
    let partial_report: serde_json::Value = serde_json::from_slice(&partial.stdout)?;
    assert_eq!(partial.status.code(), Some(1));
    assert_eq!(partial_report["sources"][0]["status"], "reconciled");
    assert_eq!(partial_report["sources"][1]["status"], "failed");
    assert_eq!(partial_report["acquisitions"]["selected"], 0);
    assert_eq!(partial_report["fingerprints"]["unchanged"], 2);
    assert_eq!(
        partial_report["playlists"]["failures"]
            .as_array()
            .map(Vec::len),
        Some(0)
    );
    assert_eq!(
        fs::read_to_string(playlists.join("collection-2.m3u8"))?,
        "#EXTM3U\nyoutube/two.opus\n"
    );
    Ok(())
}

#[test]
fn status_reports_durable_failures_without_modifying_state()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let state = root.path().join("state");
    let library = root.path().join("library");
    let playlists = root.path().join("playlists");
    fs::create_dir_all(&state)?;
    fs::create_dir_all(&library)?;
    fs::create_dir_all(&playlists)?;
    let config = root.path().join("music-sync.toml");
    fs::write(
        &config,
        format!(
            "state_directory = {:?}\nlibrary_directory = {:?}\nplaylist_directory = {:?}\n",
            state, library, playlists
        ),
    )?;
    let database = state.join("music-sync.sqlite3");
    let yt_dlp = root.path().join("yt-dlp");
    let ffprobe = root.path().join("ffprobe");
    fs::write(
        &yt_dlp,
        "#!/bin/sh\ncase \" $* \" in *' --dump-single-json '*) printf '%s' '{\"id\":\"one\",\"webpage_url\":\"https://youtu.be/one\",\"title\":\"One\"}' ;; *) printf '%s' 'temporary failure' >&2; exit 1 ;; esac\n",
    )?;
    fs::write(&ffprobe, "#!/bin/sh\nexit 1\n")?;
    for executable in [&yt_dlp, &ffprobe] {
        let mut permissions = fs::metadata(executable)?.permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(executable, permissions)?;
    }
    assert!(
        Command::new(env!("CARGO_BIN_EXE_music-sync"))
            .args(["source", "add", "https://youtu.be/one", "--database"])
            .arg(&database)
            .output()?
            .status
            .success()
    );
    assert!(
        Command::new(env!("CARGO_BIN_EXE_music-sync"))
            .args(["source", "reconcile", "1", "--database"])
            .arg(&database)
            .arg("--yt-dlp")
            .arg(&yt_dlp)
            .output()?
            .status
            .success()
    );
    assert!(
        !Command::new(env!("CARGO_BIN_EXE_music-sync"))
            .args(["acquisition", "run-one", "--config"])
            .arg(&config)
            .arg("--yt-dlp")
            .arg(&yt_dlp)
            .arg("--ffprobe")
            .arg(&ffprobe)
            .output()?
            .status
            .success()
    );
    let before_database = fs::read(&database)?;
    let mut before_entries = fs::read_dir(&state)?
        .map(|entry| entry.map(|entry| entry.file_name()))
        .collect::<Result<Vec<_>, _>>()?;
    before_entries.sort();

    let output = Command::new(env!("CARGO_BIN_EXE_music-sync"))
        .args(["--json", "status", "--config"])
        .arg(&config)
        .args(["--recent-events", "5"])
        .output()?;
    let report: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    assert!(output.status.success());
    assert_eq!(report["active_sources"], 1);
    assert_eq!(report["active_memberships"], 1);
    assert_eq!(report["unresolved_active_memberships"], 1);
    assert_eq!(report["jobs"]["deferred"], 1);
    assert_eq!(report["artifacts"]["healthy"], 0);
    assert_eq!(report["recent_events"].as_array().map(Vec::len), Some(1));
    assert_eq!(report["recent_events"][0]["event"], "acquisition_deferred");
    let history = Command::new(env!("CARGO_BIN_EXE_music-sync"))
        .args(["--json", "acquisition", "history", "--config"])
        .arg(&config)
        .args(["--limit", "5"])
        .output()?;
    let history_report: serde_json::Value = serde_json::from_slice(&history.stdout)?;
    assert!(history.status.success());
    assert_eq!(history_report.as_array().map(Vec::len), Some(1));
    assert_eq!(history_report[0]["job_id"], 1);
    assert_eq!(history_report[0]["status"], "deferred");
    assert_eq!(history_report[0]["attempt_count"], 1);
    assert!(history_report[0]["latest_message"].as_str().is_some());
    assert_eq!(fs::read(&database)?, before_database);
    let mut after_entries = fs::read_dir(&state)?
        .map(|entry| entry.map(|entry| entry.file_name()))
        .collect::<Result<Vec<_>, _>>()?;
    after_entries.sort();
    assert_eq!(after_entries, before_entries);
    assert!(playlists.read_dir()?.next().is_none());
    assert!(library.read_dir()?.next().is_none());
    Ok(())
}

#[test]
fn repair_verification_and_explicit_commit_preserve_the_lost_artifact_path()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let state = root.path().join("state");
    let library = root.path().join("library");
    let playlists = root.path().join("playlists");
    fs::create_dir_all(&state)?;
    fs::create_dir_all(&library)?;
    fs::create_dir_all(&playlists)?;
    let config = root.path().join("music-sync.toml");
    fs::write(
        &config,
        format!(
            "state_directory = {:?}\nlibrary_directory = {:?}\nplaylist_directory = {:?}\n",
            state, library, playlists
        ),
    )?;
    let database_path = state.join("music-sync.sqlite3");
    let yt_dlp = root.path().join("yt-dlp");
    let ffprobe = root.path().join("ffprobe");
    let fpcalc = root.path().join("fpcalc");
    fs::write(
        &yt_dlp,
        "#!/bin/sh\nset -eu\ncase \" $* \" in\n  *' --dump-single-json '*) printf '%s' '{\"id\":\"original\",\"webpage_url\":\"https://youtu.be/original\",\"title\":\"Fixture Track\"}' ;;\n  *) while test \"$1\" != '--paths'; do shift; done; output=$2; printf '%s' 'candidate audio' > \"$output/media.opus\"; printf '%s\\n' \"$output/media.opus\" ;;\nesac\n",
    )?;
    let mbid = "f59c5520-5f46-4d2c-b2c4-822eabf53419";
    fs::write(
        &ffprobe,
        format!(
            "#!/bin/sh\nprintf '%s' '{{\"streams\":[{{\"codec_type\":\"audio\",\"codec_name\":\"opus\",\"sample_rate\":\"48000\",\"channels\":2}}],\"format\":{{\"duration\":\"180.0\",\"tags\":{{\"MusicBrainz_Recording_Id\":\"{mbid}\"}}}}}}'\n"
        ),
    )?;
    let values = (1..=180)
        .map(|value| value.to_string())
        .collect::<Vec<_>>()
        .join(",");
    fs::write(
        &fpcalc,
        format!("#!/bin/sh\nprintf '%s' '{{\"duration\":180.0,\"fingerprint\":[{values}]}}'\n"),
    )?;
    for executable in [&yt_dlp, &ffprobe, &fpcalc] {
        let mut permissions = fs::metadata(executable)?.permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(executable, permissions)?;
    }
    assert!(
        Command::new(env!("CARGO_BIN_EXE_music-sync"))
            .args(["source", "add", "https://youtu.be/original", "--database"])
            .arg(&database_path)
            .output()?
            .status
            .success()
    );
    assert!(
        Command::new(env!("CARGO_BIN_EXE_music-sync"))
            .args(["sync", "run", "--config"])
            .arg(&config)
            .arg("--yt-dlp")
            .arg(&yt_dlp)
            .arg("--ffprobe")
            .arg(&ffprobe)
            .arg("--fpcalc")
            .arg(&fpcalc)
            .output()?
            .status
            .success()
    );
    let original = library.join("youtube/original.opus");
    fs::remove_file(&original)?;
    let connection = rusqlite::Connection::open(&database_path)?;
    assert_eq!(
        connection
            .query_row(
                "SELECT musicbrainz_recording_id FROM recordings WHERE id = 1",
                [],
                |row| row.get::<_, Option<String>>(0),
            )?
            .as_deref(),
        Some(mbid)
    );
    connection.execute("UPDATE artifacts SET health = 'missing' WHERE id = 1", [])?;
    connection.execute(
        "UPDATE provider_items SET availability = 'permanently_unavailable' WHERE id = 1",
        [],
    )?;
    connection.execute(
        "INSERT INTO repair_cases(recording_id, original_provider_item_id,
             reference_artifact_id, state) VALUES (1, 1, 1, 'unresolved')",
        [],
    )?;
    connection.execute(
        "INSERT INTO repair_attempts(repair_case_id, candidate_provider,
             candidate_provider_item_id, candidate_url, state, reason)
         VALUES (1, 'youtube', 'candidate', 'https://youtu.be/candidate',
                 'generated', 'fixture candidate')",
        [],
    )?;
    drop(connection);

    let output = Command::new(env!("CARGO_BIN_EXE_music-sync"))
        .args(["--json", "repair", "run-one", "--config"])
        .arg(&config)
        .arg("--yt-dlp")
        .arg(&yt_dlp)
        .arg("--ffprobe")
        .arg(&ffprobe)
        .arg("--fpcalc")
        .arg(&fpcalc)
        .output()?;
    let report: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(report["outcome"], "completed");
    assert_eq!(report["decision"], "verified");
    assert!(!original.exists());
    assert!(!library.join("repair").exists());
    let staged_candidate = state.join("repair-staging/attempt-1/media.opus");
    assert!(staged_candidate.exists());
    let commit = || {
        Command::new(env!("CARGO_BIN_EXE_music-sync"))
            .args(["--json", "repair", "commit", "1", "--config"])
            .arg(&config)
            .output()
    };
    fs::write(&staged_candidate, b"mutated after verification")?;
    let changed_commit = commit()?;
    assert_eq!(changed_commit.status.code(), Some(2));
    assert!(!library.join("repair").exists());
    fs::write(&staged_candidate, b"candidate audio")?;
    let first_commit = commit()?;
    let first_report: serde_json::Value = serde_json::from_slice(&first_commit.stdout)?;
    assert!(
        first_commit.status.success(),
        "{}",
        String::from_utf8_lossy(&first_commit.stderr)
    );
    assert_eq!(first_report["filesystem"], "created");
    assert_eq!(first_report["persistence"]["inserted"], true);
    let replacement = library.join("repair/recording-1-attempt-1.opus");
    assert_eq!(fs::read(&replacement)?, b"candidate audio");
    assert!(!original.exists());

    let repeated_commit = commit()?;
    let repeated_report: serde_json::Value = serde_json::from_slice(&repeated_commit.stdout)?;
    assert!(repeated_commit.status.success());
    assert_eq!(repeated_report["filesystem"], "recovered_existing");
    assert_eq!(repeated_report["persistence"]["inserted"], false);
    assert_eq!(fs::read(&replacement)?, b"candidate audio");
    Ok(())
}

#[test]
fn metadata_resolve_uses_strong_identity_and_persists_field_provenance()
-> Result<(), Box<dyn std::error::Error>> {
    use std::io::{Read, Write};
    use std::net::TcpListener;

    let root = tempfile::tempdir()?;
    let state = root.path().join("state");
    let library = root.path().join("library");
    let playlists = root.path().join("playlists");
    fs::create_dir_all(&state)?;
    fs::create_dir_all(&library)?;
    fs::create_dir_all(&playlists)?;
    let config = root.path().join("music-sync.toml");
    fs::write(
        &config,
        format!(
            "state_directory = {:?}\nlibrary_directory = {:?}\nplaylist_directory = {:?}\n",
            state, library, playlists
        ),
    )?;
    let database_path = state.join("music-sync.sqlite3");
    assert!(
        Command::new(env!("CARGO_BIN_EXE_music-sync"))
            .args(["source", "add", "https://youtu.be/bootstrap", "--database"])
            .arg(&database_path)
            .output()?
            .status
            .success()
    );
    let mbid = "f59c5520-5f46-4d2c-b2c4-822eabf53419";
    let connection = rusqlite::Connection::open(&database_path)?;
    connection.execute(
        "INSERT INTO recordings(musicbrainz_recording_id) VALUES (?1)",
        [mbid],
    )?;
    connection.execute(
        "INSERT INTO artifacts(recording_id, path, health)
         VALUES (1, '/fixture/track.opus', 'healthy')",
        [],
    )?;
    connection.execute(
        "UPDATE recordings SET preferred_artifact_id = 1 WHERE id = 1",
        [],
    )?;
    connection.execute("INSERT INTO recordings(isrc) VALUES ('USXYZ2412345')", [])?;
    connection.execute(
        "INSERT INTO artifacts(recording_id, path, health)
         VALUES (2, '/fixture/track-two.opus', 'healthy')",
        [],
    )?;
    connection.execute(
        "UPDATE recordings SET preferred_artifact_id = 2 WHERE id = 2",
        [],
    )?;
    drop(connection);

    let listener = TcpListener::bind("127.0.0.1:0")?;
    let address = listener.local_addr()?;
    let body = include_str!("../../music-sync/tests/fixtures/musicbrainz/recording.json");
    let second_recording = body
        .replace(mbid, "01234567-89ab-cdef-0123-456789abcdef")
        .replace("USABC2412345", "USXYZ2412345")
        .replace("Fixture Track", "Second Fixture Track");
    let isrc_body = format!("{{\"isrc\":\"USXYZ2412345\",\"recordings\":[{second_recording}]}}");
    let server = std::thread::spawn(move || -> Result<(), std::io::Error> {
        for expected in ["recording", "isrc"] {
            let (mut stream, _) = listener.accept()?;
            let mut request = [0_u8; 8_192];
            let length = stream.read(&mut request)?;
            let request = String::from_utf8_lossy(&request[..length]);
            assert!(
                request
                    .to_ascii_lowercase()
                    .contains("user-agent: music-sync-test/0.1 (test@example.invalid)")
            );
            let response_body = if expected == "recording" {
                assert!(request.starts_with(&format!("GET /ws/2/recording/{mbid}?")));
                body
            } else {
                assert!(request.starts_with("GET /ws/2/isrc/USXYZ2412345?"));
                &isrc_body
            };
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                response_body.len(),
                response_body
            )?;
        }
        Ok(())
    });
    let output = Command::new(env!("CARGO_BIN_EXE_music-sync"))
        .args(["--json", "metadata", "resolve", "--config"])
        .arg(&config)
        .args(["--user-agent", "music-sync-test/0.1 (test@example.invalid)"])
        .arg("--endpoint")
        .arg(format!("http://{address}/ws/2"))
        .arg("--max-recordings")
        .arg("2")
        .output()?;
    server.join().map_err(|_| "fixture server panicked")??;
    let report: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(report["selected"], 2);
    assert_eq!(report["resolved"], 2);
    let connection = rusqlite::Connection::open(&database_path)?;
    assert_eq!(
        connection.query_row("SELECT COUNT(*) FROM metadata_selections", [], |row| {
            row.get::<_, u64>(0)
        })?,
        8
    );
    assert_eq!(
        connection.query_row(
            "SELECT value FROM metadata_observations WHERE field = 'title'",
            [],
            |row| row.get::<_, String>(0),
        )?,
        "Fixture Track"
    );
    Ok(())
}

#[test]
fn artwork_fetch_caches_validated_front_cover_and_repeats_offline()
-> Result<(), Box<dyn std::error::Error>> {
    use std::io::{Read, Write};
    use std::net::TcpListener;

    let root = tempfile::tempdir()?;
    let state = root.path().join("state");
    let library = root.path().join("library");
    let playlists = root.path().join("playlists");
    fs::create_dir_all(&state)?;
    fs::create_dir_all(&library)?;
    fs::create_dir_all(&playlists)?;
    let config = root.path().join("music-sync.toml");
    fs::write(
        &config,
        format!(
            "state_directory = {:?}\nlibrary_directory = {:?}\nplaylist_directory = {:?}\n",
            state, library, playlists
        ),
    )?;
    let database_path = state.join("music-sync.sqlite3");
    assert!(
        Command::new(env!("CARGO_BIN_EXE_music-sync"))
            .args(["source", "add", "https://youtu.be/bootstrap", "--database"])
            .arg(&database_path)
            .output()?
            .status
            .success()
    );
    let release_mbid = "99999999-8888-7777-6666-555555555555";
    let connection = rusqlite::Connection::open(&database_path)?;
    connection.execute("INSERT INTO recordings DEFAULT VALUES", [])?;
    connection.execute(
        "INSERT INTO releases(musicbrainz_release_id, canonical_title)
         VALUES (?1, 'Fixture Album')",
        [release_mbid],
    )?;
    connection.execute(
        "INSERT INTO metadata_observations(recording_id, field, value, source,
         source_entity_id, confidence_millionths, resolution_context)
         VALUES (1, 'release', 'Fixture Album', 'musicbrainz', ?1, 1000000, 'fixture')",
        [release_mbid],
    )?;
    connection.execute(
        "INSERT INTO metadata_selections(recording_id, field, observation_id)
         VALUES (1, 'release', 1)",
        [],
    )?;
    drop(connection);

    let listener = TcpListener::bind("127.0.0.1:0")?;
    let address = listener.local_addr()?;
    let image = b"\xff\xd8\xfffixture-jpeg";
    let index = format!(
        "{{\"images\":[{{\"id\":42,\"image\":\"http://{address}/image.jpg\",\"front\":true,\"approved\":true}}]}}"
    );
    let server = std::thread::spawn(move || -> Result<(), std::io::Error> {
        for response_body in [index.as_bytes(), image.as_slice()] {
            let (mut stream, _) = listener.accept()?;
            let mut request = [0_u8; 8_192];
            let length = stream.read(&mut request)?;
            let request = String::from_utf8_lossy(&request[..length]);
            assert!(
                request
                    .to_ascii_lowercase()
                    .contains("user-agent: music-sync-test/0.1 (test@example.invalid)")
            );
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                response_body.len()
            )?;
            stream.write_all(response_body)?;
        }
        Ok(())
    });
    let run = |endpoint: String| {
        Command::new(env!("CARGO_BIN_EXE_music-sync"))
            .args(["--json", "artwork", "fetch", "--config"])
            .arg(&config)
            .args(["--user-agent", "music-sync-test/0.1 (test@example.invalid)"])
            .arg("--endpoint")
            .arg(endpoint)
            .output()
    };
    let first = run(format!("http://{address}"))?;
    server.join().map_err(|_| "fixture server panicked")??;
    let first_report: serde_json::Value = serde_json::from_slice(&first.stdout)?;
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    assert_eq!(first_report["resolved"], 1);
    let second = run("http://127.0.0.1:9".into())?;
    let second_report: serde_json::Value = serde_json::from_slice(&second.stdout)?;
    assert!(second.status.success());
    assert_eq!(second_report["selected"], 0);
    let connection = rusqlite::Connection::open(&database_path)?;
    let relative_path =
        connection.query_row("SELECT relative_path FROM artwork_blobs", [], |row| {
            row.get::<_, String>(0)
        })?;
    assert_eq!(fs::read(state.join(relative_path))?, image);
    assert_eq!(fs::read_dir(&library)?.count(), 0);
    Ok(())
}

#[test]
fn lyrics_fetch_writes_owned_adjacent_sidecar_and_repeats_offline()
-> Result<(), Box<dyn std::error::Error>> {
    use std::io::{Read, Write};
    use std::net::TcpListener;

    let root = tempfile::tempdir()?;
    let state = root.path().join("state");
    let library = root.path().join("library");
    let playlists = root.path().join("playlists");
    fs::create_dir_all(&state)?;
    fs::create_dir_all(&library)?;
    fs::create_dir_all(&playlists)?;
    let audio = library.join("track.opus");
    fs::write(&audio, b"owned audio")?;
    let config = root.path().join("music-sync.toml");
    fs::write(
        &config,
        format!(
            "state_directory = {:?}\nlibrary_directory = {:?}\nplaylist_directory = {:?}\n",
            state, library, playlists
        ),
    )?;
    let database = state.join("music-sync.sqlite3");
    assert!(
        Command::new(env!("CARGO_BIN_EXE_music-sync"))
            .args(["source", "add", "https://youtu.be/bootstrap", "--database"])
            .arg(&database)
            .output()?
            .status
            .success()
    );
    let connection = rusqlite::Connection::open(&database)?;
    connection.execute("INSERT INTO recordings DEFAULT VALUES", [])?;
    connection.execute("INSERT INTO artifacts(recording_id,path,duration_ms,health) VALUES (1,?1,180000,'healthy')", [audio.to_str().ok_or("path")?])?;
    connection.execute(
        "UPDATE recordings SET preferred_artifact_id=1 WHERE id=1",
        [],
    )?;
    connection.execute("INSERT INTO provider_items(provider,provider_item_id,original_url,recording_id) VALUES ('youtube','fixture','https://youtu.be/fixture',1)", [])?;
    connection.execute(
        "INSERT INTO sync_runs(status,finished_at) VALUES ('succeeded',CURRENT_TIMESTAMP)",
        [],
    )?;
    connection.execute("INSERT INTO jobs(run_id,kind,status,idempotency_key) VALUES (1,'acquire','succeeded','acquire:youtube:fixture')", [])?;
    connection.execute(
        "INSERT INTO acquisition_jobs(job_id,provider_item_id) VALUES (1,1)",
        [],
    )?;
    connection.execute("INSERT INTO acquisition_commits(job_id,staged_path,final_path,sha256,bytes,codec,duration_ms,status,committed_at) VALUES (1,'/tmp/staged',?1,?2,11,'opus',180000,'committed',CURRENT_TIMESTAMP)", rusqlite::params![audio.to_str().ok_or("path")?, "00".repeat(32)])?;
    for (field, value) in [
        ("title", "Fixture Track"),
        ("artist_credit", "Fixture Artist"),
        ("release", "Fixture Album"),
    ] {
        connection.execute("INSERT INTO metadata_observations(recording_id,field,value,source,source_entity_id,confidence_millionths,resolution_context) VALUES (1,?1,?2,'musicbrainz','fixture',1000000,'fixture')", [field,value])?;
        let id = connection.last_insert_rowid();
        connection.execute(
            "INSERT INTO metadata_selections(recording_id,field,observation_id) VALUES (1,?1,?2)",
            rusqlite::params![field, id],
        )?;
    }
    drop(connection);
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let address = listener.local_addr()?;
    let server = std::thread::spawn(move || -> Result<(), std::io::Error> {
        let (mut stream, _) = listener.accept()?;
        let mut request = [0_u8; 8192];
        let length = stream.read(&mut request)?;
        let request = String::from_utf8_lossy(&request[..length]);
        assert!(request.starts_with("GET /api/get?"));
        assert!(request.contains("duration=180"));
        let body = r#"{"id":42,"trackName":"Fixture Track","artistName":"Fixture Artist","albumName":"Fixture Album","duration":180.0,"instrumental":false,"plainLyrics":"Plain line","syncedLyrics":"[00:01.20] Synced line"}"#;
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        )?;
        Ok(())
    });
    let run = |endpoint: String| {
        Command::new(env!("CARGO_BIN_EXE_music-sync"))
            .args(["--json", "lyrics", "fetch", "--config"])
            .arg(&config)
            .args([
                "--user-agent",
                "music-sync-test/0.1 (test@example.invalid)",
                "--endpoint",
            ])
            .arg(endpoint)
            .output()
    };
    let first = run(format!("http://{address}/api"))?;
    server.join().map_err(|_| "server")??;
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&first.stdout)?;
    assert_eq!(report["sidecars_committed"], 1);
    assert_eq!(
        fs::read(library.join("track.lrc"))?,
        b"[00:01.20] Synced line\n"
    );
    let repeat = run("http://127.0.0.1:9/api".into())?;
    assert!(repeat.status.success());
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&repeat.stdout)?["selected"],
        0
    );
    assert_eq!(fs::read(&audio)?, b"owned audio");
    Ok(())
}

#[test]
fn metadata_materialize_stream_copies_and_retains_original_bytes()
-> Result<(), Box<dyn std::error::Error>> {
    use music_sync::content_hash::{ContentHasher, Sha256FileHasher};

    let root = tempfile::tempdir()?;
    let state = root.path().join("state");
    let library = root.path().join("library");
    let playlists = root.path().join("playlists");
    fs::create_dir_all(&state)?;
    fs::create_dir_all(&library)?;
    fs::create_dir_all(&playlists)?;
    let audio = library.join("track.opus");
    let original = b"original-opus";
    fs::write(&audio, original)?;
    let hash = Sha256FileHasher.hash(&audio)?;
    let source_sha = hash
        .sha256
        .iter()
        .map(|v| format!("{v:02x}"))
        .collect::<String>();
    let config = root.path().join("music-sync.toml");
    fs::write(
        &config,
        format!(
            "state_directory = {:?}\nlibrary_directory = {:?}\nplaylist_directory = {:?}\n",
            state, library, playlists
        ),
    )?;
    let database = state.join("music-sync.sqlite3");
    assert!(
        Command::new(env!("CARGO_BIN_EXE_music-sync"))
            .args(["source", "add", "https://youtu.be/bootstrap", "--database"])
            .arg(&database)
            .output()?
            .status
            .success()
    );
    let connection = rusqlite::Connection::open(&database)?;
    connection.execute("INSERT INTO recordings(musicbrainz_recording_id,isrc) VALUES ('f59c5520-5f46-4d2c-b2c4-822eabf53419','USABC2412345')",[])?;
    connection.execute("INSERT INTO artifacts(recording_id,path,sha256,duration_ms,codec,sample_rate_hz,channels,health) VALUES (1,?1,?2,1000,'opus',48000,2,'healthy')",rusqlite::params![audio.to_str().ok_or("path")?,source_sha])?;
    connection.execute(
        "UPDATE recordings SET preferred_artifact_id=1 WHERE id=1",
        [],
    )?;
    connection.execute("INSERT INTO provider_items(provider,provider_item_id,original_url,recording_id) VALUES ('youtube','fixture','https://youtu.be/fixture',1)",[])?;
    connection.execute(
        "INSERT INTO sync_runs(status,finished_at) VALUES ('succeeded',CURRENT_TIMESTAMP)",
        [],
    )?;
    connection.execute("INSERT INTO jobs(run_id,kind,status,idempotency_key) VALUES (1,'acquire','succeeded','acquire:youtube:fixture')",[])?;
    connection.execute(
        "INSERT INTO acquisition_jobs(job_id,provider_item_id) VALUES (1,1)",
        [],
    )?;
    connection.execute("INSERT INTO acquisition_commits(job_id,staged_path,final_path,sha256,bytes,codec,duration_ms,status,committed_at) VALUES (1,'/tmp/staged',?1,?2,13,'opus',1000,'committed',CURRENT_TIMESTAMP)",rusqlite::params![audio.to_str().ok_or("path")?,source_sha])?;
    let release_mbid = "99999999-8888-7777-6666-555555555555";
    for (field, value) in [
        ("title", "Canonical Track"),
        ("artist_credit", "Canonical Artist"),
        ("release", "Canonical Album"),
        ("release_date", "2025-01-02"),
    ] {
        let entity = if field == "release" {
            release_mbid
        } else {
            "fixture"
        };
        let cursor=connection.execute("INSERT INTO metadata_observations(recording_id,field,value,source,source_entity_id,confidence_millionths,resolution_context) VALUES (1,?1,?2,'musicbrainz',?3,1000000,'fixture')",[field,value,entity])?;
        assert_eq!(cursor, 1);
        let id = connection.last_insert_rowid();
        connection.execute(
            "INSERT INTO metadata_selections(recording_id,field,observation_id) VALUES (1,?1,?2)",
            rusqlite::params![field, id],
        )?;
    }
    let artwork_relative =
        "artwork-cache/aa/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa.jpg";
    let artwork = state.join(artwork_relative);
    fs::create_dir_all(artwork.parent().ok_or("artwork parent")?)?;
    fs::write(&artwork, b"\xff\xd8\xffcanonical-art")?;
    connection.execute("INSERT INTO releases(musicbrainz_release_id,canonical_title) VALUES (?1,'Canonical Album')",[release_mbid])?;
    connection.execute("INSERT INTO artwork_blobs(sha256,relative_path,mime_type,byte_count) VALUES (?1,?2,'image/jpeg',16)",rusqlite::params!["aa".repeat(32),artwork_relative])?;
    connection.execute("INSERT INTO release_artwork(release_id,blob_id,source,source_image_id,source_url,role,approved) VALUES (1,1,'cover_art_archive','42','https://example.invalid/art.jpg','front',1)",[])?;
    drop(connection);
    let tools = tempfile::tempdir()?;
    let ffmpeg = tools.path().join("ffmpeg");
    let ffprobe = tools.path().join("ffprobe");
    fs::copy(
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../music-sync/tests/fixtures/tag/ffmpeg-copy.sh"
        ),
        &ffmpeg,
    )?;
    fs::copy(
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../music-sync/tests/fixtures/tag/ffprobe.sh"
        ),
        &ffprobe,
    )?;
    for path in [&ffmpeg, &ffprobe] {
        let mut permissions = fs::metadata(path)?.permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(path, permissions)?;
    }
    let run = || {
        Command::new(env!("CARGO_BIN_EXE_music-sync"))
            .args(["--json", "metadata", "materialize", "--config"])
            .arg(&config)
            .args(["--ffmpeg"])
            .arg(&ffmpeg)
            .arg("--ffprobe")
            .arg(&ffprobe)
            .args(["--max-recordings", "1"])
            .output()
    };
    let first = run()?;
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&first.stdout)?;
    assert_eq!(report["committed"], 1);
    assert_eq!(fs::read(&audio)?, b"original-opus-canonical-tags");
    let history = state
        .join("artifact-history")
        .join(&source_sha[..2])
        .join(format!("{source_sha}.opus"));
    assert_eq!(fs::read(&history)?, original);
    let repeat = run()?;
    assert!(repeat.status.success());
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&repeat.stdout)?["selected"],
        0
    );
    let connection = rusqlite::Connection::open(database)?;
    assert_eq!(
        connection.query_row("SELECT COUNT(*) FROM artifacts", [], |row| row
            .get::<_, u64>(0))?,
        2
    );
    Ok(())
}

#[test]
fn library_health_reports_hash_mismatch_without_modifying_media()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let state = root.path().join("state");
    let library = root.path().join("library");
    let playlists = root.path().join("playlists");
    fs::create_dir_all(&state)?;
    fs::create_dir_all(&library)?;
    fs::create_dir_all(&playlists)?;
    let media = library.join("track.opus");
    fs::write(&media, b"original audio")?;
    let config = root.path().join("music-sync.toml");
    fs::write(
        &config,
        format!(
            "state_directory = {:?}\nlibrary_directory = {:?}\nplaylist_directory = {:?}\n",
            state, library, playlists
        ),
    )?;
    let database = state.join("music-sync.sqlite3");
    let ffprobe = root.path().join("ffprobe");
    let fpcalc = root.path().join("fpcalc");
    fs::write(
        &ffprobe,
        "#!/bin/sh\nprintf '%s' '{\"streams\":[{\"codec_type\":\"audio\",\"codec_name\":\"opus\"}],\"format\":{\"duration\":\"1.5\"}}'\n",
    )?;
    let mut permissions = fs::metadata(&ffprobe)?.permissions();
    permissions.set_mode(0o700);
    fs::set_permissions(&ffprobe, permissions)?;
    fs::write(
        &fpcalc,
        "#!/bin/sh\nprintf '%s' '{\"duration\":1.5,\"fingerprint\":[1,2,3,4]}'\n",
    )?;
    let mut permissions = fs::metadata(&fpcalc)?.permissions();
    permissions.set_mode(0o700);
    fs::set_permissions(&fpcalc, permissions)?;
    assert!(
        Command::new(env!("CARGO_BIN_EXE_music-sync"))
            .args(["library", "adopt"])
            .arg(&library)
            .args(["--apply", "--database"])
            .arg(&database)
            .output()?
            .status
            .success()
    );
    let run = || {
        Command::new(env!("CARGO_BIN_EXE_music-sync"))
            .args(["--json", "library", "health", "--config"])
            .arg(&config)
            .args(["--max-artifacts", "1", "--ffprobe"])
            .arg(&ffprobe)
            .output()
    };
    let first = run()?;
    let first_report: serde_json::Value = serde_json::from_slice(&first.stdout)?;
    assert!(first.status.success());
    assert_eq!(first_report["healthy"], 1);
    assert_eq!(first_report["changed"], 1);

    let fingerprint = || {
        Command::new(env!("CARGO_BIN_EXE_music-sync"))
            .args(["--json", "library", "fingerprint", "--config"])
            .arg(&config)
            .args([
                "--max-artifacts",
                "1",
                "--max-audio-seconds",
                "60",
                "--fpcalc",
            ])
            .arg(&fpcalc)
            .output()
    };
    let fingerprinted = fingerprint()?;
    let fingerprinted_report: serde_json::Value = serde_json::from_slice(&fingerprinted.stdout)?;
    assert!(fingerprinted.status.success());
    assert_eq!(fingerprinted_report["recorded"], 1);
    let fingerprint_repeat = fingerprint()?;
    let fingerprint_repeat_report: serde_json::Value =
        serde_json::from_slice(&fingerprint_repeat.stdout)?;
    assert!(fingerprint_repeat.status.success());
    assert_eq!(fingerprint_repeat_report["unchanged"], 1);

    fs::write(&media, b"externally changed audio")?;
    let second = run()?;
    let second_report: serde_json::Value = serde_json::from_slice(&second.stdout)?;
    assert!(second.status.success());
    assert_eq!(second_report["corrupt"], 1);
    assert_eq!(second_report["changed"], 1);
    let repeated = run()?;
    let repeated_report: serde_json::Value = serde_json::from_slice(&repeated.stdout)?;
    assert!(repeated.status.success());
    assert_eq!(repeated_report["corrupt"], 1);
    assert_eq!(repeated_report["changed"], 0);
    assert_eq!(fs::read(media)?, b"externally changed audio");
    Ok(())
}
