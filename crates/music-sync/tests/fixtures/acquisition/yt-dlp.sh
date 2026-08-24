#!/bin/sh
case " $* " in
  *" --dump-single-json "*)
    printf '%s' '{"id":"video1","webpage_url":"https://www.youtube.com/watch?v=video1","title":"Fixture"}'
    ;;
  *)
    while test "$1" != '--paths'; do
      shift
    done
    printf '%s' 'fixture audio' > "$2/media.opus"
    printf '%s\n' "$2/media.opus"
    ;;
esac
