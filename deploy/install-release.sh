#!/bin/sh
set -eu

if [ "$#" -ne 3 ]; then
    echo "usage: $0 VERSION MUSIC_SYNC_BINARY YT_DLP_BINARY" >&2
    exit 2
fi

version=$1
music_sync_binary=$2
yt_dlp_binary=$3
release_root=/opt/music-sync/releases
release_directory=$release_root/$version
pending_link=/opt/music-sync/.current.new

case "$version" in
    *[!A-Za-z0-9._-]*|'')
        echo "error: VERSION must contain only letters, digits, dot, underscore, or hyphen" >&2
        exit 2
        ;;
esac

test -f "$music_sync_binary"
test -f "$yt_dlp_binary"
if [ -e "$release_directory" ]; then
    echo "error: release already exists: $release_directory" >&2
    exit 1
fi

install -d -m 0755 /opt/music-sync "$release_root"
install -d -m 0755 "$release_directory"
install -m 0755 "$music_sync_binary" "$release_directory/music-sync"
install -m 0755 "$yt_dlp_binary" "$release_directory/yt-dlp"
"$release_directory/music-sync" --version
"$release_directory/yt-dlp" --version
ln -s "$release_directory" "$pending_link"
mv -T "$pending_link" /opt/music-sync/current
echo "Activated music-sync release $version"
