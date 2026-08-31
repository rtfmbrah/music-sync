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

Schema version 12 adds release-artwork resolution, selected-art provenance, and
immutable content-addressed blobs. `artwork fetch` considers only canonically selected
MusicBrainz releases. It reads the bounded Cover Art Archive release index, selects a
front image before other roles and approved art within a role, and prefers the current
1200 or 500 pixel thumbnail URLs before an original image. A provider `404` becomes a
durable unavailable result; network, provider, malformed-response, and storage errors
are deferred independently per release.

Downloaded art is limited to 20 MiB and accepted only when JPEG, PNG, or WebP magic
bytes match. SHA-256 determines an immutable path below `artwork-cache/` in application
state. Creation is no-clobber and an existing path must contain identical bytes.
Selection and provider JSON are persisted only after cache validation. Repeated runs
do no HTTP or filesystem work for resolved or unavailable releases. Artwork enrichment
does not rewrite audio and does not place files in the library.

Schema version 13 and `lyrics fetch` resolve exact canonical title, artist-credit,
release, and probed-duration signatures sequentially through LRCLIB. The adapter
requires an identifying User-Agent, uses a 2 MiB response limit and explicit deadline,
and treats 404, instrumental, and retryable failures as distinct durable states.
Returned title, artist, album, and duration must match independently before text is
accepted. Synchronized lyrics outrank plain text; synchronized content must contain a
timestamp and all text is NUL-free and bounded.

Adjacent `.lrc` output is restricted to healthy preferred artifacts proven owned by
a committed acquisition or repair. Adopted files are never mutated. Output intent is
prepared in SQLite before a same-filesystem no-clobber commit, enabling exact-byte
crash recovery. Unknown existing sidecars and contradictory prepared bytes are
preserved and deferred. Repeated committed runs perform no provider or file work.

Schema version 14 and `metadata materialize` provide explicit source-preserving tag
output for healthy artifacts proven owned by acquisition or repair. Selected title,
artist credit, release, date, recording MBID, and ISRC become ffmpeg metadata while
`-c copy` forbids audio transcoding. Selected canonical release art replaces provider
art: ordinary attachment-capable containers receive an attached image stream, while
Opus/Ogg receives a FLAC-picture block in Vorbis comments through a temporary
ffmetadata input rather than an oversized process argument.

Before remux, the exact source hash is copied into immutable SHA-256-addressed artifact
history. Hidden output is durably reserved, produced with overwrite disabled, then
structurally probed and hashed. Codec, sample rate, channels, and bounded duration must
agree with the source. Complete commit intent precedes atomic visible replacement; the
original history and output hashes distinguish interruption states. SQLite then moves
the historical artifact row, inserts the derived healthy artifact, and changes
preference transactionally. Unknown hidden paths and changed source bytes are
preserved/deferred; retry is explicit.

Schema version 20 adds provenance-aware provider display enrichment for managed media.
Source enumeration persists one bounded snapshot payload per provider item; acquisition
persists the complete yt-dlp info JSON captured beside the staged media, and a bounded
`provider_metadata` service phase refreshes owned items that still lack a complete
single-item payload through `--dump-single-json` without downloading media. Extraction
keeps explicit music fields (`track`, `artist`, `creator`, `album`, `genre`,
`release_date`) and labels channel/uploader fallbacks with their source field as
`artist_provenance`; generic categories such as `Music` or `Entertainment` are never
retained as genres, and only HTTPS thumbnail URLs are kept. Missing albums and genres
remain absent rather than fabricated.

Provider display fields drive conservative fallbacks only when canonical selections are
missing: lyrics signatures and tag materialization coalesce canonical title, artist
credit, release, and date with provider display values, and materialization additionally
carries explicit provider genres and artist provenance. Lyrics fall back to provider
fields only when the canonical artist credit exists or the provider artist provenance is
an explicit `artist`/`creator` field; channel/uploader display names never feed lyrics
lookup. Canonical release art outranks provider thumbnails: a validated HTTPS thumbnail
is selected only for owned committed recordings without canonical artwork, cached by
SHA-256 with the same magic-byte and size bounds, and embedded through the existing
tag-materialization picture path. Provider enrichment never creates recording identity,
never changes metadata selections, and provider-only failures are deferred and audited
without blocking canonical work.

Service phases with work selection report `skipped` instead of `succeeded` when zero
candidates were selected, so idle phases are distinguishable from committed work.

## PLANNED

Extend canonical release context with
track/disc positions through bounded release lookup. Do not model enrichment as
whichever provider wrote last.

Remaining provider payload fields—description, duration, and playlist context—remain
available in the retained raw enrichment JSON for debugging and recovery. Canonical
release art outranks a video thumbnail. Release selection accounts for original albums,
singles, soundtracks, deluxe editions, compilations, and remasters without confusing
them with recording identity.

Normalized filenames and future directory organization remain independent from codec
uniformity and this tag-only visible path transition. Exact acquired bytes remain in
application history.
