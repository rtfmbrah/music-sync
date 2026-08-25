# Acquisition and recovery

## CURRENT

Successful source reconciliation creates one explicitly linked acquisition job per
provider item. Schema version 4 backfills earlier idempotency-keyed jobs and makes the
job/provider relation queryable without parsing strings. Persistence can atomically
claim the oldest explicitly pending job, increment its attempt, defer it with a
persistent event, recover abandoned running jobs to deferred, and complete only a
running job. Deferred work is excluded from timer runs until an operator explicitly
releases that job for retry.

`AcquisitionStaging` creates a stable `acquisition-staging/job-<id>` directory below
application state. Repeated preparation preserves partial files for later diagnosis
or resume.

The yt-dlp adapter can download exactly one provider URL into prepared staging with
a non-zero deadline, 16 MiB output bounds, playlist expansion disabled, a fixed
`media.%(ext)s` template, and overwrite disabled. A successful subprocess result is
accepted only when it reports exactly one absolute, non-empty regular file whose
canonical parent is the staging directory. The file remains uncommitted staging
evidence until acquisition orchestration validates and commits it.

Autonomous service construction supplies validated request and randomized download
sleep bounds to the adapter. The delay occurs inside yt-dlp and remains covered by
the bounded subprocess deadline. This pacing complements serial job processing and
reduces provider bursts without changing acquisition state transitions.

Staged-media validation composes the ffprobe and streaming SHA-256 boundaries. It
requires a non-empty regular file with a structural audio stream and rejects any
size mismatch across the pre-probe observation, hashed byte count, and post-hash
observation. Successful evidence contains codec, optional duration/sample rate/
channels, structurally valid unambiguous embedded recording MBID/ISRC values, exact
byte count, and lowercase SHA-256. Schema version 10 carries canonical values through
prepared acquisition intent and populates previously unresolved recording evidence
without overwriting contradictory identity. Validation remains read-only; a
well-formed embedded tag is evidence, not provider-object identity.

Schema version 5 persists prepared acquisition commits containing staged/final paths
and all validation evidence. Final paths derive only from bounded provider identity
components and the staged extension. Filesystem commit uses a same-filesystem hard
link, which atomically fails rather than overwriting an existing path, followed by
directory synchronization and one SQLite transaction associating the provider item,
recording, healthy artifact, committed intent, and succeeded job. Staging is retained.

An exact-byte destination is recoverable only when the same job already had matching
prepared intent before the destination was observed. Pre-existing paths—even with
identical bytes—remain unowned and untouched. A crash after the hard link but before
SQLite finalization can therefore resume without treating arbitrary existing media
as music-sync-owned.

`acquisition run-one` exposes the complete first workflow through the reusable
library and thin CLI. It claims at most one job, prepares stable staging, downloads,
validates, and commits it. Any failure after claim is recorded as a deferred job with
a persistent diagnostic. Ordinary runs leave it untouched until `acquisition retry
<job-id>` explicitly moves it back to pending; the following claim increments the
attempt. When no work remains, the command reports an explicit idle outcome.

`acquisition run-pending` snapshots up to a non-zero configured maximum of pending
job IDs, then atomically claims and attempts each ID at most once. An
ordinary per-job error is persisted/deferred and reported while unrelated jobs
continue. Database state failures stop the batch because further claims would be
unsafe. A deferred job can appear again only after explicit retry release.
`acquisition recover-running` is a separate operator action for a confirmed
abandoned process; it marks running jobs deferred, audits the transition, and retains
staging evidence.

The one-time adopted-library migration can also defer a pending job when an exact
provider ID filename candidate exists but cannot be audio-verified. This quarantine
is audited and uses the ordinary deferred state, so timer runs skip it and only an
explicit operator retry can release it. A verified fingerprint contradiction does
not quarantine the job: the provider object is then genuinely absent from the
existing artifact set and remains eligible for acquisition.

Verified repair uses the same structural validation, exact hashing, safe destination,
same-filesystem hard-link, directory-sync, and no-clobber recovery boundaries. Repair
staging is separately namespaced by durable attempt. A later explicit commit rehashes
staging before preparing intent, creates a new repair artifact path, and preserves the
old missing/corrupt path and row indefinitely.

Discovery acquisition adds a stricter assertion before prepared commit. An approved
recording MBID is routed only when MusicBrainz returns the exact recording, a
canonical duration, and exactly one recording-level supported YouTube URL. The
staged file must then carry either that exact recording MBID or the recording's sole
canonical ISRC, and its duration must be within two seconds. The relationship and
canonical values are persisted before the job is queued. Missing, ambiguous, or
contradictory evidence cannot create or commit an artifact.

## PLANNED

Every attempt uses a job-specific temporary directory on a filesystem that supports
an atomic final rename. The sequence is:

```text
reserve idempotent job -> download -> probe/decode validation
-> canonical and audio identity checks -> metadata/artwork/lyrics preparation
-> cryptographic hash + fingerprint -> fsync where required
-> atomic no-clobber commit -> transactional database success
```

Files are never written directly to their final name. A pre-existing target causes a
reconciliation decision, not blind overwrite. Restart scans persistent running jobs
and temporary space, validates any staged result, and either resumes, commits safely,
or retains evidence for diagnosis.

Preserve source Opus, AAC, or other sensible audio where possible. Remuxing is
acceptable; lossy-to-lossy transcoding purely for uniformity is not. Bounded download
concurrency and exponential backoff protect providers, while disk-space checks stop
new acquisition before exhaustion.

## NON-NEGOTIABLE

Partial media never becomes a library artifact. Filesystem commit and database state
must have an explicitly recoverable crash boundary. Discovery and replacement use
this same safe acquisition path.
