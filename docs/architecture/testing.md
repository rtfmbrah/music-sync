# Testing strategy

## CURRENT

- Unit tests cover configuration constraints, migrations, preservation decisions,
  conservative identity policy, read-only diagnostics, and adoption classification.
- Filesystem tests prove adoption classification and zero side effects, including
  preservation of unknown files and non-followed symbolic links. They also cover
  same-stem lyrics, artwork locality, resolved/missing/external playlist references,
  invalid UTF-8 playlists, and unchanged playlist contents.
- Subprocess fixtures cover ffprobe parsing, nonzero exits, process deadlines, and
  deterministic adoption probe limits without requiring installed media tools.
- Parser, subprocess, aggregation, and black-box CLI fixtures distinguish absent,
  valid, and malformed recording MBID/ISRC tags without live metadata services.
- Known SHA-256 vectors and filesystem tests cover streaming byte counts,
  deterministic hash limits, exact-duplicate aggregation, and zero file effects.
- SQLite and black-box CLI tests cover explicit adoption apply, whole-batch rollback,
  absolute-root enforcement, repeat idempotency, and media/unknown-file preservation.
- Stored yt-dlp video/playlist/error fixtures cover typed parsing, raw metadata,
  subprocess deadlines, output bounds, conservative failure classes, and CLI output
  without live provider access.
- Schema-v1 migration, persistence, and black-box CLI tests cover durable source
  add/list/deactivate/reactivate behavior and URL rejection before database creation.
- Persistence and black-box subprocess tests cover transactional source snapshot
  reconciliation, repeat idempotency, membership-only removal, provider mismatch
  rollback, and transient enumeration failure without membership mutation.
- Schema-v4 backfill and persistence tests cover exclusive acquisition claims,
  monotonic attempt counts, deferred events, interrupted recovery, terminal success,
  and stable staging that preserves partial evidence.
- Deterministic yt-dlp download fixtures cover one contained non-empty staged file,
  rate-limit failure, missing/relative output, canonical path escape, empty media,
  and process timeout without live provider access.
- Staged-validation tests cover combined structural properties and a known SHA-256,
  ffprobe rejection, hash byte-count mismatch, unchanged failed evidence, and
  non-empty regular-file requirements.
- Artifact-commit tests cover atomic no-clobber creation, idempotent committed state,
  interruption after filesystem commit, different-byte collision, identical but
  unowned pre-existing media, unsafe provider paths, and symlink destination escape.
- Black-box CLI coverage executes source creation/reconciliation followed by a failed
  acquisition, deferred retry at attempt two, validated artifact commit, and idle
  repeat using only executable fixtures.
- Mixed batch coverage proves two successful jobs commit around one provider failure,
  the failure is attempted only once per batch, and only that deferred job is selected
  by the following batch.
- Playlist workflow coverage proves ordered creation, unchanged repetition, atomic
  update after membership removal, preservation of removed audio, and safe adoption
  of an existing named output without dropping unknown entries.
- Named-playlist migration coverage proves correct paths relative to a sibling
  playlist directory, adoption of existing unknown entries, deduplication of known
  managed entries, exact-hash retirement of a legacy internal-ID output, unchanged
  repetition, and preservation across later membership removal.
- Full sync CLI coverage proves all phases complete in one invocation, repeats avoid
  downloads, and one transient source failure preserves its prior membership while
  unrelated reconciliation and playlist work continue.
- Status black-box coverage creates a deferred acquisition and proves typed counts
  and its warning event are reported without changing database bytes, SQLite sidecar
  files, library files, or playlist files.
- Simplified-CLI black-box coverage proves the primary help hides technical command
  groups while keeping them callable, and validates `list --full` precedence across
  success, pending, failed, missing, and copyright evidence. A healthy local artifact
  remains successful despite permanent remote state. Combined status filters use OR
  semantics, source selectors accept configured names and durable IDs, and filtered
  JSON excludes nonmatching members.
- Duplicate-report unit and black-box coverage separates exact-byte groups from
  strict audio-match candidates and proves immutable execution leaves database bytes,
  SQLite sidecars, and media unchanged.
- Acquisition transition coverage proves deferred jobs are not selected by ordinary
  runs, explicit retry is audited and attempted later, succeeded work cannot be
  released again, abandoned running recovery retains staging, and immutable history
  reports bounded newest-first provider/job failure context.
- Artifact-health tests prove stable bounded checking, healthy/missing/hash-mismatch
  classification, unchanged media bytes, persistent corrupt evidence, and prior
  healthy-state preservation when probing fails.
- Raw fingerprint tests cover bounded fpcalc arguments/output/deadlines, schema-v7
  persistence, repeat idempotency, automatic sync extraction, isolated failures, and
  strict aligned/duration-aware comparison without live services. Subprocess fixtures
  also prove that the exact exit-3 end-of-file diagnostic may carry valid JSON while
  malformed or empty output and every other non-zero exit remain failures.
- Fingerprint deferral tests prove failed artifacts do not starve later work, ordinary
  repeats skip the same bound, and explicit operator retry releases only the selected
  artifact.
- Repair persistence tests prove transient failures cannot create cases, every
  permanent-loss/health/membership/fingerprint prerequisite is required, generated
  candidates remain unverified and idempotent, recovered health cancels a case, and
  renewed loss can reopen it.
- Schema-v9 and black-box repair tests cover migration of generated candidates,
  exclusive claim and evidence state, real staged download/probe/hash/fingerprint
  composition, canonical/duration/perceptual verification, rejection of staging
  mutation, atomic no-clobber commit, old-path preservation, and idempotent recovery.
- Canonical metadata tests cover MusicBrainz recording and multi-recording ISRC JSON,
  exact MBID and unique-ISRC HTTP paths, required User-Agent, loopback-only test HTTP,
  deterministic release selection, transactional artist/release/field provenance,
  explicit selections, repeat idempotency, and ambiguous results without live API use.
- Canonical tag tests enforce literal ffmpeg stream-copy arguments, canonical field and
  Opus picture-block routing, source/result structural comparison, immutable original
  bytes, no-clobber hidden staging, atomic preferred-artifact transition, repeat
  idempotency, and recovery after visible replacement precedes SQLite finalization.
- Scenario tests encode removed membership, disappeared remote with healthy local
  media, transient failure, same-title fingerprint mismatch, and repeat idempotency.
- The architecture check reads Cargo metadata and enforces CLI-to-library dependency
  direction.
- Deployment shell scripts receive offline syntax validation; their network-dependent
  downloads are exercised explicitly outside the canonical suite.
- CLI subprocess tests prove fatal startup diagnostics reach `logs/current.log`, a
  repeat preserves the prior bytes under the UTC timestamp naming policy, malformed
  Markdown endpoint values are rejected specifically, and disabled discovery does
  not require an AcoustID secret.
- File-log unit coverage proves an overlapping process cannot rotate the active
  owner's `current.log`.
- `just check` formats, lints with warnings denied, runs all tests, and checks the
  dependency boundary and deployment-script syntax without live services.

## PLANNED

Add filesystem/SQLite integration tests for adoption evidence reconciliation and
additional interruption points. Scenario coverage grows only alongside real
behavior: one artifact in several playlists, discovery deduplication, qualifiers,
duration, crash points, disk limits, and interrupted commits.

Live YouTube, MusicBrainz, Cover Art Archive, LRCLIB, AcoustID, ListenBrainz, and
Navidrome tests are explicit, opt-in, and never part of `just check`. Security audits
are separate because advisory refresh requires a network. Tests assert observable
state transitions and side effects, never constants or empty interfaces.

## CI contract

Any CI system enters `nix develop -c just check` (or uses the pinned Rust toolchain
and invokes `just check`). The repository is not currently configured for a specific
hosting provider, so no vendor-specific workflow is assumed.
