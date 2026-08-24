# Metadata, artwork, and lyrics

## CURRENT

Provider items have a separate source metadata payload so canonical enrichment
cannot erase acquisition/audit context. Acquisition and bounded adoption probing
classify embedded MusicBrainz recording IDs and ISRCs as absent, structurally valid,
or malformed. Initial acquisition retains unambiguous values as recording evidence;
structural validity is not provider-object identity.

Schema version 11 separates canonical artists, releases, ordered artist credits,
recording/release relationships, field observations, explicit field selections, and
resolution state. Each observation retains field, value, provider, provider entity,
fixed-point confidence, resolution context, and observation time. Provider payloads
remain untouched.

`metadata resolve` selects healthy recordings with a strong recording MBID or
normalized ISRC in stable bounded order. Exact MBID lookup is preferred. ISRC lookup
is accepted only when exactly one returned recording contains that ISRC; zero or
multiple matches are persisted ambiguous. Titles are never lookup identity.
Provider/network failures become deferred without changing selected metadata.

The MusicBrainz `/ws/2` adapter uses JSON, a 4 MiB response bound, explicit request
deadline, HTTPS except controlled loopback fixtures, a required meaningful
User-Agent, and production pacing of at most one request per second. It retains the
bounded raw response for audit. Release selection deterministically prefers official
non-compilation Album, EP, then Single context, followed by date and MBID. Canonical
recording title, composed artist credit, selected release title, and release date are
stored as independent selected observations.

## PLANNED

Resolve release artwork through Cover Art Archive and synchronized or plain lyrics
through LRCLIB or another evaluated provider. Extend canonical release context with
track/disc positions through bounded release lookup. Do not model enrichment as
whichever provider wrote last.

Provider metadata—ID, URL, title, channel, description, duration, thumbnail, and
playlist context—remains available for debugging and recovery. Canonical release art
outranks a video thumbnail. Release selection accounts for original albums, singles,
soundtracks, deluxe editions, compilations, and remasters without confusing them with
recording identity.

Normalized filenames, directory organization, tags, art, and lyrics are independent
of codec uniformity. Prefer adjacent `.lrc` files for lyrics so updates do not rewrite
audio and remain compatible with Navidrome. Tag mutations need atomic replacement,
prior-artifact validation, and idempotency tests.
