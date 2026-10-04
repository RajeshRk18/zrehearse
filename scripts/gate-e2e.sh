#!/bin/sh
# Runs a real rehearsal. Needs Docker and the zfnd/zebra:6.2.3 image.
set -e
rm -rf out/e2e
cargo run --quiet -- run examples/nu6_3.toml --out out/e2e > out-e2e.log 2>&1 || { cat out-e2e.log; exit 1; }
grep -q "REHEARSAL PASSED" out-e2e.log
grep -q '"passed": true' out/e2e/report.json
grep -q '"name": "rpc-smoke"' out/e2e/report.json
test -z "$(docker ps -aq --filter label=zrehearse)"
rm -f out-e2e.log
echo GATE_E2E_OK
