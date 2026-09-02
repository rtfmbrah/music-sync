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

The repository includes example service, sync timer, and daily backup timer units
under `deploy/systemd`. They are not
installed automatically. Before deployment, create a dedicated service account,
place configuration outside the repository, install the static binary and pinned
yt-dlp at the explicit unit paths, and replace the example writable library and
playlist paths with the configured deployment paths. `ProtectSystem=strict` keeps
the remaining host filesystem read-only to the service.

The sync timer uses a half-hour wall-clock schedule with a randomized delay of up to
two minutes. A calendar schedule retains a real next activation after installation
or restart; it does not inherit an already-expired monotonic trigger from an older
unit definition. `Persistent=true` catches up one missed activation after downtime.

The hardened oneshot invokes `service run --trigger timer`, loads optional secrets
from `/etc/music-sync/music-sync.env`, and returns 0 only when every enabled phase
succeeds, 1 after isolated failures, and 2 for fatal configuration or durable-state
errors. Schema-backed exclusivity rejects overlap before provider or managed-file
effects. The timer may therefore alert on partial runs without preventing successful
unrelated work from being committed.

Optional AcoustID discovery search reads `ACOUSTID_CLIENT_KEY` from the same
protected environment file. The key is required only when
`discovery.youtube_search_fallback=true` and never belongs in tracked TOML. `doctor`
checks presence without displaying its value. AcoustID receives a compressed
Chromaprint and duration, not audio bytes. Defaults stage at most three serial
YouTube candidates and fingerprint at most 900 seconds per candidate.
`deploy/configure-acoustid-discovery.sh` reads the key on standard input, validates
it without echoing or logging it, atomically preserves other environment secrets,
backs up and updates the `[discovery]` policy, runs `doctor` with the protected
environment loaded, and restarts only the timer. The key must not be placed on a
command line because process listings and shell history could expose it.

When YouTube requires authenticated anti-bot access, configure
`service.yt_dlp_cookie_file` with an absolute Netscape-format cookie file outside the
repository. The adapter passes only its path to yt-dlp and never reads, serializes, or
logs cookie contents. `doctor` requires the file to be regular and inaccessible to
group and other users. Cookie rotation does not require an application rebuild.
yt-dlp persists cookie-jar updates back to the configured file, so the hardened
service unit grants write access to that single secret path while `/etc` otherwise
remains protected read-only.

Timer-driven provider access is deliberately paced. The default service policy asks
yt-dlp to wait one second between extraction requests and to wait a randomized five
to fifteen seconds before each media download. These positive bounds are explicit
configuration and are validated at startup. Serial job execution alone is not a
provider-throttling policy; keep the pacing enabled even when acquisition concurrency
is one.

The production adapter also selects YouTube's `default,web_embedded` client set for
cookie-authenticated access and places yt-dlp's cache below the configured state
directory. This avoids the demonstrated logged-in `tv_downgraded` reload failure and
does not require a home directory for the dedicated service account.

Do not enable `MemoryDenyWriteExecute` on the service unit. yt-dlp delegates current
YouTube JavaScript challenges to Deno, whose V8 runtime requires executable memory;
the restriction reproducibly makes Deno panic and leaves no playable formats. The
unit retains `NoNewPrivileges`, strict filesystem protection, private temporary
storage, namespace restrictions, and kernel/control-group hardening.

Expose `/srv` through one `ReadWritePaths` entry rather than separate state and
library entries. Separate systemd bind mounts create a mount boundary inside the
sandbox and make the crash-safe hard-link commit fail with `EXDEV`, even when both
paths report the same backing filesystem outside the unit. The dedicated service
account's ordinary ownership and ACLs still restrict actual writes to application
state, the managed library subtree, playlists, and the explicitly writable cookie
file; preserved production audio remains non-writable.

For an adopted library, do not enable the timer immediately after provider-link
verification. First run `music-sync library quarantine-unverified-provider-links
--config /etc/music-sync/music-sync.toml --max-items 10000` while the service is
stopped, review its JSON report, and confirm that no verification remains `running`.
The command defers only unresolved exact filename candidates; it does not touch
audio. This prevents the first ordinary service cycle from downloading duplicates.

## Versioned releases, backup, and rollback

`deploy/install-release.sh VERSION MUSIC_SYNC_BINARY YT_DLP_BINARY` installs both
already-validated executables into a new immutable
`/opt/music-sync/releases/VERSION` directory. It executes both version commands and
then atomically switches `/opt/music-sync/current`; it never replaces an existing
release directory. The service configuration points its yt-dlp adapter at
`/opt/music-sync/current/yt-dlp`, so application and provider-adapter rollback occur
together. Run the installer only after isolated acceptance, and retain the previous
release.

Before activation or schema migration, create a snapshot with `music-sync
maintenance backup --config /etc/music-sync/music-sync.toml --directory
/srv/music-sync-backups`. The directory must already exist and be writable only by
the service account. The daily example timer performs the same consistent SQLite
snapshot and intentionally does not delete old backups. Operators define and test
retention outside music-sync.

Never stop an active oneshot merely to begin maintenance: that leaves its exclusive
durable service run truthfully `running` until explicit operator recovery, and every
later timer invocation must refuse to overlap it. Run
`deploy/quiesce-service.sh [TIMEOUT_SECONDS]` as root before backup or activation.
It stops only the timer, waits for any current service cycle to finish naturally,
and leaves the timer stopped for maintenance. On timeout or interruption it restores
the previously active timer and performs no recovery. After maintenance, explicitly
run `systemctl start music-sync.timer`.

If a process was externally interrupted, first confirm that the unit is inactive and
that no music-sync service process exists. Back up SQLite, inspect `runs history
--status running`, and use `runs recover-interrupted` exactly once. This changes only
the abandoned durable run/phase state; it never retries work or changes audio.

`deploy/rollback-release.sh VERSION` validates a retained release and atomically
repoints `current`. Quiesce the timer and allow the service to finish before rollback. Binary rollback is
safe only when that retained binary supports the current schema; otherwise restore
the paired pre-upgrade SQLite snapshot first, while the service is stopped. Audio is
never part of automated rollback and must not be deleted or replaced.

On the confirmed LXC, `/var/lib` is on the 30 GiB root filesystem while `/srv/music`
is on the 469 GiB data filesystem. Production state therefore belongs at
`/srv/music-sync-state`, and backups at `/srv/music-sync-backups`. This both avoids
the constrained root volume and keeps acquisition staging on the same filesystem as
the library, which is required for atomic hard-link commits.

Production activation completed on 2026-08-25 with schema 18 and release
`0.1.0-20260825.3`. The migration registered 1,560 healthy artifacts without
rewriting audio, proved 1,127 adopted/provider associations, quarantined 205
unresolved filename candidates, deferred 117 unavailable provider objects, and
persisted 88 isolated fingerprint failures for explicit retry. The hardened service
uses the dedicated account, pinned yt-dlp, request/download pacing, `/srv` state and
backup paths, a half-hour sync timer, and a daily backup timer. A partial service run
remains a failed oneshot result for alerting while the timer continues scheduling
later runs.

The first new production playlist item completed end to end on 2026-08-25. After the
authenticated-client, Deno-compatible sandbox, and single-`/srv` mount corrections,
job 1450 downloaded YouTube item `5li1Cu-AJpo`, validated it, atomically committed a
new healthy WebM artifact, updated playlist output, and repeated with a fully
successful service cycle. Production then reported 1,561 healthy artifacts, 1,128
succeeded acquisition jobs, no pending/running work, and no missing or corrupt
audio.

Named playlist adoption was accepted and deployed on 2026-08-26 with schema 19 and
release `0.1.0-20260826.5`. An isolated copy of production state proved that the
eight real YouTube playlist sources adopted their existing `.m3u` files, while the
four single-video sources produced no playlist output. The production upgrade first
created a consistent SQLite backup and a complete copy of `/srv/music/_playlists`,
then retired only legacy `collection-<id>.m3u8` files whose bytes matched their
recorded music-sync SHA-256. A repeated materialization was unchanged. Final doctor
reported schema 19; status reported eight playlist outputs, 1,562 healthy artifacts,
and no missing or corrupt artifacts. No audio was modified or deleted, and the
half-hour timer remained active after acceptance.

The simplified operator CLI and immutable duplicate report were accepted against a
consistent production-state copy on 2026-08-27. `list --full` classified all 1,571
active members across eight playlist and four single-video sources in 156 ms: 1,129
had healthy local audio, 321 retained failed/deferred acquisition diagnostics, 120
were explicitly missing, and one carried explicit copyright evidence. Healthy local
audio took precedence over every provider state. The duplicate report considered
1,562 healthy artifacts, used 1,473 stored raw fingerprints, explicitly counted 89
with insufficient fingerprint evidence, and produced 125 evidence groups in 8.526
seconds. The copied SQLite SHA-256 was identical before and after both commands and
no WAL or shared-memory sidecars were created. No provider or media file was read or
changed by either report.

Production release `0.1.0-20260827.6` activated the accepted CLI on 2026-08-27
without a schema change. A consistent pre-upgrade SQLite backup was created first.
Installed `doctor` passed every configuration, directory, schema-19, tool, secret,
filesystem, and endpoint check using the default production config path. Direct
production `list --full` and `duplicates` repeated the accepted 12-source,
1,571-member, and 125-group results; the production SQLite SHA-256 was identical
before and after. The timer was restored after validation.

The composed command was validated on 2026-08-24 with the static-musl binary and
offline two-item fixtures under `/srv/music-sync-v2-test/sync-20260824`. Its first
run reconciled one source, committed two acquisitions, and created one ordered
playlist. The repeat reported two unchanged memberships, zero selected acquisition
jobs, and an unchanged playlist. Both audio files remained present and `/srv/music`
remained non-writable. Local `systemd-analyze verify` parsed the example units and
then reported the expected missing `/opt/music-sync/current/music-sync`, because the
examples are deliberately not installed on the development host. A complete unit
verification remains a deployment-time check after adapting and installing paths.

Read-only operational status was validated against that completed isolated sync
state. It reported schema 6, one active source, two resolved active memberships, two
succeeded jobs, two healthy artifacts, and one committed playlist output. The exact
SQLite SHA-256 and the state-directory file set were identical before and after the
command, no SQLite sidecars appeared, and `/srv/music` remained non-writable.

Explicit acquisition recovery was validated on 2026-08-24 under
`/srv/music-sync-v2-test/recovery-20260824`. Two offline acquisitions were forced to
fail transiently and appeared in newest-first history as deferred with attempt count
one and their persisted diagnostics. A normal following batch selected zero jobs.
After explicit `acquisition retry 1`, a bounded batch selected and committed only
that job; the other remained deferred. Production music remained non-writable.

Bounded artifact health was validated against the isolated recovered artifact. The
static binary reported one selected healthy artifact with no state change or failure.
Its exact media SHA-256 was identical before and after the check, and production
music remained non-writable.

Raw Chromaprint persistence was validated with the schema-v7 static binary and a
deterministic `fpcalc` fixture against the two isolated sync artifacts. The first
bounded pass recorded two fingerprints; the identical repeat reported both
unchanged. Exact media hashes matched before and after, operational status reported
schema 7 and two healthy artifacts, and `/srv/music` remained non-writable.

The complete conservative repair path was validated on 2026-08-25 with the
schema-v10 static binary and offline provider/probe/fingerprint fixtures under
`/srv/music-sync-v2-test/repair-20260825`. One acquired artifact retained embedded
canonical identity and raw fingerprint evidence. Its final test-library path was
moved to preserved test storage, health became missing, and an explicit permanent
original loss created exactly one repair case. Bounded search generated one untrusted
candidate; staging, structural validation, exact hashing, canonical/duration/raw
fingerprint comparison verified it. No media appeared in the library before explicit
commit. Commit created a new no-clobber repair artifact, retained the historical
bytes and missing artifact row, and playlist materialization changed preference to
the new path. Status reported one committed attempt, one healthy replacement, and
one historical missing artifact. `/srv/music` remained non-writable throughout.

Canonical metadata resolution was validated with the schema-v11 static binary and a
single-request MusicBrainz-compatible loopback fixture against that isolated repaired
recording. The adapter sent the required application User-Agent, parsed recording,
ordered artist credit, release, and ISRC JSON, resolved one exact MBID, and selected
four independently provenanced fields. The immediate repeat selected zero work and
made no provider request. Status reported one resolved recording and four selected
fields; `/srv/music` remained non-writable.

Release artwork was validated with the schema-v12 static binary and a two-request
Cover Art Archive-compatible loopback fixture under
`/srv/music-sync-v2-test/artwork-20260825`. An initial deliberate connection race was
durably deferred without accepting bytes; the retry selected one canonical release,
validated JPEG magic, and created one SHA-256-addressed immutable cache blob. An
immediate repeat used an unreachable endpoint, selected zero work, and still
succeeded, proving it performed no HTTP request. Status reported one resolved release
and one cached blob. No file was written to the test library or production library,
and `/srv/music` remained non-writable.

Lyrics enrichment was validated with the schema-v13 static binary and an LRCLIB-
compatible loopback fixture under `/srv/music-sync-v2-test/lyrics-20260825`. One
healthy artifact proven owned by a committed acquisition used selected canonical
title, artist credit, release, and probed duration. Synchronized lyrics outranked the
plain response and were atomically committed as an adjacent `.lrc`; the audio bytes
remained unchanged. An immediate repeat used an unreachable endpoint, selected zero
work, and succeeded without HTTP. Status reported one resolved recording and one
committed sidecar. `/srv/music` remained non-writable.

Source-preserving canonical tag materialization was validated with the schema-v14
static binary and Debian's real ffmpeg/ffprobe under
`/srv/music-sync-v2-test/tags-20260825`. A real generated Opus stream and JPEG were
processed with audio stream copy. Full ffprobe output showed canonical title, artist,
album, date, recording MBID, and ISRC on the audio stream, plus a canonical MJPEG
stream with `attached_pic=1` decoded from the Opus picture block. The visible result
had a new container hash while immutable artifact history exactly matched the original
SHA-256. The immediate repeat selected zero work and succeeded with deliberately
nonexistent ffmpeg/ffprobe paths. Status reported one committed materialization and two
healthy artifact identities. `/srv/music` remained non-writable.

The schema-v16 complete production-operations cycle was accepted on Debian 12 under
`/srv/music-sync-v2-test/service-p5-20260825` on 2026-08-25. The exact static binary
SHA-256 was `a485112722cdd4c32c509a2841032abec3a9bf782559c512c3eea5947b0504d2`.
Offline doctor passed configuration, directories, executable-path, same-filesystem,
endpoint, and secret-contract checks; its initial missing-database warning was
expected. Because the target has no fpcalc package yet, `/bin/true` occupied only the
explicit fpcalc fixture path for this empty-library orchestration test; final service
readiness still requires real `fpcalc`.

Two complete 14-phase cycles succeeded in 41 ms total, repeated with no selected
work, and produced two durable successful run records with no failed phases. A
consistent schema-v16 backup was created in isolated backup storage, and immutable
status reported no pending work or failures. The static binary, configuration,
state, generated playlists, and backup all remained inside isolated test storage.
The production library was verified readable and non-writable before and after.

Schema-v17 adopted/provider verification was accepted under isolated LXC storage on
2026-08-25 using a read-only copy of one sufficiently long production M4A. The exact
YouTube ID token generated one candidate. Real Debian ffprobe and fpcalc derived
independent duration and raw Chromaprint evidence; an offline yt-dlp fixture staged
the same provider bytes. Verification linked one provider object, satisfied its
pending acquisition job, selected the healthy adopted artifact as preferred, and
reduced unresolved active memberships to zero. No `library/youtube` duplicate was
created, the adopted and production SHA-256 both remained
`3f41c49eff9ba6e682e490fe2d42beacdab8d7d811383db916d1d7ea0fd5c3cf`, and
production remained non-writable.
