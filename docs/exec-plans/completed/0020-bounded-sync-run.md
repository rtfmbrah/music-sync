# Bounded operational sync run

- Status: Complete
- Started: 2026-08-24
- Roadmap: P5 operations and deployment

## Goal

Expose one deterministic, bounded command that reconciles every active source,
attempts a bounded acquisition snapshot, and materializes playlists for timer-driven
operation.

## Scope

- Add a library orchestration boundary with structured phase results.
- Isolate per-source enumeration/reconciliation failures and continue other sources.
- Reuse the existing bounded acquisition batch and atomic playlist materialization.
- Add `sync run` text/JSON CLI behavior and meaningful exit status.
- Add deterministic black-box coverage for successful repeat and isolated failure.
- Validate the complete command using offline fixtures in isolated LXC storage.
- Document a least-privilege systemd service/timer example without installing it.

## Safety boundary

- A provider failure never commits an empty snapshot or removes membership.
- A failed source or job does not block unrelated work.
- The command never deletes audio or overwrites unknown output.
- Every invocation has an explicit non-zero acquisition limit and subprocess timeout.

## Result

- Added reusable bounded orchestration and `sync run` text/JSON CLI behavior.
- Per-source provider failure is isolated before membership mutation; ordinary job
  and playlist failures are aggregated into exit status 1 after safe work completes.
- Added a black-box workflow test covering a successful complete run, idempotent
  acquisition behavior, and one isolated transient source failure.
- Added uninstalled least-privilege systemd service/timer examples. Local
  `systemd-analyze verify` parsed them and reached the expected missing deployment
  executable; complete verification remains deployment-time work after path setup.
- `just check` passed with 76 deterministic tests.
- Static-musl LXC validation completed all phases and proved an unchanged repeat in
  isolated storage while production music remained read-only.
