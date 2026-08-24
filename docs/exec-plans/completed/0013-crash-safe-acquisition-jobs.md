# Establish crash-safe acquisition jobs

- Status: Completed
- Started: 2026-08-24
- Completed: 2026-08-24
- Roadmap: P2 acquisition and identity

## Goal

Turn pending provider-item acquisition work into an explicit, recoverable workflow
without exposing partial media or overwriting any existing artifact.

## Scope

- Add a forward migration linking acquisition jobs to provider items explicitly.
- Create that link transactionally during successful snapshot reconciliation.
- Claim one pending job atomically and expose a strong acquisition-work type.
- Give every claimed job a stable staging directory under application state.
- Record success, deferred failure, and interrupted-process recovery explicitly.
- Add deterministic persistence and filesystem tests; no live provider calls.
- Do not commit a final library artifact until probe/hash validation and no-clobber
  filesystem commit are implemented in the following slice.

## Safety boundary

- Staging is outside the managed library and scoped by durable job ID.
- Claiming or retrying never deletes media and never interprets provider failure as
  replacement eligibility.
- Existing final paths are never overwritten.

## Work completed

- Added schema version 4 with explicit acquisition-job/provider-item linkage and
  deterministic backfill from existing provider-keyed jobs.
- Added immediate-transaction claiming, monotonic attempt counts, deferred events,
  interrupted-job recovery, and running-only completion.
- Added stable job-specific staging below application state that never erases partial
  evidence during repeated preparation.

## Validation evidence

- `just check`: passed with warnings-denied Clippy, 60 deterministic tests,
  architecture enforcement, and deployment-script syntax validation.
- Tests cover schema-v3 backfill, claim ordering, defer/retry, interruption recovery,
  terminal completion, and repeated staging preparation with preserved content.
