#!/bin/sh
set -e
test -s docs/zrehearse.svg
test -s docs/architecture.png
grep -q 'docs/zrehearse.svg' README.md
grep -q 'docs/architecture.png' README.md
echo GATE_README_OK
