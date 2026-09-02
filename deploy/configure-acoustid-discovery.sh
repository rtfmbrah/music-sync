#!/bin/sh
set -eu

config=/etc/music-sync/music-sync.toml
environment=/etc/music-sync/music-sync.env
staging=/srv/music-sync-v2-test/deployment-20260902

test "$(id -u)" -eq 0
test -f "$config"
IFS= read -r key
case "$key" in
    ''|*[!A-Za-z0-9]*)
        echo "error: AcoustID client key must contain only letters and digits" >&2
        exit 2
        ;;
esac
test "${#key}" -le 128

"$staging/quiesce-service.sh" 21600

environment_tmp=$(mktemp /etc/music-sync/.music-sync.env.XXXXXX)
config_tmp=$(mktemp /etc/music-sync/.music-sync.toml.XXXXXX)
trap 'rm -f "$environment_tmp" "$config_tmp"' EXIT HUP INT TERM

if test -f "$environment"; then
    awk '$0 !~ /^ACOUSTID_CLIENT_KEY=/' "$environment" >"$environment_tmp"
fi
printf 'ACOUSTID_CLIENT_KEY=%s\n' "$key" >>"$environment_tmp"
chown root:music-sync "$environment_tmp"
chmod 0640 "$environment_tmp"

awk '
    /^youtube_search_fallback[[:space:]]*=/ { next }
    /^youtube_search_max_candidates[[:space:]]*=/ { next }
    /^acoustid_minimum_score[[:space:]]*=/ { next }
    /^acoustid_fingerprint_audio_seconds[[:space:]]*=/ { next }
    /^\[discovery\]$/ {
        found=1
        print
        print "youtube_search_fallback = true"
        print "youtube_search_max_candidates = 3"
        print "acoustid_minimum_score = 0.95"
        print "acoustid_fingerprint_audio_seconds = 900"
        next
    }
    { print }
    END { if (!found) exit 42 }
' "$config" >"$config_tmp"
chown root:music-sync "$config_tmp"
chmod 0640 "$config_tmp"

backup="$config.pre-acoustid-$(date +%s)"
cp -p "$config" "$backup"
mv "$environment_tmp" "$environment"
mv "$config_tmp" "$config"
trap - EXIT HUP INT TERM

set -a
. "$environment"
set +a
/opt/music-sync/current/music-sync doctor --config "$config"
unset ACOUSTID_CLIENT_KEY key
systemctl start music-sync.timer
systemctl list-timers music-sync.timer --no-pager

echo "AcoustID discovery search enabled; previous config: $backup"
