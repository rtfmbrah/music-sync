#!/bin/sh
set -eu

binary=${1:-/srv/music-sync-v2-test/deployment-20260902/music-sync-schema24}
root=${2:-/srv/music-sync-state/schema24-acoustid-acceptance-20260902}
config=$root/music-sync.toml
state=$root/state
playlists=$root/playlists
pre_backups=$root/pre-backups
post_backups=$root/post-backups
acceptance_binary=$root/music-sync-schema24

test "$(id -u)" -eq 0
test -x "$binary"
test "$(systemctl is-active music-sync.service || true)" = inactive

install -d -o music-sync -g music-sync -m 0750 \
    "$root" "$state" "$playlists" "$pre_backups" "$post_backups"
install -o root -g music-sync -m 0750 "$binary" "$acceptance_binary"

runuser -u music-sync -- /opt/music-sync/current/music-sync maintenance backup \
    --config /etc/music-sync/music-sync.toml --directory "$pre_backups"
snapshot=$(find "$pre_backups" -maxdepth 1 -type f -name 'music-sync-*.sqlite3' -print | sort | tail -1)
test -n "$snapshot"
install -o music-sync -g music-sync -m 0640 "$snapshot" "$state/music-sync.sqlite3"

cat >"$config" <<EOF
state_directory = "$state"
library_directory = "/srv/music"
playlist_directory = "$playlists"

[discovery]
enabled = false
youtube_search_fallback = false

[service]
enabled = false
EOF
chown music-sync:music-sync "$config"
chmod 0640 "$config"

runuser -u music-sync -- "$acceptance_binary" maintenance backup \
    --config "$config" --directory "$post_backups"
runuser -u music-sync -- "$acceptance_binary" --json status --config "$config" >"$root/status.json"
grep -q '"schema_version": 24' "$root/status.json"

echo "Schema 24 isolated migration acceptance complete: $root"
