# Enumerate YouTube sources through yt-dlp

- Status: Completed
- Started: 2026-08-24
- Completed: 2026-08-24
- Roadmap: P1 source synchronization

## Goal

Add the first provider boundary: bounded YouTube video/playlist enumeration through
`yt-dlp`, with stored fixtures and conservative typed failures.

## Work completed

- Added provider-neutral item and ordered source-snapshot types, kept separate from
  canonical recordings.
- Added bounded yt-dlp video/flat-playlist enumeration without media downloads.
- Retained raw item metadata and normalized finite durations to milliseconds.
- Classified permanent unavailability, authentication, rate limit, timeout,
  extraction, and conservative transient failures.
- Added stored video, playlist, private, and rate-limit data plus malformed/timeout
  subprocess and black-box CLI tests.
- Added `source enumerate` text/JSON output; persistence and reconciliation remain
  deliberately outside this inspect-only slice.

## Validation evidence

- `just check`: passed before documentation closure with warnings-denied Clippy, 47
  deterministic tests, and architecture enforcement.
- Static musl release build: passed.
- Debian LXC offline fixture enumeration returned both ordered playlist items and raw
  metadata; installed yt-dlp version is 2026.06.09.
- No live request or library-file effect occurred; production remained non-writable.
