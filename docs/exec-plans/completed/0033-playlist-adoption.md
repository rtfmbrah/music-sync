# Named playlist adoption

- Status: Complete
- Started: 2026-08-26
- Roadmap: P5 production compatibility

## Goal

Materialize Navidrome-compatible playlists under their real collection names,
adopting a matching existing M3U8 without losing unknown entries and retiring only
the exact prior music-sync-owned `collection-<id>` output.

## Safety boundary

- Existing named playlist entries are preserved unless they duplicate a known active
  managed artifact.
- Only an old output whose bytes match music-sync's recorded SHA-256 may be retired.
- Audio is never changed or deleted.
- Output paths remain contained in the configured playlist directory.
- Repeated materialization is idempotent and membership removal never deletes audio.

## Tasks

- [x] Add durable preserved-entry state.
- [x] Use safe collection names and paths relative to the playlist directory.
- [x] Adopt matching existing playlists and deduplicate managed entries.
- [x] Migrate and safely retire exact owned legacy outputs.
- [x] Exclude single-video sources and retire their exact owned legacy outputs.
- [x] Run canonical checks and isolated production-shaped acceptance.
- [x] Back up, deploy, verify Navidrome-visible production output, and complete plan.

## Completion evidence

- `just check` passed with 71 library unit tests, 31 CLI integration tests, all
  adapter/scenario tests, Clippy with warnings denied, and the architecture check.
- Isolated production-shaped acceptance materialized exactly eight adopted `.m3u`
  playlists, no `collection-*` output, and no output for four single-video sources;
  the immediate repeat was unchanged.
- Production release `0.1.0-20260826.5` migrated to schema 19 after consistent
  database and playlist-directory backups. Final doctor passed, the timer was active,
  and status reported eight playlist outputs with no missing or corrupt audio.
