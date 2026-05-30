#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
RUST_DIR="${REPO_ROOT}/rust"

cd "${RUST_DIR}"
cargo build --package telepathy-cli

echo "CLI_BINARY=${RUST_DIR}/target/debug/telepathy-cli"
