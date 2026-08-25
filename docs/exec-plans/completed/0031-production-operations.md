# Production service orchestration and operations

- Status: Complete
- Started: 2026-08-26
- Roadmap: P5 operations and deployment

## Goal

Run every autonomous phase through one bounded, mutually exclusive, observable
service cycle with durable history, truthful exit status, safe crash recovery, and
least-privilege Debian LXC packaging.

## Safety boundary

- No intermediate production installation or mutation of `/srv/music`.
- One service cycle may write only configured music-sync state and managed outputs.
- Concurrent service cycles are rejected before provider or filesystem effects.
- Every phase has a durable running and terminal state with bounded diagnostics.
- A failed independent phase does not erase successful work or starve later safe
  phases; unsafe dependent phases are skipped explicitly.
- Crash recovery never retries destructive work implicitly.
- Secrets remain outside tracked configuration and command output.

## Tasks

- [x] Add schema-backed service runs, phases, metrics, history, and filters.
- [x] Add exclusive service-cycle locking and explicit interrupted-run recovery.
- [x] Compose source, acquisition, health/fingerprint, repair, enrichment, playlist,
  and discovery phases with explicit dependencies and bounds.
- [x] Expand offline doctor for tools, permissions, filesystem atomicity, storage,
  secrets, and service readiness.
- [x] Add structured progress/event output suitable for terminals and systemd.
- [x] Add hardened systemd units, environment contract, backup, rollback, and
  versioned release installation tooling.
- [x] Add deterministic service scenarios and performance regression gates.
- [x] Run canonical checks and complete isolated Debian LXC service acceptance.
