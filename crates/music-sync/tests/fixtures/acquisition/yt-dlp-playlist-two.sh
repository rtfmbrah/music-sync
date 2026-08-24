#!/bin/sh
set -eu

case " $* " in
  *" --dump-single-json "*)
    printf '%s' '{"id":"fixture-playlist","title":"Fixture playlist","entries":[{"id":"one","url":"https://youtu.be/one"},{"id":"two","url":"https://youtu.be/two"}]}'
    ;;
  *)
    while test "$1" != "--paths"; do shift; done
    output=$2
    printf '%s' 'fixture audio' > "$output/media.opus"
    printf '%s\n' "$output/media.opus"
    ;;
esac
