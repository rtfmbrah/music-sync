# Lyrics provenance and adjacent sidecars

- Status: Complete
- Started: 2026-08-25
- Completed: 2026-08-25
- Roadmap: P3 metadata enrichment

## Goal

Resolve bounded synchronized or plain lyrics from canonical recording metadata and
materialize Navidrome-compatible adjacent `.lrc` files without rewriting audio or
overwriting unknown files.

## Result

- Schema version 13 stores provider observations, explicit selection, independent
  resolution states, and prepared/committed output intent.
- The sequential bounded LRCLIB adapter identifies the client and independently
  validates title, artist, album, duration, content size, and synchronized timestamps.
- Synchronized text outranks plain; instrumental, unavailable, and deferred are
  durable isolated outcomes.
- Adjacent output is limited to committed acquisition/repair artifacts. Adopted audio
  is excluded, unknown sidecars are preserved, and same-filesystem no-clobber commit
  supports exact-byte recovery.
- CLI/status, deterministic parser/no-clobber/ownership/black-box tests, canonical
  checks, and isolated schema-v13 LXC validation pass.
