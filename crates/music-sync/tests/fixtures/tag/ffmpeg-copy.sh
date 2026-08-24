#!/bin/sh
set -eu
source_path=
previous=
destination=
saw_copy=false
saw_title=false
saw_picture=false
for argument do
    if [ "$previous" = "-i" ] && [ -z "$source_path" ]; then
        source_path=$argument
    elif [ "$previous" = "-i" ]; then
        grep -q '^METADATA_BLOCK_PICTURE=' "$argument"
        grep -q '^title=Canonical Track$' "$argument"
        saw_picture=true
        saw_title=true
    fi
    if [ "$previous" = "-c" ] && [ "$argument" = "copy" ]; then
        saw_copy=true
    fi
    if [ "$previous" = "-metadata" ] && [ "$argument" = "title=Canonical Track" ]; then
        saw_title=true
    fi
    if [ "$previous" = "-metadata" ]; then
        case "$argument" in METADATA_BLOCK_PICTURE=*) saw_picture=true ;; esac
    fi
    previous=$argument
    destination=$argument
done
test "$saw_copy" = true
test "$saw_title" = true
test "$saw_picture" = true
cp "$source_path" "$destination"
printf '%s' '-canonical-tags' >> "$destination"
