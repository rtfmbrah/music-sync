# Validate LXC compatibility

- Status: Completed
- Started: 2026-08-23
- Completed: 2026-08-23
- Roadmap: P5 deployment foundation

## Goal

Confirm the production container's generic runtime/media characteristics and prove
that the bootstrap CLI runs safely in isolated storage without accessing legacy code
or mutating production music.

## Safety upheld

- Remote inspection was read-only except for two files copied under
  `/srv/music-sync-v2-test`.
- Production `/srv/music` remained read-only before and after testing.
- Legacy source/state, secrets, Navidrome database contents, and Docker API were not
  inspected.
- No package installation, service mutation, or production deployment occurred.

## Work completed

- Confirmed Debian 12, glibc 2.36, x86_64, tools, storage, permissions, and media
  layout.
- Sampled a generic M3U and one media artifact's tags with `ffprobe`.
- Recorded 1,560 M4A artifacts, 891 LRC files, 45 text files, 16 playlists, two
  JPEGs, and no symlinks.
- Identified that ordinary NixOS/glibc output embeds Nix-store paths and cannot be
  copied directly to Debian.
- Added a pinned musl Rust target/compiler and `just build-portable`.
- Verified the result was statically linked, 4,871,432 bytes, and SHA-256
  `60acd8f97ab84c873d3c8de2776032f9c1fdb940ecaf0355e5270363113485b4`.
- Copied only the binary and non-secret test config to isolated test storage.
- Ran version and offline JSON doctor successfully on Debian. Installed
  `yt-dlp`/`ffmpeg`/`ffprobe` passed; future `fpcalc` was correctly a warning.
- Rechecked that `/srv/music` was not writable.

## Validation

- `nix develop path:. -c just build-portable`: passed.
- ELF dynamic interpreter/`NEEDED` inspection: none; `ldd` reported statically linked.
- Local/remote SHA-256: identical.
- Remote `music-sync --version`: `music-sync 0.1.0`.
- Remote `music-sync --json doctor --config .lxc-test.toml`: healthy with warnings
  only for planned/uninitialized features.
- Remote production write guard: `production_writable=no`.

