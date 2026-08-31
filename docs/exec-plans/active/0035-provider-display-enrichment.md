# Provider display enrichment and conservative fallbacks

- Status: In progress (local implementation complete, LXC acceptance pending)
- Started: 2026-08-31
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
- [ ] Isolated LXC acceptance: schema-19 production-state copy migrates to 20.
- [ ] Production release activation with pre-upgrade backup.
- [ ] Real Navidrome acceptance: title, artist, album, genre, cover, lyrics, and
      playlist assignment for the managed items.

## Acceptance evidence (so far)

- `cargo fmt --check`, Clippy with `-D warnings`, all 142 deterministic tests, the
  architecture check, and the deployment-script checks pass locally.
