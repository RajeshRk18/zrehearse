#!/bin/sh
# The failing example must make the run fail, and cleanup must still happen.
rm -rf out/neg
if cargo run --quiet -- run examples/failing_project.toml --out out/neg > out-neg.log 2>&1; then
  echo "expected a non-zero exit"; cat out-neg.log; exit 1
fi
grep -q "REHEARSAL FAILED" out-neg.log || { cat out-neg.log; exit 1; }
grep -q '"passed": false' out/neg/report.json || exit 1
test -z "$(docker ps -aq --filter label=zrehearse)" || { echo "container left behind"; exit 1; }
rm -f out-neg.log
echo GATE_NEGATIVE_OK
