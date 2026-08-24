# Read-only operational status

- Status: Complete
- Started: 2026-08-24
- Roadmap: P5 operations and deployment

## Goal

Provide one read-only text/JSON command that truthfully summarizes durable source,
membership, artifact, acquisition, playlist, and recent failure state.

## Scope

- Add a reusable persistence query returning typed aggregate counts.
- Report every constrained job and artifact-health state explicitly.
- Count unresolved active memberships without guessing recording identity.
- Include a bounded recent warning/error list for operator diagnosis.
- Add `status --config` text/JSON CLI output and deterministic tests.
- Validate the static binary against isolated LXC workflow state.

## Safety boundary

- Status performs no provider request and no filesystem mutation.
- Counts come from durable state rather than inferred filenames.
- Recent diagnostics are bounded and ordered newest first.

## Result

- Added a current-schema immutable SQLite status query with typed source,
  membership, job, artifact-health, playlist-output, and recent-event results.
- Added bounded `status --config` text/JSON CLI output with no provider access.
- Black-box coverage proves a deferred job and its event are reported without any
  database-byte, SQLite-sidecar, library, or playlist mutation.
- `just check` passed with 77 deterministic tests.
- Static-musl LXC validation reported completed workflow state while preserving the
  exact database hash and state-directory file set; production music stayed read-only.
