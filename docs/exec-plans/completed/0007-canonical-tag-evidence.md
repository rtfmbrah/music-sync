# Validate embedded canonical tag evidence

- Status: Completed
- Started: 2026-08-24
- Completed: 2026-08-24
- Roadmap: P1 adoption

## Goal

Distinguish structurally valid MusicBrainz recording IDs and ISRCs from malformed or
absent embedded tags during bounded adoption probing, without exposing values or
claiming that a tag alone proves audio identity.

## Work completed

- Added typed absent, valid, and malformed canonical-tag states.
- Validated UUID-shaped MusicBrainz recording IDs and both compact and official
  display-form ISRCs.
- Recognized normalized recording-ID and standard MusicBrainz track-ID tag aliases.
- Aggregated any-valid, valid-MBID, valid-ISRC, and malformed-tag file counts while
  keeping identifier values out of reports.
- Added parser, subprocess, adoption aggregation, and black-box CLI fixtures.
- Documented that structurally valid embedded identifiers remain unverified claims.

## Validation evidence

- `just check`: passed with warnings-denied Clippy, 35 deterministic tests, and
  architecture enforcement.
- Static musl release build: passed.
- Alias-aware Debian LXC full probe succeeded for all 1,560 media files with zero
  valid or malformed recording MBID/ISRC observations.
- The remote run reported zero modified, deleted, or downloaded files; production
  remained non-writable to `music-sync-dev`.
