# Canonical metadata and provenance

- Status: Complete
- Started: 2026-08-25
- Completed: 2026-08-25
- Roadmap: P3 metadata enrichment

## Goal

Resolve strong recording identities through MusicBrainz and retain canonical artist,
release, and field-level metadata without last-writer-wins mutation.

## Safety boundary

- Titles never create canonical recording identity.
- Network, rate-limit, malformed response, and ambiguous ISRC results retain prior
  metadata and defer or remain ambiguous.
- Observations are append/idempotent provenance; selection is explicit and
  transactional.
- Metadata enrichment does not rewrite audio.

## Result

- Schema version 11 separates canonical entities, relationships, field observations,
  selections, and durable resolution state.
- The bounded MusicBrainz client enforces response/deadline/User-Agent/HTTPS/pacing
  policy and retains raw response evidence.
- Exact MBID and unique ISRC resolution persist selected title, artist credit,
  release, and release date without rewriting audio or provider payloads.
- Offline parser/HTTP/SQLite/CLI tests and isolated static LXC validation pass.
