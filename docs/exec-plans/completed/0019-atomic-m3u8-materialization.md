# Materialize active collections as atomic M3U8

- Status: Complete
- Started: 2026-08-24
- Roadmap: P1 source synchronization / Navidrome output

## Goal

Write one stable UTF-8 M3U8 per reconciled collection using only active memberships
that resolve to an explicit preferred healthy artifact.

## Scope

- Add a forward migration for preferred artifacts and owned playlist output paths.
- Set the initial acquired artifact as preferred without replacing an existing choice.
- Query ordered active membership paths and count unresolved entries.
- Derive stable `collection-<id>.m3u8` names under the configured playlist directory.
- First output uses atomic no-clobber creation; registered updates use atomic replace.
- Synchronize file and directory state and persist the emitted SHA-256.
- Add `playlist materialize` text/JSON CLI behavior and deterministic tests.
- Validate the complete source/acquisition/playlist workflow in isolated LXC storage.

## Safety boundary

- Inactive membership never appears in output and never deletes its artifact.
- Unknown existing playlist files are never overwritten or adopted implicitly.
- Missing/unresolved artifacts are reported and omitted rather than guessed.

## Result

- Added schema version 6 with explicit preferred artifacts and owned playlist paths.
- Added atomic, idempotent M3U8 materialization and text/JSON CLI output.
- Added deterministic coverage for creation, repeat, membership update, audio
  preservation, and unknown-output no-clobber behavior.
- `just check` passed with 75 deterministic tests.
- Static-musl LXC validation passed in isolated test storage; production music
  remained read-only.
