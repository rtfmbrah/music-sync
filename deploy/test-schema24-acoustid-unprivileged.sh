#!/bin/sh
set -eu

old_binary=${1:-/opt/music-sync/current/music-sync}
new_binary=${2:-/srv/music-sync-v2-test/deployment-20260902/music-sync-schema24}
root=${3:-/srv/music-sync-v2-test/schema24-acoustid-acceptance-20260902-v2}

test -x "$old_binary"
test -x "$new_binary"
test ! -e "$root"
mkdir -p "$root/state" "$root/library" "$root/playlists" \
    "$root/schema23-backups" "$root/schema24-backups"
cat >"$root/music-sync.toml" <<EOF
state_directory = "$root/state"
library_directory = "$root/library"
playlist_directory = "$root/playlists"

[discovery]
enabled = false

[service]
enabled = false
EOF

"$old_binary" maintenance backup --config "$root/music-sync.toml" \
    --directory "$root/schema23-backups"
"$new_binary" maintenance backup --config "$root/music-sync.toml" \
    --directory "$root/schema24-backups"
"$new_binary" --json status --config "$root/music-sync.toml" >"$root/status.json"
grep -q '"schema_version": 24' "$root/status.json"

echo "Unprivileged schema 23 -> 24 migration acceptance complete: $root"
