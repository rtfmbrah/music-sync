# Add bounded artifact hash evidence

- Status: Completed
- Started: 2026-08-24
- Completed: 2026-08-24
- Roadmap: P1 adoption

## Goal

Add opt-in, bounded SHA-256 evidence to read-only adoption so exact duplicate files
can be observed without confusing artifact equality with recording identity.

## Work completed

- Added a reusable streaming SHA-256 boundary with fixed-size buffers.
- Added opt-in `--hash`, deterministic `--max-hashes`, and independent combined
  probe/hash operation.
- Aggregated attempts, successes, failures, skips, bytes, exact-duplicate groups,
  and duplicate files without exposing digest values.
- Kept artifact equality explicitly separate from musical recording identity.
- Added known-vector, duplicate, limit, failure, combined-operation, CLI, and
  preservation tests.

## Validation evidence

- `just check`: passed, including warnings-denied Clippy, 32 deterministic tests,
  and architecture enforcement.
- Static musl release build: passed.
- Debian LXC full scan hashed all 1,560 media files and 12,719,299,395 bytes with
  zero failures and zero exact-byte duplicate groups.
- Remote sample and full runs reported zero modified, deleted, or downloaded files;
  production remained non-writable to `music-sync-dev`.
