#!/bin/sh
set -e
n=$(git rev-list --count HEAD)
[ "$n" -ge 6 ] || { echo "only $n commits"; exit 1; }
if git log --format=%B | grep -qi 'co-authored-by'; then echo "co-author trailer found"; exit 1; fi
# Every commit body must be empty (one-line messages).
bad=$(git log --format='%H %b' | awk 'NF>1' | wc -l | tr -d ' ')
[ "$bad" -eq 0 ] || { echo "$bad commits have a body"; exit 1; }
echo GATE_COMMITS_OK
