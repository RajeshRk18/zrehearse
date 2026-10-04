#!/bin/sh
set -e
cargo build --quiet
cargo clippy --quiet --all-targets -- -D warnings
cargo test --quiet
echo GATE_BUILD_OK
