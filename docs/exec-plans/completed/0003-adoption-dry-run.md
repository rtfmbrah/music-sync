# Implement adoption dry run

- Status: Completed
- Started: 2026-08-23
- Completed: 2026-08-23
- Roadmap: P1

## Goal

Implement a useful generic library-adoption scanner and truthful CLI report without
introducing database mutations, media probing claims, or an unsafe apply workflow.

## Work completed

- Added reusable extension/readability classification with non-followed symlinks.
- Preserved unknown files and reported zero-byte media as an issue rather than
  deleting or adopting it as healthy.
- Added bounded issue details, robust JSON paths, and explicit zero-effect counts.
- Added `music-sync library adopt <path>` text/JSON output.
- Added unit, filesystem integration, and black-box CLI tests.
- Rebuilt the static binary and executed a complete production-library dry run under
  the read-only LXC account.

## Validation

- `cargo clippy --workspace --all-targets --all-features -- -D warnings`: passed.
- `cargo test --workspace --all-features`: passed (20 tests).
- `nix develop path:. -c just build-portable`: passed.
- Remote dry run: 2,514 files; 1,560 media; 891 lyrics; two artwork; 16 playlists;
  45 unknown preserved; zero corrupt/unreadable; zero filesystem effects.
- Remote production write guard after scanning: `production_writable=no`.
