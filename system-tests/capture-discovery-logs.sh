#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ARTIFACT_ROOT="${1:?artifact directory is required}"

mkdir -p "${ARTIFACT_ROOT}"
export COMPOSE_PROJECT_NAME="telepathy-system-tests"
docker compose -f "${SCRIPT_DIR}/docker-compose.yml" logs --no-color iroh-relay \
  >"${ARTIFACT_ROOT}/relay.log" 2>&1 || true
docker compose -f "${SCRIPT_DIR}/docker-compose.yml" logs --no-color iroh-dns-server \
  >"${ARTIFACT_ROOT}/dns.log" 2>&1 || true
