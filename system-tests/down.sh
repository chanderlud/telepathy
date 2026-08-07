#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

export COMPOSE_PROJECT_NAME="telepathy-system-tests"
docker compose -f "${SCRIPT_DIR}/docker-compose.yml" down
