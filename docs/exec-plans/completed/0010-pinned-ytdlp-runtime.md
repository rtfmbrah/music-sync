# Pin the yt-dlp runtime artifact

- Status: Completed
- Started: 2026-08-24
- Completed: 2026-08-24
- Roadmap: P1/P5 provider deployment

## Goal

Make yt-dlp deployment reproducible and independently updateable without embedding
provider code into the Rust binary or following an unverified `latest` URL.

## Work completed

- Pinned official stable Unix artifact version `2026.08.19` and its SHA-256.
- Added an explicit-destination installer with verified download, executable mode,
  atomic replacement, and post-install version verification.
- Documented self-update versus project-pinned side-by-side deployment.
- Added offline installer syntax validation to the canonical repository check.

## Validation evidence

- Installer argument and Bash syntax checks: passed.
- Official artifact download, SHA-256 verification, atomic `/tmp` install, and
  reported version: passed.
- Checksum-verified artifact executed successfully in isolated Debian LXC storage.
- The existing `/usr/local/bin/yt-dlp` remained unchanged; production music remained
  non-writable.
