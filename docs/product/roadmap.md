# Authoritative roadmap

This is the single product-level TODO list. Task detail belongs in execution plans.
Completed behavior is moved into release notes or marked here; it is not duplicated
in ad hoc TODO documents.

## P0 — Repository foundation (complete)

- Rust library-first workspace, pinned toolchain, Nix shell, `just` commands.
- Configuration, structured observability, SQLite migrations.
- Preservation and identity policy vertical slice.
- Unit, integration/scenario, and architecture tests.
- Product/architecture docs, ADRs, and agent workflow.

## P1 — Library adoption and source synchronization

- Safe extension/readability scanner and read-only adoption report (complete).
- Bounded structural media probing and tag-presence summary (complete).
- Conservative sidecar and bounded playlist-reference validation (complete).
- Bounded artifact hashes and exact-byte duplicate evidence (complete).
- Embedded canonical-tag validation (complete).
- Initial transactional, idempotent adoption apply (complete).
- Fingerprint-verified adopted-artifact/provider migration without duplicate library
  output, including first-activation quarantine of unresolved filename candidates
  (complete, including production migration acceptance).
- Persisted raw perceptual identity evidence and bounded adoption reconciliation
  with durable isolated-failure deferral (complete); external canonical resolution
  remains planned.
- Source/provider model and stored `yt-dlp` fixtures (complete).
- Inspect-only YouTube video/playlist enumeration with typed errors (complete).
- Persistent source add/list/deactivate/reactivate lifecycle (complete).
- Transactional membership reconciliation and idempotency tests (complete).
- Atomic M3U8 materialization for active memberships (complete).

## P2 — Acquisition and identity

- Crash-safe acquisition jobs and recovery (complete for initial yt-dlp/ffprobe/hash
  acquisition; interrupted staging is retained).
- `yt-dlp`, `ffprobe`, `ffmpeg`, hash, and `fpcalc` adapters (complete, including
  bounded source-preserving ffmpeg remuxing).
- Artifact probing and health reconciliation (complete).
- Conservative staged candidate pipeline and perceptual identity thresholds
  (complete for retained embedded canonical evidence; external resolution planned).
- Fixture-backed unavailable/transient/provider-change scenarios (unavailable,
  transient, mismatched, insufficient, verified, and staging-change coverage complete).

## P3 — Metadata enrichment

- MusicBrainz recording/artist/release resolution and field provenance (complete for
  exact recording MBID and unique ISRC lookup with deterministic release context).
- Cover Art Archive selection and immutable content-addressed cache (complete).
- Synchronized/plain LRCLIB lyrics with crash-safe adjacent-file output (complete).
- Source-preserving atomic tag normalization and canonical artwork priority (complete).
- Provenance-aware provider display enrichment (schemas 20–21): complete YouTube payloads
  for managed media, provider title/artist/album/genre/thumbnail fallbacks for lyrics,
  artwork, and tags without identity claims, plus explicit zero-work phase status
  (complete and production accepted, including compatible PNG-cover rematerialization,
  durable lyrics mismatch migration, and real Navidrome/Feishin verification).
- Conservative external genre enrichment (schemas 22–23): bounded MusicBrainz
  exact/search resolution, duration and version verification, provenance, provider
  genre merging, exact-artist fallback, and lossless retagging (complete and
  production accepted on schema 23).

## P4 — Autonomous discovery

- Seeds and recommendation-provider boundaries (complete for Navidrome favorites and
  ListenBrainz collaborative filtering).
- Deduplication and explainable taste scoring (complete).
- Navidrome listening-signal reader without database mutation (complete for exact
  recording-MBID favorites).
- Growth/storage/artist budgets and tunable exploration (complete).
- Automated verified acquisition through the P2 pipeline (complete for exact
  MusicBrainz recording-level YouTube relationships and independent staged
  MBID/ISRC plus duration verification; isolated LXC acceptance remains).

## P5 — Operations and deployment

- Bounded `sync run` orchestration and systemd examples (complete).
- Read-only durable operational status (complete).
- Acquisition job history and explicit retry/recovery (complete).
- Run-level history plus exact run/event/component/job filtering and expanded
  offline doctor checks (complete).
- Simplified operator CLI, full per-source member status trees, and immutable
  evidence-separated duplicate reporting (complete and production accepted).
- Complete mutually exclusive service-cycle orchestration (complete); interactive
  progress and persistent log policy remain.
- JSON phase summaries, hardened systemd examples, consistent state backup, and
  atomic versioned release/rollback tooling (complete).
- Least-privilege LXC compatibility/deployment validation (complete for isolated
  acceptance and production activation with paced provider access, versioned
  releases, schema 19 state, named existing-playlist adoption, half-hour sync, and
  daily backup timers).
- Production health checks, performance measurement, and repair workflows.
