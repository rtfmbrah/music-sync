# Validate adoption relationships

- Status: Completed
- Started: 2026-08-24
- Completed: 2026-08-24
- Roadmap: P1 adoption

## Goal

Extend the read-only adoption report with conservative adjacent-sidecar association
and bounded M3U/M3U8 reference validation without following links or changing files.

## Work completed

- Associated same-directory, same-stem LRC files with recognized healthy media.
- Reported whether artwork candidates occur in directories containing media.
- Added an 8 MiB UTF-8 playlist bound and lexical reference resolution.
- Classified entries as resolved, missing in-root, or external without canonicalizing
  paths, following links, fetching URIs, or claiming recording identity.
- Added deterministic relationship, malformed-playlist, preservation, and CLI output
  coverage.
- Stabilized newly created subprocess fixtures and probe executables with a narrow,
  bounded retry for transient executable-busy errors.

## Validation evidence

- `just check`: passed, including Clippy with warnings denied, 26 deterministic tests,
  and architecture enforcement.
- Static musl release build: passed.
- Debian LXC scan: all 891 lyrics associated, both artwork files beside media, and
  all 2,925 entries across 16 playlists resolved.
- Remote scan found zero missing, external, or invalid references and reported zero
  modified, deleted, or downloaded files.
- The production library remained non-writable to `music-sync-dev`.
