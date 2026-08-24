# Persist configured sources

- Status: Completed
- Started: 2026-08-24
- Completed: 2026-08-24
- Roadmap: P1 source synchronization

## Goal

Persist configured YouTube sources with idempotent add/list/deactivate operations so
future sync runs have a durable source set.

## Work completed

- Added forward schema-v2 migration and durable source rows with stable IDs.
- Added conservative HTTPS YouTube URL validation before database creation.
- Added idempotent add, active/default and all-source listing, deactivation-only
  remove, and same-ID reactivation with name preservation.
- Added text/JSON CLI commands and strong source ID/domain response types.
- Added v1-to-v2 migration, persistence lifecycle, URL, and black-box CLI tests.

## Validation evidence

- `just check`: passed with warnings-denied Clippy, 52 deterministic tests,
  architecture enforcement, and deployment-script syntax validation.
- Static musl release build: passed.
- Debian LXC lifecycle used a new isolated schema-v2 database and preserved stable ID
  1 across add/repeat/list/deactivate/list-all/reactivate/list.
- No live provider call or media effect occurred; production remained non-writable.
