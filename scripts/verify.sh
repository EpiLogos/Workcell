#!/usr/bin/env bash
set -euo pipefail

bash -n scripts/exe-dev-workcell-bootstrap.sh
bash -n scripts/stage-remote-file-share.sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --all-targets
