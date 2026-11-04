#!/bin/sh
set -eu

repository=$(cd "$(dirname "$0")/.." && pwd)

cargo build --quiet --release --manifest-path "$repository/Cargo.toml" -p steep -p smoke-test-runner
exec "${CARGO_TARGET_DIR:-$repository/target}/release/smoke-test-runner" "$@"
