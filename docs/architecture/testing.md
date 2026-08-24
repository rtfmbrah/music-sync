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
  update after membership removal, preservation of removed audio, and rejection of
  an unknown existing output without changing its bytes.
- Scenario tests encode removed membership, disappeared remote with healthy local
  media, transient failure, same-title fingerprint mismatch, and repeat idempotency.
- The architecture check reads Cargo metadata and enforces CLI-to-library dependency
  direction.
- Deployment shell scripts receive offline syntax validation; their network-dependent
  downloads are exercised explicitly outside the canonical suite.
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
