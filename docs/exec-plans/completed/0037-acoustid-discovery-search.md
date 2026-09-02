# Optional AcoustID discovery search (completed)

## Goal

Allow an approved ListenBrainz recording recommendation that lacks an exact
MusicBrainz YouTube relationship to search YouTube automatically, while keeping
text matching candidate-only and requiring optional AcoustID corroboration before
publication.

## Safety and design

- The fallback is disabled by default and requires both explicit configuration and
  an `ACOUSTID_CLIENT_KEY` process secret.
- MusicBrainz supplies the expected recording MBID, artist credit, title, duration,
  and meaningful version qualifiers.
- yt-dlp search rank, title, uploader, and duration only reject or prioritize
  candidates. They never prove recording identity.
- Candidate media remains in durable staging until a bounded Chromaprint lookup
  returns the exact expected recording MBID with sufficient confidence and without
  contradictory high-confidence identities.
- An empty AcoustID result is unresolved, not a mismatch. Transient provider or
  infrastructure failures remain retryable and never cause another candidate to be
  trusted.
- Existing exact MusicBrainz recording-level YouTube relationships retain their
  current independent embedded-MBID/ISRC verification path.
- No audio is submitted to AcoustID. The request contains the compressed
  Chromaprint, duration, application key, and response metadata selection.

## Work

- [x] Add bounded compressed Chromaprint extraction and an AcoustID lookup adapter.
- [x] Persist search candidates, attempts, provider evidence, and explicit terminal
  or retryable states.
- [x] Add conservative canonical text/duration/version prefiltering and serial
  yt-dlp staging.
- [x] Permit acquisition commit only after exact AcoustID MBID corroboration.
- [x] Integrate the optional phase into service operation and CLI observability.
- [x] Add fixture-backed scenarios, documentation, migration, and LXC acceptance.
- [x] Run the canonical repository verification and production acceptance.

## Acceptance

- `just check` passed with deterministic unit, integration, architecture, CLI,
  safety, deployment, and performance checks.
- The portable release migrated an isolated production snapshot from schema 23 to
  schema 24 as the unprivileged service account.
- Production schema 24 diagnostics passed with the AcoustID secret present.
- Production timer run 186 completed all enabled phases successfully on
  2026-09-02; systemd reported exit status 0.
