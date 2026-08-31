#!/bin/sh
set -eu

if [ "$#" -gt 1 ]; then
    echo "usage: $0 [TIMEOUT_SECONDS]" >&2
    exit 2
fi

timeout_seconds=${1:-21600}
case "$timeout_seconds" in
    *[!0-9]*|'')
        echo "error: timeout must be a positive integer" >&2
        exit 2
        ;;
esac
if [ "$timeout_seconds" -eq 0 ]; then
    echo "error: timeout must be greater than zero" >&2
    exit 2
fi
if [ "$(id -u)" -ne 0 ]; then
    echo "error: service quiescing must run as root" >&2
    exit 1
fi

timer_was_active=false
if systemctl is-active --quiet music-sync.timer; then
    timer_was_active=true
    systemctl stop music-sync.timer
fi

restore_timer_on_error() {
    status=$?
    trap - EXIT HUP INT TERM
    if [ "$status" -ne 0 ] && [ "$timer_was_active" = true ]; then
        systemctl start music-sync.timer
    fi
    exit "$status"
}
trap restore_timer_on_error EXIT HUP INT TERM

started=$(date +%s)
while systemctl is-active --quiet music-sync.service; do
    now=$(date +%s)
    if [ $((now - started)) -ge "$timeout_seconds" ]; then
        echo "error: music-sync.service did not finish within ${timeout_seconds}s" >&2
        exit 1
    fi
    sleep 2
done

trap - EXIT HUP INT TERM
echo "music-sync is idle; music-sync.timer remains stopped for maintenance."
echo "Restart the timer after maintenance: systemctl start music-sync.timer"
