# Autonomous music discovery

## CURRENT

Configuration defines a disabled-by-default discovery policy with target/max daily
tracks, per-artist limit, exploration/wildcard ratios, and minimum free disk. The
off-switch returns before opening state or contacting providers.

ListenBrainz collaborative-filtering recommendations enter as canonical recording
MBIDs through a bounded read-only HTTPS adapter. Scores are normalized and combined
with seed strength using fixed-point arithmetic. Every explanation and decision is
persisted. Exact owned/approved/queued/acquired MBIDs are duplicates. Daily total,
per-artist (including one conservative shared bucket for unknown artists), lane, and
free-storage budgets are enforced in one immediate SQLite transaction.

Optional Navidrome favorites are read through `getStarred2` in the public Subsonic
API. The adapter never reads Navidrome's database. Only favorite entries carrying an
exact recording MBID that already exists locally become active seeds. The precomputed
Subsonic token and salt are process secrets; they are not stored in TOML or logs.

When `youtube_search_fallback` is explicitly enabled, an approved recording MBID
without a MusicBrainz recording-level YouTube relationship uses a bounded serial
yt-dlp search. Canonical title, full artist credit, two-second duration, and version
qualifiers reject candidates but never establish identity. Candidate audio stays in
`discovery-search-staging` while it is structurally validated, hashed, and checked
through AcoustID. AcoustID must return the exact expected recording MBID above the
configured threshold without a competing above-threshold MBID. The exact verified
SHA-256 is persisted and the ordinary acquisition path reuses those bytes for its
atomic no-clobber commit; it never downloads again after verification.

The fallback is disabled by default and requires `ACOUSTID_CLIENT_KEY` only when both
discovery and the fallback are enabled. AcoustID receives the compressed fingerprint,
rounded duration, and metadata selector, never media bytes. Empty or low-confidence
coverage is unresolved rather than contradictory, so catalog gaps do not classify
obscure remixes as wrong. Provider, fingerprint, and AcoustID failures are deferred
and stop that recommendation's candidate sequence.

## PLANNED

Expand signals beyond favorites and collaborative filtering to ratings,
recent/repeated plays, and artist/album listening where canonical identifiers exist.

Taste confidence and identity confidence are separate. Medium taste confidence may
be acceptable exploration; uncertain identity is never acceptable acquisition. An
explainable scorer records contributing signals and assigns candidates to tunable
exploitation, exploration, or wildcard bands. Approximate starting ratios belong in
configuration and evidence, not hard-coded business logic.

Route approved candidates only when MusicBrainz supplies a recording-level provider
URL relationship and the staged result independently matches canonical identity and
duration. Normal operation is automatic, not a weekly review inbox. Discovery misses
are acceptable; misidentified downloads are not.

Discovery-acquired media follows permanent preservation. Lack of listening is never
implicit permission to delete.

The complete recommendation-to-acquisition boundary is validated on Debian 12 with
a static musl binary in isolated writable storage. The acceptance proves durable
schema migration, hard-budget approval, exact MusicBrainz relationship routing,
independent staged MBID/ISRC and duration verification, committed artifact state,
and read-only production-library access.
