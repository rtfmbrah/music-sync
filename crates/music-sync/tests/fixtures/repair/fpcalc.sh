#!/bin/sh
set -eu

printf '%s' '{"duration":180.0,"fingerprint":['
separator=''
value=1
while test "$value" -le 180; do
    printf '%s%s' "$separator" "$value"
    separator=','
    value=$((value + 1))
done
printf '%s' ']}'
