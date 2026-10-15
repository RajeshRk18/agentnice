#!/usr/bin/env bash
# The only list of check commands. CI calls this script.
set -euo pipefail
cd "$(dirname "$0")/.."

mode="${1:-quick}"
case "$mode" in
  quick)
    cargo fmt --check
    cargo clippy --all-targets -- -D warnings
    cargo test
    ;;
  ci-linux)
    cargo fmt --check
    cargo clippy --locked --all-targets -- -D warnings
    cargo test --locked
    cargo build --locked --release
    ;;
  *)
    echo "usage: $0 [quick|ci-linux]" >&2
    exit 2
    ;;
esac
