# Metadata, artwork, and lyrics

## CURRENT

Provider items have a separate source metadata payload so later canonical enrichment
cannot erase acquisition/audit context. Bounded adoption probing classifies embedded
MusicBrainz recording IDs and ISRCs as absent, structurally valid, or malformed.
Values remain out of aggregate reports, and structural validity is evidence rather
than proof that the tagged audio is the claimed recording. No remote metadata
integration is implemented.

## PLANNED

Resolve recording, artist, and release through MusicBrainz; release artwork through
Cover Art Archive; synchronized or plain lyrics through LRCLIB or another evaluated
provider. Keep provenance, confidence, source identifiers, and timestamps per field.
Do not model enrichment as whichever provider wrote last.

Provider metadata—ID, URL, title, channel, description, duration, thumbnail, and
playlist context—remains available for debugging and recovery. Canonical release art
outranks a video thumbnail. Release selection accounts for original albums, singles,
soundtracks, deluxe editions, compilations, and remasters without confusing them with
recording identity.

Normalized filenames, directory organization, tags, art, and lyrics are independent
of codec uniformity. Prefer adjacent `.lrc` files for lyrics so updates do not rewrite
audio and remain compatible with Navidrome. Tag mutations need atomic replacement,
prior-artifact validation, and idempotency tests.
