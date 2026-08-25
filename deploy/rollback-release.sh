#!/bin/sh
set -eu

if [ "$#" -ne 1 ]; then
    echo "usage: $0 VERSION" >&2
    exit 2
fi

version=$1
release_directory=/opt/music-sync/releases/$version
pending_link=/opt/music-sync/.current.new

case "$version" in
    *[!A-Za-z0-9._-]*|'')
        echo "error: invalid release version" >&2
        exit 2
        ;;
esac

test -x "$release_directory/music-sync"
test -x "$release_directory/yt-dlp"
"$release_directory/music-sync" --version
"$release_directory/yt-dlp" --version
ln -s "$release_directory" "$pending_link"
mv -T "$pending_link" /opt/music-sync/current
echo "Activated prior music-sync release $version"
