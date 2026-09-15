#!/bin/bash
# Enforce Rust line coverage >= 85% across the workspace.
# Usage: ./scripts/coverage-rust.sh [-- <extra cargo llvm-cov args>]
# Requires: cargo-llvm-cov (already installed) + llvm-tools (in toolchain).
set -euo pipefail
cd "$(dirname "$0")/../rust"
cargo llvm-cov --workspace --fail-under-lines 85 "$@"
