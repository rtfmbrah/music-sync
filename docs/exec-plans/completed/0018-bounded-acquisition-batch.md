# Process a bounded acquisition batch

- Status: Completed
- Started: 2026-08-24
- Completed: 2026-08-24
- Roadmap: P2 acquisition and identity

## Goal

Process the currently runnable acquisition set without requiring one CLI invocation
per track and without allowing one failure to block unrelated jobs.

## Scope

- Snapshot at most a user-specified number of pending/deferred job IDs.
- Atomically claim each snapshotted job at most once during the batch.
- Reuse the complete one-job staging/validation/commit workflow.
- Continue after ordinary per-job failure and return structured counts/diagnostics.
- Stop on database state failures that make further claims unsafe.
- Add `acquisition run-pending --max-jobs` text/JSON CLI behavior.
- Add deterministic mixed success/failure/idempotency tests.

## Safety boundary

- Batch size is non-zero and explicitly bounded.
- A deferred failure is not immediately retried again in the same batch.
- One failed job never blocks unrelated acquisition work.

## Work completed

- Added stable bounded snapshots of runnable acquisition job IDs and atomic
  claim-by-ID persistence behavior.
- Added reusable batch orchestration that attempts each selected job once, continues
  after ordinary deferred failures, and stops for unsafe database failures.
- Added `acquisition run-pending --max-jobs` text/JSON CLI output with selected,
  committed, failed, and skipped outcomes.

## Validation evidence

- `just check`: passed with warnings-denied Clippy, 74 deterministic tests,
  architecture enforcement, and all shell-fixture syntax checks.
- Black-box mixed batch committed jobs before and after one failed provider item,
  attempted that failure once, and selected only the deferred job next time.
