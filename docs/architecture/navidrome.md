# Navidrome integration

## CURRENT

Configuration separates library and playlist output directories. music-sync has no
Navidrome database dependency. `playlist materialize` writes one stable UTF-8 M3U8
per durable collection from ordered active memberships and explicit preferred
healthy artifacts. Missing or unresolved artifacts are reported and omitted.

The initial output is installed with no-clobber semantics. Only paths registered as
music-sync-owned may subsequently be atomically replaced. Synchronized temporary
files and directory metadata make normal repeats unchanged and permit exact-byte
recovery after interruption. Playlist removal changes only membership output;
acquired audio remains preserved.

## PLANNED

Navidrome scans the managed filesystem and remains responsible for playback,
streaming, transcoding, users, favorites, ratings, and play counts. Later listening
feedback uses a documented, read-only supported interface where available; it never
writes Navidrome's internal database. Lyrics and artwork layouts must be tested
against Navidrome compatibility.

Deployment detects actual library paths, playlist paths, service ownership, and
permissions in the target LXC. Initial migration access is read-only against
production media, with all generated/test output isolated elsewhere.
