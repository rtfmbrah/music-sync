# Autonomous music discovery

## CURRENT

Configuration defines a disabled-by-default discovery policy with target/max daily
tracks, per-artist limit, exploration/wildcard ratios, and minimum free disk. Values
are validated, but no candidates are generated or acquired.

## PLANNED

Generate candidates from configured seeds, the canonical library, related artists
and recordings, genres/channels, ListenBrainz-like data, and read-only Navidrome
favorites, ratings, recent/repeated plays, and artist/album listening. Deduplicate
against owned and already-considered recordings before scoring.

Taste confidence and identity confidence are separate. Medium taste confidence may
be acceptable exploration; uncertain identity is never acceptable acquisition. An
explainable scorer records contributing signals and assigns candidates to tunable
exploitation, exploration, or wildcard bands. Approximate starting ratios belong in
configuration and evidence, not hard-coded business logic.

Apply daily target/hard maximum, per-artist diversity, bounded provider concurrency,
and minimum-free-storage guards before canonical resolution and the common verified
acquisition pipeline. Normal operation is automatic, not a weekly review inbox.
Discovery misses are acceptable; misidentified downloads are not.

Discovery-acquired media follows permanent preservation. Lack of listening is never
implicit permission to delete.

