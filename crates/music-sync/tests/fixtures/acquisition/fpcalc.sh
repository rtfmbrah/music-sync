#!/bin/sh
set -eu

test "$1" = "-raw"
test "$2" = "-json"
test "$3" = "-algorithm"
test "$4" = "2"
test "$5" = "-length"
test "$6" -gt 0

printf '%s' '{"duration":180.0,"fingerprint":['
separator=''
value=1
while test "$value" -le 180; do
    printf '%s%s' "$separator" "$value"
    separator=','
    value=$((value + 1))
done
printf '%s' ']}'
