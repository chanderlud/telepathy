#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CERTS_DIR="${1:?certificate output directory is required}"

mkdir -p "${CERTS_DIR}"
cd "${SCRIPT_DIR}/certgen"
cargo run --quiet -- -o "${CERTS_DIR}"
