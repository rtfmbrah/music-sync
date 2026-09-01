# External genre enrichment (complete)

## Goal

Enrich owned recordings with useful Internet genre evidence without turning title
search into recording identity, then losslessly rematerialize tags for Navidrome.

## Safety and design

- MusicBrainz is the only open, unauthenticated external genre source; proprietary
  catalog fallbacks are
  intentionally excluded.
- Exact recording MBIDs are preferred. Otherwise title, complete artist credit,
  duration, perfect provider score, and version qualifiers must all agree.
- Recording genres are retained with provider entity, confidence, and scope.
- When no recording genre exists, a unique perfect-score exact-name MusicBrainz
  artist may supply lower-confidence artist-scoped genres.
- Existing explicit provider genres are merged, never erased.
- No match, multiple matches, and no genre evidence remain explicit durable states.
- A new genre schedules stream-copy tag materialization; it never downloads or
  transcodes audio and retained artifact history remains intact.

## Work

- [x] Schema 22 genre resolution and provenance tables.
- [x] Bounded/paced MusicBrainz exact and search adapter.
- [x] Conservative identity gate and durable isolated failure states.
- [x] Service phase, operational status, and tag-materialization integration.
- [x] Deterministic matching, persistence, merge, and orchestration tests.
- [x] Bounded exponential retry for MusicBrainz 429/502/503/504 responses.
- [x] Schema 23 exact-artist fallback and safe release of prior empty results.
- [x] Canonical repository verification: `just check`, including 146 deterministic
  tests, Clippy with warnings denied, architecture, deployment, and performance gates.
- [x] Production migration and live acceptance: release
  `0.1.0-20260901.15`, schema 23, 4 recordings resolved with 16 artist-scoped
  MusicBrainz genres, and all values verified in the preferred Opus files with
  `ffprobe`. Seventeen recordings have no safe open-data result; one MusicBrainz 503
  remains correctly deferred. The timer is active and no artifact is missing/corrupt.
