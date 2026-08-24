#!/bin/sh
set -eu

target=''
for argument do
    target=$argument
done

case " $* " in
    *' --dump-single-json '*)
        case "$target" in
            ytsearch*)
                printf '%s' '{"id":"search","title":"Fixture search","entries":[{"id":"candidate","webpage_url":"https://youtu.be/candidate","title":"Fixture Track","duration":180.0}]}'
                ;;
            *)
                if test -n "${REPAIR_MARKER:-}" && test -f "$REPAIR_MARKER"; then
                    printf '%s' 'ERROR: Video unavailable: deleted' >&2
                    exit 1
                fi
                printf '%s' '{"id":"original","webpage_url":"https://youtu.be/original","title":"Fixture Track","duration":180.0}'
                ;;
        esac
        ;;
    *)
        while test "$1" != '--paths'; do shift; done
        output=$2
        printf '%s' 'fixture repair audio' > "$output/media.opus"
        printf '%s\n' "$output/media.opus"
        ;;
esac
