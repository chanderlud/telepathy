#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
STATE_ROOT="${1:?discovery state directory is required}"
CERTS_DIR="${STATE_ROOT}/certs"

mkdir -p "${CERTS_DIR}"
bash "${SCRIPT_DIR}/relay/gen-certs.sh" "${CERTS_DIR}"

export COMPOSE_PROJECT_NAME="telepathy-system-tests"
export TELEPATHY_RELAY_CERTS="${CERTS_DIR}"
docker compose -f "${SCRIPT_DIR}/docker-compose.yml" up -d --wait
