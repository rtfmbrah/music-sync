# Observability and operations

## CURRENT

The CLI initializes `tracing` and maps default, `-v`, `-vv`, and `-vvv` to warning,
info, debug, and trace. `doctor` emits stable named checks in English and optionally
pretty JSON. Database tables reserve persistent runs, jobs, and contextual events.
Deferred acquisition attempts now persist a warning event and retain their attempt
count; abandoned running acquisitions can be made explicitly retryable on restart.
The one-job workflow emits structured idle/committed JSON including durable job,
attempt, provider item, final path, filesystem effect, recording, and artifact IDs.
Bounded batch output reports selected, committed, skipped, and per-job failure
diagnostics in both text and JSON.
`sync run` reports per-source outcomes plus aggregate acquisition and playlist phase
results. It returns a nonzero status for isolated failures only after all safe phases
have completed, making timer alerts truthful without turning one failure into global
work starvation.
`status` opens current-schema SQLite state through an immutable read-only connection
and reports source, membership, job, artifact-health, playlist-output, and bounded
recent warning/error counts in text or JSON. It creates no WAL/shared-memory files
and performs no provider or managed-filesystem access. Because SQLite immutable mode
deliberately ignores concurrent WAL changes, operators run it between oneshot sync
invocations rather than concurrently with an active writer.
`acquisition history` provides bounded newest-first job/provider state with attempt
counts and the latest persisted warning/error. Deferred jobs require audited
`acquisition retry`; ordinary timer runs leave them untouched. Explicit
`acquisition recover-running` converts confirmed abandoned claims to deferred while
retaining staging evidence.

## PLANNED

Interactive output shows real phase counts for source enumeration, reconciliation,
metadata, downloads, artwork, lyrics, verification, and discovery. `--no-progress`
and JSON event output support systemd/non-terminal use. Persistent events include
timestamp, level, run/component, recording/source/job identifiers, event name,
message, and structured decision evidence.

Expanded run-level history and failure filtering remain planned. Candidate and
discovery logs explain both scores
and final decisions, for example metadata compatibility followed by fingerprint
mismatch and rejection. This is one local application, not a distributed queue.

An internal scheduler remains deferred. One failing job is recorded and isolated
rather than terminating unrelated work.
