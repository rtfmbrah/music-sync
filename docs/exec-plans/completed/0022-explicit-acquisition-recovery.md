# Explicit acquisition history and recovery

- Status: Complete
- Started: 2026-08-24
- Roadmap: P5 operations and deployment

## Goal

Make failed and interrupted acquisition work diagnosable and operator-controlled
instead of retrying deferred work automatically on every timer invocation.

## Scope

- Keep `deferred` jobs out of ordinary runnable selection.
- Add an audited deferred-to-pending retry transition for one explicit job ID.
- Add audited recovery of abandoned `running` jobs to `deferred` without deleting
  staging evidence.
- Add bounded newest-first acquisition history through immutable read-only SQLite.
- Add text/JSON CLI commands and deterministic transition/black-box tests.
- Validate recovery behavior with static fixtures in isolated LXC storage.

## Safety boundary

- Succeeded jobs cannot be retried and committed artifacts are never modified.
- Recovery never assumes a running process is abandoned automatically; it is an
  explicit operator command intended only when no sync process is active.
- Retry retains stable staging evidence and increments attempts only on the next claim.

## Result

- Deferred acquisitions are excluded from ordinary runnable snapshots.
- Added audited one-job retry and explicit abandoned-running recovery without staging
  deletion.
- Added bounded immutable acquisition history with provider identity, attempts,
  timestamps, state, and latest warning/error context.
- Updated deterministic workflow tests for explicit release semantics; `just check`
  passes with 77 tests.
- Static-musl LXC validation proved deferred work remains idle until explicit retry,
  then commits only the released job while production music stays read-only.
