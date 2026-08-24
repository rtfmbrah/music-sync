# Cover Art Archive selection and immutable cache

- Status: Complete
- Started: 2026-08-25
- Completed: 2026-08-25
- Roadmap: P3 metadata enrichment

## Goal

Select release artwork through a bounded Cover Art Archive boundary and retain it in
an immutable content-addressed cache without rewriting audio or replacing existing
library files.

## Safety boundary

- Only canonically selected MusicBrainz releases are candidates.
- Index and image responses have hard byte and time limits.
- Image magic bytes determine the accepted JPEG, PNG, or WebP type.
- Cache paths derive from SHA-256 and are created without overwriting.
- Missing art is durable; network and provider failures are deferred.
- Tests use stored fixtures and controlled loopback HTTP only.

## Result

- Schema version 12 stores explicit resolution state, selected provider provenance,
  and unique content-addressed image blobs.
- The bounded adapter handles release `404` as unavailable, upgrades trusted legacy
  CAA HTTP URLs, rejects other insecure URLs, and accepts current string or numeric
  image IDs.
- Deterministic selection prefers front, approved, stable-ID art and current bounded
  thumbnails; downloaded bytes require supported magic before no-clobber cache commit.
- CLI/status, parser, persistence, cache, retry, repeat-offline, and black-box tests
  pass.
- Canonical local verification and isolated schema-v12 LXC validation pass while the
  production library remains read-only.
