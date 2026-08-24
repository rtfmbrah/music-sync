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
- Persisted raw perceptual identity evidence and bounded adoption reconciliation
  (complete); external canonical resolution remains planned.
- Source/provider model and stored `yt-dlp` fixtures (complete).
- Inspect-only YouTube video/playlist enumeration with typed errors (complete).
- Persistent source add/list/deactivate/reactivate lifecycle (complete).
- Transactional membership reconciliation and idempotency tests (complete).
- Atomic M3U8 materialization for active memberships (complete).

## P2 — Acquisition and identity

- Crash-safe acquisition jobs and recovery (complete for initial yt-dlp/ffprobe/hash
  acquisition; interrupted staging is retained).
- `yt-dlp`, `ffprobe`, `ffmpeg`, hash, and `fpcalc` adapters (yt-dlp, ffprobe, hash,
  and bounded raw fpcalc complete; ffmpeg planned).
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

## P4 — Autonomous discovery

- Seeds and recommendation-provider boundaries.
- Deduplication and explainable taste scoring.
- Navidrome listening-signal reader without database mutation.
- Growth/storage/artist budgets and tunable exploration.
- Automated verified acquisition through the P2 pipeline.

## P5 — Operations and deployment

- Bounded `sync run` orchestration and systemd examples (complete).
- Read-only durable operational status (complete).
- Acquisition job history and explicit retry/recovery (complete).
- Run-level history, expanded failure filtering, and expanded doctor commands.
- Interactive and JSON progress, persistent logs, systemd examples.
- Least-privilege LXC compatibility/deployment validation in isolated test storage.
- Production health checks, performance measurement, and repair workflows.
