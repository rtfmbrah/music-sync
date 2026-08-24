# Proxmox LXC deployment profile

## Confirmed target

Read-only inspection on 2026-08-23 established:

- Proxmox container `101`, Debian 12 (bookworm), x86_64, glibc 2.36;
- SSH alias `music-sync-lxc`, unprivileged user `music-sync-dev`;
- production music `/srv/music`, read-only to that user;
- isolated writable test area `/srv/music-sync-v2-test`;
- ext4 storage with 469 GiB total and 190 GiB free at inspection time;
- `ffmpeg`/`ffprobe` 5.1.9 and `yt-dlp` 2026.06.09 installed;
- `fpcalc`, SQLite CLI, Rust, and Cargo absent;
- Docker exists, but its API is correctly unavailable to the development user.

The application bundles SQLite, so the missing SQLite CLI is not a runtime blocker.
Chromaprint/`fpcalc` becomes required when fingerprint acquisition is implemented.

## Existing library adoption profile

The production library contains 1,560 AAC/M4A artifacts, 891 LRC sidecars, 45 text
files, 16 M3U playlists, two JPEG files, and no symlinks. Existing playlists use
relative paths. Filenames commonly retain YouTube provider IDs. A representative
artifact contained valid AAC audio, embedded JPEG artwork, and title/artist/album
tags, but no canonical recording ID in the selected fields.

These observations are generic migration inputs only. No legacy source or application
state was inspected. Future adoption must begin read-only and preserve every file.

## Binary portability

Ordinary binaries produced on NixOS reference Nix-store glibc paths and are not
portable to Debian. The supported test artifact is therefore built with:

```bash
nix develop -c just build-portable
```

The repository Cargo configuration keeps artifacts in the hidden sibling directory
`../.music-sync-v2-target` so direct Cargo, rust-analyzer, and Nix development builds
cannot place `target` inside a path-flake snapshot. This produces the static-musl
x86_64 binary under
`../.music-sync-v2-target/x86_64-unknown-linux-musl/release/music-sync`.
Deployment must copy it only into isolated test or versioned application storage.
Never build in or write to the production music directory.

Workspace editor settings additionally direct rust-analyzer flycheck output to
`../.music-sync-v2-ra-target`; flycheck otherwise creates an in-repository
`target/flycheck*` directory independently of Cargo's configured target directory.

## yt-dlp runtime policy

yt-dlp is a replaceable provider adapter, not linked into music-sync. YouTube changes
frequently, so it needs a faster update cadence than the Rust application. Releases
pin the official upstream Unix zipimport artifact and SHA-256, install it beside the
application (for example `/opt/music-sync/libexec/yt-dlp`), and pass that explicit
path to the adapter. The artifact requires Python; the larger official
`yt-dlp_linux` standalone build is the fallback for hosts without Python.

`scripts/install-ytdlp DESTINATION` currently pins stable version `2026.08.19`. It
downloads from the immutable upstream release URL, verifies the pinned SHA-256, and
atomically replaces only the explicit destination. It is network-dependent and is
never part of `just check`. Updating the pin requires reviewing upstream release
notes, changing both version and checksum, running fixture tests, and validating in
isolated LXC storage before deployment.

The container's existing `/usr/local/bin/yt-dlp` is the official self-update-capable
Unix artifact and reported `2026.06.09` on 2026-08-24. A root operator can update it
immediately with `/usr/local/bin/yt-dlp -U`, but production music-sync deployments
should use the project-pinned side-by-side artifact so rollback stays deterministic.

The pinned `2026.08.19` artifact was checksum-verified by the installer and executed
successfully from `/srv/music-sync-v2-test` on 2026-08-24. The existing system copy
was left unchanged, and production music remained non-writable.

The bootstrap binary was successfully executed from `/srv/music-sync-v2-test` on
2026-08-23. Its offline doctor found the configured read-only library and installed
media tools. The test did not initialize a database or create playlist output.

The first real adoption dry run also completed there: 12 directories and 2,514 files
were scanned, producing 1,560 recognized media, 891 lyrics, two artwork candidates,
16 playlists, 45 unknown preserved files, and zero corrupt/unreadable paths. It
reported zero modifications, deletions, and downloads; the production write guard
remained false.

A subsequent bounded full-library ffprobe run inspected all 1,560 media files with a
10-second per-file deadline. All succeeded with known duration and AAC audio; total
duration was 522,373,526 ms, 1,557 files had embedded artwork, all had basic tags, and
none exposed MBID/ISRC canonical identity in the inspected tag fields. Files modified,
deleted, and downloaded remained zero, and production remained non-writable.

Relationship validation on 2026-08-24 associated all 891 LRC files with same-stem
media and found both artwork candidates beside recognized media. It parsed 2,925
entries across all 16 bounded playlists; every entry resolved to recognized media
inside the scan root, with zero missing, external, or invalid references. The scan
again reported zero modifications, deletions, and downloads, and the production
write guard remained false.

Transactional adoption apply was validated on 2026-08-24 using a new SQLite database
under `/srv/music-sync-v2-test`. The first run registered 1,560 unresolved recordings
and 1,560 artifacts from the read-only production library. An immediate second run
inserted zero rows and reported all 1,560 artifacts as existing. Both runs reported
zero modified, deleted, or downloaded library files, and production remained
non-writable. No application state was written outside isolated test storage.

The bounded yt-dlp source boundary was validated offline on 2026-08-24 using the
stored two-item playlist fixture and the static musl binary under isolated test
storage. Ordered provider IDs, URLs, durations, titles, and raw metadata were emitted
correctly. The target has yt-dlp 2026.06.09 installed. No live provider request was
made, no production state changed, and the music library remained non-writable.

Canonical embedded-tag validation on 2026-08-24 probed all 1,560 media files with
recording MBID aliases and ISRC selection enabled. All probes succeeded; no valid or
malformed MusicBrainz recording ID or ISRC tags were observed. The run reported zero
modifications, deletions, and downloads, and production remained non-writable.

Bounded SHA-256 adoption evidence was validated on 2026-08-24. A deterministic
100-file sample hashed 633,727,849 bytes successfully. The subsequent full pass
hashed all 1,560 recognized media files and 12,719,299,395 bytes with zero failures,
skips, or exact-byte duplicate groups. Both runs reported zero modifications,
deletions, and downloads; the production write guard remained false.

Persistent source management was validated on 2026-08-24 against a new schema-v2
SQLite database under `/srv/music-sync-v2-test`. A YouTube playlist source was added
with ID 1, repeated without duplication, listed, deactivated, listed as inactive,
and reactivated with the same ID and name. No provider request or library-file effect
occurred, and production music remained non-writable.

Transactional source reconciliation was validated on 2026-08-24 with the static
musl binary, stored two-item yt-dlp fixture, and a new schema-v3 database under
isolated test storage. The first snapshot inserted two provider items, activated two
memberships, and created two acquisition jobs. The identical repeat inserted no
items, changed no memberships, and created no jobs. A deliberately failing adapter
was classified as transient before persistence; the following successful repeat
still observed both memberships unchanged. No download ran, and production music
remained read-only.

The complete offline acquisition workflow was validated on 2026-08-24 with a new
schema-v5 database and static-musl binary under isolated test storage. A stored
provider fixture produced one job; `acquisition run-one` staged 13 bytes, accepted a
fixture ffprobe audio stream, hashed the exact bytes, atomically created
`library/youtube/video1.opus`, and transactionally committed recording/artifact ID 1.
The immediate repeat reported idle. No live provider was contacted, no partial file
appeared in production, and `/srv/music` remained non-writable.

Atomic playlist materialization was validated on 2026-08-24 with the updated static
musl binary and a new schema-v6 database under
`/srv/music-sync-v2-test/playlist-20260824`. Two offline provider items were acquired
and produced an ordered `collection-1.m3u8`. The immediate repeat reported
`Unchanged`. A following snapshot deactivated one membership; materialization
reported `Updated`, omitted that entry, and preserved both acquired audio files.
`/srv/music` remained non-writable throughout.

## Safety boundary

- Never read `/srv/.git`, `/srv/.music-sync`, legacy source, or `/srv/.env`.
- Never access Navidrome's database contents or Docker socket.
- Never write, rename, tag, or delete under `/srv/music` during adoption tests.
- Use `/srv/music-sync-v2-test` for binaries, config, state, and generated output.
- Global package installation or service changes require separate explicit approval.

## Timer-driven operation

The repository includes example systemd units under `deploy/systemd`. They are not
installed automatically. Before deployment, create a dedicated service account,
place configuration outside the repository, install the static binary and pinned
yt-dlp at the explicit unit paths, and replace the example writable library and
playlist paths with the configured deployment paths. `ProtectSystem=strict` keeps
the remaining host filesystem read-only to the service.

The oneshot command returns 0 only when all source, acquisition, and playlist work
succeeds, 1 after isolated failures, and 2 for fatal configuration or durable-state
errors. The timer may therefore alert on partial runs without preventing successful
unrelated work from being committed.

The composed command was validated on 2026-08-24 with the static-musl binary and
offline two-item fixtures under `/srv/music-sync-v2-test/sync-20260824`. Its first
run reconciled one source, committed two acquisitions, and created one ordered
playlist. The repeat reported two unchanged memberships, zero selected acquisition
jobs, and an unchanged playlist. Both audio files remained present and `/srv/music`
remained non-writable. Local `systemd-analyze verify` parsed the example units and
then reported the expected missing `/opt/music-sync/bin/music-sync`, because the
examples are deliberately not installed on the development host. A complete unit
verification remains a deployment-time check after adapting and installing paths.

Read-only operational status was validated against that completed isolated sync
state. It reported schema 6, one active source, two resolved active memberships, two
succeeded jobs, two healthy artifacts, and one committed playlist output. The exact
SQLite SHA-256 and the state-directory file set were identical before and after the
command, no SQLite sidecars appeared, and `/srv/music` remained non-writable.
