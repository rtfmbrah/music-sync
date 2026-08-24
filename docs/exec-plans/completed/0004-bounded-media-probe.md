# Add bounded media probing

- Status: Completed
- Started: 2026-08-23
- Completed: 2026-08-23
- Roadmap: P1 adoption

## Goal

Add explicit, read-only ffprobe inspection to adoption without slowing the default
scan or turning probe failures into destructive decisions.

## Work completed

- Added typed media properties and a narrow ffprobe subprocess boundary.
- Bounded each process by a monotonic deadline and 1 MiB stdout/stderr capture.
- Added opt-in `--probe`, deterministic `--max-probes`, and configurable timeout.
- Aggregated codec, duration, artwork, basic-tag, and canonical-ID presence.
- Kept tag values out of reports and classified every probe failure as an issue.
- Added pure parsing, fake-probe, subprocess success/failure, and timeout tests.
- Rejected a signal-based timeout dependency after its sandbox-incompatible behavior
  was exposed by tests; replaced it with safe portable `try_wait` polling.

## Validation evidence

- Clippy with warnings denied: passed.
- Deterministic Rust suite: passed (25 tests).
- 25-file Debian sample: 25 successes, known durations, AAC, artwork/basic tags, zero
  canonical IDs, zero probe failures.
- Full Debian run: 1,560/1,560 successful, zero timeouts/failures, all AAC, all basic
  tags, 1,557 embedded artwork, zero inspected canonical IDs, 522,373,526 ms total.
- Both remote runs reported zero modified/deleted/downloaded files and
  `production_writable=no`.
