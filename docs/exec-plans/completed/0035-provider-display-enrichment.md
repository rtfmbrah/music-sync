# Provider display enrichment and conservative fallbacks

- Status: Complete
- Started: 2026-08-31
- Completed: 2026-08-31
- Roadmap: P3 metadata enrichment

## Goal

Managed media acquired by v2 must appear complete in Navidrome without inventing
identity: persist complete YouTube metadata per video, use provider
title/artist/album/genre as provenance-labelled display fallbacks, download,
validate, cache, and embed provider thumbnails, resolve lyrics conservatively with
title/artist/duration (album optional), retag existing managed files losslessly,
enrich all future downloads automatically, and distinguish zero-work phases from
committed work in status output.

## Safety boundary

- Provider display fields never establish recording identity and never override
  canonical MusicBrainz selections.
- Channel/uploader names are labelled fallbacks and never feed lyrics lookup; only
  explicit `artist`/`creator` provenance does.
- Generic provider categories (`Music`, `Entertainment`, `People & Blogs`) are never
  genres; absent album/genre remain absent rather than fabricated.
- Only HTTPS thumbnail URLs are accepted; images reuse the 20 MiB bound, magic-byte
  validation, and SHA-256 content-addressed cache.
- Retagging remains `-c copy` stream remux of owned committed artifacts only; adopted
  files are never mutated.
- Zero-candidate phases report `skipped`, not `succeeded`.

## Tasks

- [x] Schema 20: `provider_item_enrichments`, `recording_provider_artwork`,
      `provider_artwork_resolutions`.
- [x] Provenance-aware extraction (`provider_metadata`) with deterministic tests.
- [x] Snapshot enrichment during source reconciliation; full info-JSON enrichment at
      acquisition commit; bounded `provider_metadata` service phase for owned items
      lacking complete payloads.
- [x] Lyrics/tag/artwork candidate queries with canonical-first COALESCE fallbacks and
      artist-provenance gating.
- [x] Provider-thumbnail artwork fallback: validated cache, durable resolution state,
      embedding through the existing tag picture path.
- [x] Optional album in lyrics signatures and tag snapshots; explicit genres and
      artist provenance in tag materialization.
- [x] Zero-work phase status semantics (`phase_for_work`).
- [x] Deterministic tests: unit, persistence fallback scenario, adapter single-item
      metadata, acquisition info-JSON fixtures, 15-phase CLI cycle.
- [x] Architecture docs, roadmap, and status output updated.
- [x] Isolated LXC acceptance: schema-19 production-state copy migrates to 20.
- [x] Production release activation with pre-upgrade backup.
- [x] Production evidence: provider metadata resolved for 20 owned recordings, tags
      committed for 20, artwork resolved for 19, and three initial `.lrc` sidecars
      committed without modifying audio streams.
- [x] Follow-up implementation: prefer JPEG/PNG provider thumbnails, treat thumbnail
      404 as durable unavailable, convert cached WebP to validated PNG before tag
      embedding, and provide audited no-media-change release of incompatible prior
      materializations.
- [x] Contradictory exact LRCLIB responses become durable unavailable outcomes instead
      of recurring transient failures.
- [x] Schema 21 safely backfills prior exact-signature mismatch deferrals while
      preserving unrelated retryable failures and any selected/output lyrics.
- [x] Production compatibility release and audited rematerialization of all 22 WebP
      tag outputs.
- [x] Real Navidrome acceptance: title, artist, album, genre, cover, lyrics, and
      playlist assignment for the managed items.

## Acceptance evidence (so far)

- `just check` passed: formatting, Clippy with `-D warnings`, all 143 deterministic
  tests, the architecture check, deployment-script checks, and the performance gate.
- Committed as `5cb7078`; portable static binary SHA-256
  `c2c5ede3d0aa5cbbaf69e5a0bc8b9cd0e8fc812f3c947e67d3458fb7dae83b7b` staged on the
  LXC as `music-sync-enrichment` under the established deployment directory.
- Release `0.1.0-20260831.10` is active with schema 20. The production run exposed a
  real Navidrome compatibility gap: ffprobe reported WebP attached-picture MIME as
  unknown. Nineteen provider artworks resolved, one thumbnail returned HTTP 404, and
  two exact LRCLIB lookups returned contradictory signatures. The compatibility
  follow-up above addresses all three outcomes without weakening identity checks.
- Release `0.1.0-20260831.12` is active with schema 21. Production verification
  confirmed 22 committed tag outputs with readable title/artist fields and compatible
  embedded PNG front covers, five exact validated `.lrc` sidecars, eleven durable
  unavailable lyrics outcomes, and zero deferred artwork, lyrics, or tag outputs.
  The one-shot compatibility release selected zero items on repeat.
- Navidrome imported all 22 changed managed tracks and refreshed 18 albums. Its own UI
  displayed the embedded covers. Feishin initially retained stale Subsonic cover state
  and received temporary `getCoverArt` 429 responses; logging out and back in refreshed
  the client state and displayed every cover without another media rewrite or scan.
