#!/bin/sh
set -eu

version=0.1.0-20260902.16
staging=/srv/music-sync-v2-test/deployment-20260902
binary=$staging/music-sync-schema24
expected_sha256=6a596aef70252721027b2433b7904ff538aef841a2567b4d977061ccf8eb3bd5
release=/opt/music-sync/releases/$version

test "$(id -u)" -eq 0
test -x "$binary"
printf '%s  %s\n' "$expected_sha256" "$binary" | sha256sum --check -

"$staging/quiesce-service.sh" 21600
"$staging/test-schema24-acoustid.sh" "$binary"

/opt/music-sync/current/music-sync maintenance backup \
    --config /etc/music-sync/music-sync.toml \
    --directory /srv/music-sync-backups

if test ! -d "$release"; then
    "$staging/install-release.sh" "$version" "$binary" /opt/music-sync/current/yt-dlp
else
    printf '%s  %s\n' "$expected_sha256" "$release/music-sync" | sha256sum --check -
    pending=/opt/music-sync/.current.new
    test ! -e "$pending"
    ln -s "$release" "$pending"
    mv -T "$pending" /opt/music-sync/current
    echo "Release $version is already installed; activation recovered."
fi

/opt/music-sync/current/music-sync maintenance backup \
    --config /etc/music-sync/music-sync.toml \
    --directory /srv/music-sync-backups
/opt/music-sync/current/music-sync doctor --config /etc/music-sync/music-sync.toml
/opt/music-sync/current/music-sync --json status \
    --config /etc/music-sync/music-sync.toml | grep -q '"schema_version": 24'

echo "Schema 24 AcoustID discovery release activated; music-sync.timer remains stopped."
echo "Configure the optional key and fallback before restarting the timer."
