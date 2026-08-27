# Observability and operations

## CURRENT

The CLI initializes `tracing` and maps default/`-v`, `-vv`, and `-vvv` to info,
debug, and trace. `doctor` emits stable named checks in English and optionally
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

Schema version 16 adds mutually exclusive service-cycle history. `sync run` reserves
one durable cycle before contacting providers and records ordered source,
acquisition, fingerprint, and playlist phases with exact JSON summaries. A second
cycle is rejected while one remains running. Successful and partial cycles finish
truthfully; fatal errors finish the active phase and cycle as failed. `runs history`
is immutable, bounded, newest-first, and supports exact state filtering. `runs show`
returns ordered phase evidence. `runs recover-interrupted` is the only transition for
an operator-confirmed abandoned cycle and never retries work implicitly.

`service run` is the complete headless cycle. It uses the same exclusive durable run
for core source synchronization, discovery/taste import and verified acquisition,
artifact health, post-acquisition fingerprinting, conservative repair
assessment/execution, canonical metadata, artwork, lyrics, source-preserving tags,
and final playlists. Verified repair attempts are rehashed and committed
automatically; rejected, unresolved, deferred, and commit failures remain isolated
and auditable. A repeat retries previously verified but uncommitted safe repair work.

`events` performs immutable newest-first queries with exact severity, component,
run, and job filters plus a hard result bound. It never creates SQLite sidecars or
contacts a provider.

Human-oriented commands show durable service run/phase transitions on stderr at the
default information level. `--no-progress` suppresses them, while `--json`
automatically keeps stdout machine-readable and stderr quiet unless verbosity is
explicitly raised. The same structured `tracing` events are captured by journald for
the systemd oneshot.

`maintenance backup` uses SQLite `VACUUM INTO` to create a consistent, uniquely
named snapshot in an existing operator-provisioned directory. It never overwrites a
snapshot and does not delete old backups; retention remains an explicit operator
policy.

The public operator help is deliberately small: `list`, `add`, `remove`,
`duplicates`, `backup`, `doctor`, `sync`, and `status`. Existing subsystem commands
remain callable but hidden from primary help for deployment and recovery
compatibility. Bare `sync` runs the same complete cycle as `service run`; the older
`sync run` core cycle remains available. `status history` exposes durable service
history and `status watch` follows the systemd unit journal.

`list` reads configured playlist and single-video sources immutably. `list --full`
adds active members in provider order with provider ID, proven canonical
artist/title when available, and a strict operator state. Healthy preferred local
audio always wins as `success`; new/running work is `pending`; explicit persisted
copyright and permanent-unavailability diagnostics become `copyright` and
`missing`; other deferred or terminal work is `failed` with its stored diagnostic.
Provider titles are retained rather than split to guess artist identity.

`just test-performance` runs the complete deterministic offline service cycle twice,
checks idempotency and exclusivity, and fails if execution exceeds a deliberately
generous ten-second regression ceiling. Build time is excluded. Target-LXC acceptance
also records real total timing before production activation.

## PLANNED

Richer per-item interactive counters remain planned. Persistent events include
timestamp, level, run/component, recording/source/job identifiers, event name,
message, and structured decision evidence.

Candidate and discovery logs explain both scores
and final decisions, for example metadata compatibility followed by fingerprint
mismatch and rejection. This is one local application, not a distributed queue.

An internal scheduler remains deferred. One failing job is recorded and isolated
rather than terminating unrelated work.
