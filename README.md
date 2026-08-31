# music-sync

music-sync is an autonomous music-library manager for Navidrome. It synchronizes
configured YouTube playlists and tracks, preserves downloaded audio, enriches
metadata, and performs conservative identity-verified repair and discovery.

## Usage

The installed LXC configuration defaults to `/etc/music-sync/music-sync.toml`.

```bash
music-sync status
music-sync status watch
music-sync list
music-sync list --full
music-sync list --failed --missing
music-sync list --success --source winstreak
music-sync duplicates
music-sync sync
```

Manage sources with:

```bash
music-sync add "YOUTUBE_URL" --name "NAME"
music-sync remove SOURCE_ID
```

`remove` disables a source without deleting audio. `duplicates` is report-only and
never deletes, moves, or merges files. Use `--json` for machine-readable output.

## Development

```bash
nix develop
just check
just build-portable
```
