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

## PLANNED

Interactive output shows real phase counts for source enumeration, reconciliation,
metadata, downloads, artwork, lyrics, verification, and discovery. `--no-progress`
and JSON event output support systemd/non-terminal use. Persistent events include
timestamp, level, run/component, recording/source/job identifiers, event name,
message, and structured decision evidence.

Run/job/attempt transitions permit status, history, failure listing, and explicitly
safe retry commands after crashes. Candidate and discovery logs explain both scores
and final decisions, for example metadata compatibility followed by fingerprint
mismatch and rejection. This is one local application, not a distributed queue.

External systemd timers or cron invoke deterministic `sync`/`run` commands first; an
internal scheduler is deferred. One failing job is recorded and isolated rather than
terminating unrelated work.
