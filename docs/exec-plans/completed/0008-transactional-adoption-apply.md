# Apply adoption transactionally

- Status: Completed
- Started: 2026-08-24
- Completed: 2026-08-24
- Roadmap: P1 adoption

## Goal

Add an explicit, idempotent adoption apply that registers recognized media in
music-sync-owned SQLite without changing library files or inventing identity.

## Work completed

- Added explicit `--apply --database <path>` with absolute-root enforcement.
- Registered all new media paths in one transaction as separate unresolved
  recordings and unknown-health artifacts.
- Left existing artifact rows unchanged on repeat apply.
- Kept probing and hashing separate from this initial persistence operation.
- Reported database inserts/existing rows separately from zero library-file effects.
- Added whole-batch rollback, idempotency, relative-root rejection, CLI argument, and
  filesystem-preservation tests.

## Validation evidence

- `cargo check --workspace --all-targets --all-features`: passed; the reported editor
  borrow error was not present in current source or compiler output.
- `just check`: passed with warnings-denied Clippy, 41 deterministic tests, and
  architecture enforcement.
- Static musl release build: passed.
- Debian LXC first apply inserted 1,560 recordings and artifacts into isolated test
  SQLite; the second inserted zero and observed all 1,560 existing artifacts.
- Both remote runs reported zero modified, deleted, or downloaded library files;
  production remained non-writable to `music-sync-dev`.
