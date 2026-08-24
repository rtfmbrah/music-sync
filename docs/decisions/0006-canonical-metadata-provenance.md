# ADR 0006: Strong canonical lookup and field-level provenance

- Status: Accepted
- Date: 2026-08-25

## Context

Provider titles and embedded tags are useful evidence but can be wrong. Canonical
metadata providers may disagree, return multiple recordings for one ISRC, or change
individual fields independently. A last-writer-wins recording row would erase both
provenance and resolution context.

## Decision

Automatic canonical lookup starts only from a structurally valid recording MBID or
normalized ISRC. Exact recording MBID is preferred; ISRC resolves automatically only
when exactly one returned recording contains it. Names and titles never establish
recording identity.

Store canonical artists, releases, and relationships separately. Store each selected
metadata field as a provenance observation with source entity, fixed-point confidence,
resolution context, and time, then reference it through an explicit selection.
Ambiguous and retryable results are durable states that do not alter prior selections.

Use a bounded in-process HTTPS client for metadata/artwork/lyrics APIs. MusicBrainz
requests identify the application, honor its one-call-per-second policy, have an
explicit deadline and response-size limit, and retain bounded raw JSON for audit.

## Consequences

Metadata writes are more relational than a mutable tag blob, but conflicts and
provider evolution remain explainable. Additional providers add observations rather
than owning recording rows. Offline tests exercise controlled loopback HTTP and stored
fixtures; live services are always opt-in.
