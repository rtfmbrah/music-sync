//! Black-box tests for truthful command-line behavior.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::process::Command;

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
    assert_eq!(repeated_json["selected"], 1);
    assert_eq!(repeated_json["committed"], 0);
    assert_eq!(repeated_json["failures"].as_array().map(Vec::len), Some(1));
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
    for executable in [&yt_dlp, &ffprobe] {
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
            .output()
    };

    let first = run()?;
    let first_report: serde_json::Value = serde_json::from_slice(&first.stdout)?;
    assert!(first.status.success());
    assert_eq!(first_report["sources"].as_array().map(Vec::len), Some(2));
    assert_eq!(first_report["acquisitions"]["committed"], 2);
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
