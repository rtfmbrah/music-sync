# Run one acquisition end to end

- Status: Completed
- Started: 2026-08-24
- Completed: 2026-08-24
- Roadmap: P2 acquisition and identity

## Goal

Expose the first complete reusable/CLI workflow from a pending provider item to one
validated, healthy managed artifact.

## Scope

- Compose claim, staging, yt-dlp download, ffprobe, SHA-256, no-clobber commit, and
  SQLite finalization in the reusable library.
- Defer a claimed job with a persistent diagnostic after any adapter, validation,
  filesystem, or commit failure.
- Add a thin `acquisition run-one` CLI command driven by the normal config file.
- Report idle, committed, and failure outcomes in text/JSON.
- Add deterministic black-box success, idle-repeat, and deferred-failure tests.
- Validate with a static binary only in isolated LXC test storage.

## Safety boundary

- The workflow processes at most one job per invocation.
- Failure never exposes partial media in the managed library.
- Existing targets remain untouched and a failed job stays retryable.

## Work completed

- Added reusable one-job orchestration for claim, stable staging, yt-dlp download,
  ffprobe, SHA-256, prepared no-clobber commit, and transactional finalization.
- Added automatic deferred state plus persistent diagnostic for every post-claim
  failure and monotonic retry attempts.
- Added the thin text/JSON `acquisition run-one` CLI command driven by normal config.

## Validation evidence

- `just check`: passed with warnings-denied Clippy, 73 deterministic tests,
  architecture enforcement, and deployment-script syntax validation.
- Black-box CLI test passed failure/defer, attempt-two commit, preserved staging, and
  idle repeat entirely through executable fixtures.
- Static-musl Debian 12 LXC test committed one fixture artifact under isolated test
  storage, persisted recording/artifact ID 1, then reported idle. Production music
  remained read-only and no live provider was contacted.
