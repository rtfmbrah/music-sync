# Product requirements

This document inventories product capabilities. The roadmap determines sequence;
architecture documents explain implementation boundaries.

## Current bootstrap

- Rust 1.95.0 / Edition 2024 workspace with reusable library and thin CLI.
- Strict TOML configuration for directories, bounded concurrency, and discovery
  budgets; secrets remain separate.
- SQLite schema migrations for domain and operational state.
- Structured `tracing` foundation and verbosity levels.
- Read-only, offline `doctor` checks for config, paths, SQLite, and media tools.
- Read-only generic library scanner classifying media, lyrics, artwork, playlists,
  symlinks, unknown files, unreadable paths, and obvious empty media.
- Optional bounded ffprobe summaries for codec, duration, embedded artwork, useful
  tag presence, canonical-ID presence, and structural probe failures.
- Conservative same-stem lyric and adjacent-artwork observations plus bounded,
  read-only M3U/M3U8 reference validation.
- Opt-in, bounded SHA-256 artifact evidence and exact-byte duplicate counts, kept
  explicitly separate from musical identity.
- Structural validation and aggregate counts for embedded MusicBrainz recording IDs
  and ISRCs, treated as unverified canonical evidence.
- Explicit transactional adoption apply for absolute media paths, with unresolved
  identities, whole-batch rollback, idempotency, and zero library-file effects.
- Bounded inspect-only YouTube video/playlist enumeration through `yt-dlp`, producing
  provider-neutral snapshots with retained raw metadata and typed failures.
- Persistent HTTPS YouTube source add/list/deactivate/reactivate operations with
  stable IDs and forward SQLite migration.
- Transactional successful-snapshot reconciliation into provider items, ordered
  active memberships, succeeded sync runs, and idempotent acquisition jobs;
  enumeration failure leaves prior membership unchanged.
- One-job acquisition workflow composing durable claim, job staging, bounded yt-dlp,
  ffprobe, SHA-256, atomic no-clobber managed artifact commit, and transactional
  recording/artifact/job finalization with deferred failure retry.
- Bounded acquisition batches snapshot runnable jobs and attempt each at most once,
  continuing after isolated failures without immediately retrying the same job.
- Atomic UTF-8 M3U8 materialization from ordered active memberships and preferred
  healthy artifacts, with unresolved-entry reporting and no-clobber first writes.
- One bounded timer-friendly sync command composing active-source reconciliation,
  acquisition, and playlist phases while isolating ordinary provider/job failures.
- Executable preservation and replacement policies plus deterministic tests.
- Nix development shell, task runner, architecture enforcement, and CI-ready check.

## Planned source and library management

- Adopt arbitrary libraries with a read-only dry run by default. Inspect media tags,
  names/layout, duration/codec, canonical and provider IDs, and perceptual
  fingerprints. Report identified, partial, unknown, duplicate, and corrupt files.
  Extend explicit apply with evidence persistence and conservative reconciliation.
- Never require redownload merely because the v2 database is new.

## Planned acquisition and audio policy

- Wrap `yt-dlp`, `ffmpeg`, `ffprobe`, and Chromaprint/`fpcalc` as subprocess
  adapters. Keep YouTube behavior isolated.
- Classify provider/infrastructure errors into meaningful domain states.
- Download to a per-job temporary directory; validate, probe, verify, enrich, hash,
  fingerprint, atomically rename, then transactionally commit state.
- Recover or clean incomplete jobs after crashes without exposing partial media.
- Preserve the best sensible source codec. Avoid lossy-to-lossy conversion for
  uniformity; remux without re-encoding when justified. Let Navidrome transcode for
  playback.
- Record cryptographic hash, Chromaprint, duration, codec, sample rate, channels,
  provider origin, and canonical identities when known.

## Planned enrichment

- Resolve recording/release/artist through MusicBrainz, release artwork through
  Cover Art Archive, lyrics through a high-quality provider such as LRCLIB, and
  similarity/listening signals through ListenBrainz or suitable alternatives.
- Preserve provenance and confidence per metadata field; never use last-writer-wins
  metadata. Provider titles, channels, descriptions, duration, thumbnail, URL, and
  playlist context remain auditable fallback data.
- Canonical artwork outranks music-video thumbnails.
- Support synchronized and plain lyrics, preferably as Navidrome-compatible adjacent
  `.lrc` files so lyrics updates do not rewrite audio.

## Planned autonomous discovery

- Keep deterministic source discovery separate from probabilistic music discovery.
- Generate from explicit seeds, library contents, related artists/recordings, genres,
  channels, Navidrome favorites/ratings/plays, and future recommendation providers.
- Deduplicate before acquisition and keep taste confidence separate from identity
  confidence.
- Use explainable scoring first, recording why each item was selected.
- Balance tunable exploitation, exploration, and a small wildcard component.
- Enforce daily growth, per-artist diversity, bounded concurrency, backoff, and
  minimum-free-space limits. Never use routine human inbox review as the normal loop.

## Planned operations

- Deterministic `sync` and high-level `run` commands suitable for systemd timers or
  cron; an internal scheduler/daemon is not a priority.
- Useful interactive phase progress and `--no-progress`; JSON/non-interactive output
  for systemd automation; `-v` through `-vvv` logging.
- Persistent `SyncRun`, `Job`, attempts, and contextual events supporting status,
  run history, failures, and safe retry commands.
- Contextual logs include time, level, run/component, source/track/job IDs, event,
  message, and explainable replacement/discovery decisions.
- Normal local tests use fixtures, never live YouTube or metadata providers. Live
  tests and dependency audits remain opt-in.
- Linux runtime stays portable to the eventual Debian-like LXC; environment and
  package assumptions are detected only after access is granted.
