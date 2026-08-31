# Navidrome integration

## CURRENT

Configuration separates library and playlist output directories. music-sync has no
Navidrome database dependency. `playlist materialize` writes one stable UTF-8 M3U8
per durable collection from ordered active memberships and explicit preferred
healthy artifacts. Missing or unresolved artifacts are reported and omitted.

New managed acquisition artifacts are published as read-only files (`0644`) below
traversable managed provider directories (`0755`). This allows a separately running
Navidrome scanner to read committed media even though staging and the music-sync
service remain private. Publication changes modes only; it never rewrites audio.

The initial output is installed with no-clobber semantics. Only paths registered as
music-sync-owned may subsequently be atomically replaced. Synchronized temporary
files and directory metadata make normal repeats unchanged and permit exact-byte
recovery after interruption. Playlist removal changes only membership output;
acquired audio remains preserved.

Output filenames derive from safe current collection names. Entries are rendered
relative to the configured playlist directory, not the library root, so a playlist
below the library resolves sibling media correctly. If the named M3U8 already
exists, music-sync adopts it: valid known managed entries are deduplicated and all
other non-comment entries are durably preserved. A prior `collection-<id>.m3u8` is
retired only when its bytes match music-sync's recorded ownership hash.

Only sources that enumerate a real provider collection produce playlist output.
Single-video sources retain their provider membership and acquired audio but never
materialize a one-track M3U. Any legacy output for such a source is retired only
under the same exact recorded-hash ownership check.

## PLANNED

Navidrome scans the managed filesystem and remains responsible for playback,
streaming, transcoding, users, favorites, ratings, and play counts. Later listening
feedback uses a documented, read-only supported interface where available; it never
writes Navidrome's internal database. Lyrics and artwork layouts must be tested
against Navidrome compatibility.

Deployment detects actual library paths, playlist paths, service ownership, and
permissions in the target LXC. Initial migration access is read-only against
production media, with all generated/test output isolated elsewhere.
