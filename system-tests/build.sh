set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
RUST_DIR="${REPO_ROOT}/rust"

cd "${RUST_DIR}"
cargo build --package relay-server --package telepathy-cli

echo "RELAY_BINARY=${RUST_DIR}/target/debug/relay-server"
echo "CLI_BINARY=${RUST_DIR}/target/debug/telepathy-cli"
