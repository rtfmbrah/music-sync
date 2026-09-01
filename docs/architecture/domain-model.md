# Domain model

## CURRENT

The initial SQLite migration keeps identity and operational concepts separate:

- `recordings`: canonical musical recordings, optionally carrying MBID/ISRC;
- `provider_items`: remote objects, source metadata, availability, and an optional
  recording association;
- `artifacts`: physical files, health, hashes/fingerprints, and media properties;
- `collections` and `collection_memberships`: playlists independent of artifacts;
- `sync_runs`, `jobs`, and `events`: persistent operational state.

The schema is intentionally small and migrations are embedded, ordered, and applied
inside a transaction. SQLite foreign keys are enabled. Domain states use constrained
values rather than unconstrained state strings.

Explicit library adoption inserts one unresolved `recordings` row per newly observed
artifact and an `artifacts` row keyed by its absolute path. No filename-derived
identity is stored. Repeated registration leaves existing artifacts unchanged, and
the complete batch commits or rolls back together.

Schema version 2 adds `sources`: durable provider/url configuration with an optional
user name and explicit active state. Provider plus URL is unique. Removing a source
deactivates its row; adding the same URL is idempotent and reactivates it while
preserving its durable ID.

Schema version 3 links each configured source to its durable collection. Successful
snapshots upsert provider items, reconcile ordered active memberships, create at most
one acquisition job per provider item, and record a succeeded sync run atomically.
Absent snapshot items retain their provider rows and are membership-inactive only.

Schema version 4 adds an explicit one-to-one acquisition-job/provider-item relation,
including migration backfill for existing provider-keyed jobs. Claiming is serialized
with an immediate transaction; attempts and pending/running/deferred/succeeded states
are durable rather than inferred from staging files.

Schema version 5 records prepared/committed artifact intent with staged and final
paths plus validated hash/media evidence. Only committed intent may atomically create
a healthy artifact row, associate the provider item with its recording, and mark the
job succeeded. This table is the durable filesystem/SQLite crash boundary.

Schema version 6 gives each recording an explicit preferred artifact and records
owned playlist output paths with the SHA-256 of their last committed bytes. Existing
recordings are backfilled only when exactly one healthy artifact is unambiguous.
Initial acquisitions set preference only when none exists.

Schema version 7 stores one raw algorithm-2 Chromaprint observation per artifact,
including its extraction-length bound, reported duration, value count, serialized
values, and refresh timestamp. It remains artifact evidence rather than a recording
identifier and is retained independently from provider availability.

Schema version 8 adds durable `repair_cases` and `repair_attempts`. Cases reference
the canonical recording, permanently unavailable original provider object, and the
preserved unhealthy reference artifact. Attempts retain provider candidates and an
explicit generated/rejected/unresolved/verified/committed state; candidate search is
not folded into acquisition or recording identity.

Schema version 9 expands repair attempts with exclusive running/deferred execution,
monotonic attempt counts, complete staged validation/fingerprint/decision evidence,
and prepared/committed artifact intent. A committed replacement adds a new healthy
artifact and changes preference; it does not mutate or delete the historical
missing/corrupt artifact row.

Schema version 10 retains unambiguous embedded recording MBID and normalized ISRC
through crash-safe acquisition intent. Initial acquisition may populate an unresolved
recording with those values; repeated acquisition never silently overwrites a
contradictory canonical association.

Schema version 11 adds canonical artists/releases and their recording relationships,
plus append/idempotent field observations, explicit selections, and durable
resolved/ambiguous/deferred state. Fixed-point confidence and resolution context are
stored per field; source provider payloads remain separate.

Schema version 12 stores canonical release-artwork resolution, selected provider
provenance, and immutable SHA-256-addressed blobs in application state.

Schema version 13 stores lyrics observations, explicit selection, resolution state,
and prepared/committed adjacent-output intent. Automatic sidecars are limited to
healthy artifacts proven owned by a committed acquisition or repair. Adopted media
remains read-only.

Schema version 14 stores reserved hidden staging, prepared/committed canonical tag
materialization, immutable original-byte history, explicit deferral, and the derived
preferred artifact. The former artifact row moves to its retained history path; it is
not deleted or collapsed into the derived container.

Schema version 17 stores migration-only adopted/provider verification attempts.
Filename tokens generate candidates but never establish identity. Retained provider
staging, exact hash, duration, raw fingerprint comparison, attempt count, and terminal
decision are auditable. Only a verified duration and perceptual match atomically
associates the provider item with the existing unresolved recording and satisfies its
pending acquisition job; no artifact row or audio path is replaced.

Schema version 18 stores bounded artifact-fingerprint deferrals with their extraction
bound, attempt count, diagnostic, and update time. Timer runs skip a matching
deferral until explicit operator retry, while a different extraction bound remains
independently eligible.

Schema version 19 stores ordered entries preserved from an adopted named playlist.
Known managed artifacts are deduplicated against active collection membership;
unknown existing entries remain byte-path-preserved across later materialization.
Playlist output ownership may move from the legacy internal-ID filename only after
the old file matches its recorded SHA-256.

Schema version 20 stores provenance-aware provider display enrichment separately
from canonical identity. One row per provider item retains the display title, an
optional display artist with its source-field provenance (`artist`, `creator`,
`channel`, or `uploader`), an explicit provider album, release date, explicit
non-generic genres as JSON, a secure HTTPS thumbnail URL, snapshot-versus-full
completeness, and the bounded raw payload. It also stores one selected
provider-thumbnail artwork per recording plus per-recording thumbnail resolution
state. Provider enrichment is display fallback evidence only; it never establishes
recording identity and never overrides canonical selections.

Schemas 22–23 store one durable external genre-resolution state per recording
and selected genre rows carrying source, source entity, fixed-point confidence, and
recording or artist scope. Genre evidence does not establish recording identity. Retag scheduling is
transactional with a newly resolved selection.

## PLANNED

Add discovery seeds/candidates and expanded job attempts when a real workflow
requires each concept. A recording can belong to several releases; a
single preferred artifact can satisfy multiple provider items and memberships.

Metadata fields carry source, confidence, and resolution context rather than a
single mutable provider-owned row. Filesystem and SQLite state are reconciled
explicitly; filenames are never the entire database.

## NON-NEGOTIABLE

Provider identity, musical identity, releases, artifacts, and membership are never
collapsed. Schema changes use forward migrations. Important state transitions are
transactional and idempotent.
