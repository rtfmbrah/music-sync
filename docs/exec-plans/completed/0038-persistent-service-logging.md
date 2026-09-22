# Persistent service logging and startup hardening

- Status: Complete
- Started: 2026-09-22
- Roadmap: P5 operations and deployment

## Goal

Preserve a complete local log for every autonomous service invocation, retain prior
logs with sortable UTC timestamps, and turn known fatal configuration mistakes into
specific startup diagnostics.

## Safety boundary

- Logs never contain secret values or cookie contents.
- Rotation preserves every non-empty completed log and never overwrites an archive.
- Logging remains inside the configured state directory.
- Disabled integrations cannot require credentials or contact providers.
- Retry policy remains explicit; logging changes do not make deferred work run
  automatically.

## Tasks

- [x] Add `logs/current.log` for complete and core service invocations.
- [x] Rotate non-empty prior logs to `yyyy-mm-dd-hh-mm-ss-music-sync.log` in UTC.
- [x] Prevent overlapping processes from rotating the active owner's log.
- [x] Keep file phase summaries at information level when terminal progress is quiet.
- [x] Validate service endpoints with actionable configuration errors.
- [x] Do not initialize AcoustID when autonomous discovery is disabled.
- [x] Add deterministic coverage and run canonical repository checks.
- [x] Update operations, testing, roadmap, and deployment documentation.
- [x] Correct the production-discovered `fpcalc` exit-3 end-of-file edge case without
      weakening output validation or accepting other subprocess failures.
