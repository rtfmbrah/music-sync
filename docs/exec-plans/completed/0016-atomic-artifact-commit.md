# Commit validated artifacts without clobbering

- Status: Completed
- Started: 2026-08-24
- Completed: 2026-08-24
- Roadmap: P2 acquisition and identity

## Goal

Commit one validated staged download into managed library storage and SQLite without
ever overwriting an existing path or losing the crash boundary between filesystem
and database state.

## Scope

- Add a forward migration for durable prepared/committed acquisition intent.
- Derive a conservative provider-ID destination below the managed library.
- Persist intended final path and validation evidence before filesystem commit.
- Commit with an atomic same-filesystem, no-clobber operation.
- Finalize provider-item recording association, healthy artifact evidence, and job
  success in one SQLite transaction.
- Make prepared commits safely inspectable/retryable after process interruption.
- Add deterministic success, collision, mismatch, repeat, and crash-window tests.
- Do not delete staged evidence automatically in this slice.

## Safety boundary

- Existing destination paths are never overwritten, truncated, renamed, or deleted.
- Exact-byte equality can resume a previously prepared commit but never replaces a
  different existing file.
- A database job succeeds only after the final path contains the validated bytes.

## Work completed

- Added schema version 5 for prepared/committed paths and validation evidence.
- Added conservative provider-ID destinations and rejected unsafe components,
  extensions, relative roots, and symlink-resolved destination parents.
- Added same-filesystem atomic hard-link commit with file/directory synchronization
  and no overwrite behavior.
- Added transactional recording, provider association, healthy artifact, commit, and
  job-success finalization plus idempotent crash recovery.
- Restricted exact-byte recovery to matching intent that predates the observed final
  path; arbitrary existing media remains unowned even when byte-identical.

## Validation evidence

- `just check`: passed with warnings-denied Clippy, 72 deterministic tests,
  architecture enforcement, and deployment-script syntax validation.
- Tests simulate success, full repeat, filesystem-before-database interruption,
  differing collision, identical unowned collision, unsafe identity, and symlink
  escape while proving staged and existing bytes remain preserved.
