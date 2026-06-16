#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

bash "${SCRIPT_DIR}/relay/gen-certs.sh"
docker compose -f "${SCRIPT_DIR}/docker-compose.yml" up -d --wait
