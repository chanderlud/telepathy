#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

if [[ "${EUID}" -eq 0 ]]; then
  printf 'privileged system tests must start as the calling user; the wrapper uses sudo only for host topology and pytest.\n' >&2
  exit 2
fi

for command in docker flock ip iptables ping tc sysctl sudo; do
  if ! command -v "${command}" >/dev/null 2>&1; then
    printf 'privileged system tests failed: required command %s is missing from PATH.\n' "${command}" >&2
    exit 2
  fi
done

if ! docker compose version >/dev/null 2>&1; then
  printf 'privileged system tests failed: Docker Compose v2 is required.\n' >&2
  exit 2
fi

if ! sudo -n true; then
  printf 'privileged system tests failed: non-interactive sudo is required.\n' >&2
  exit 2
fi

ARTIFACTS_PARENT="${SYSTEM_TEST_ARTIFACTS_DIR:-${SCRIPT_DIR}/artifacts}"
mkdir -p "${ARTIFACTS_PARENT}"
chmod 700 "${ARTIFACTS_PARENT}"
RUN_ROOT="$(mktemp -d "${ARTIFACTS_PARENT}/run-XXXXXX")"
chmod 700 "${RUN_ROOT}"
STATE_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/telepathy-system-tests-XXXXXX")"
OLD_IP_FORWARD="$(sysctl -n net.ipv4.ip_forward)"

LOCK_FILE="${XDG_RUNTIME_DIR:-/tmp}/telepathy-system-tests.lock"
exec 9>"${LOCK_FILE}"
if ! flock -n 9; then
  printf 'system tests failed: another discovery Compose run holds %s.\n' "${LOCK_FILE}" >&2
  exit 2
fi

cleanup() {
  local final_status="$1"
  trap - EXIT INT TERM
  set +e
  bash "${SCRIPT_DIR}/capture-discovery-logs.sh" "${RUN_ROOT}"
  bash "${SCRIPT_DIR}/down.sh"
  sudo ip addr del 100.64.0.1/32 dev lo 2>/dev/null
  sudo sysctl -w "net.ipv4.ip_forward=${OLD_IP_FORWARD}" >/dev/null
  sudo chown -R "$(id -u):$(id -g)" "${RUN_ROOT}"
  rm -rf "${STATE_ROOT}"
  exit "${final_status}"
}
trap 'cleanup $?' EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

bash "${SCRIPT_DIR}/up.sh" "${STATE_ROOT}"
sudo sysctl -w net.ipv4.ip_forward=1 >/dev/null
unset TELEPATHY_DISCOVERY_HOST TELEPATHY_SLIRP_INTERFACE TELEPATHY_RUN_ARTIFACT_DIR
python3 "${SCRIPT_DIR}/wait-for-discovery.py" 127.0.0.1

export SYSTEM_TEST_ARTIFACTS_DIR="${RUN_ROOT}"
export TELEPATHY_DISCOVERY_LOG_DIR="${RUN_ROOT}"

set +e
sudo -E \
  SYSTEM_TEST_ARTIFACTS_DIR="${RUN_ROOT}" \
  TELEPATHY_DISCOVERY_LOG_DIR="${RUN_ROOT}" \
  "$@"
STATUS=$?
set -e
printf 'pytest exit status: %s\n' "${STATUS}" >"${RUN_ROOT}/runner.log"
printf 'system-test artifacts: %s\n' "${RUN_ROOT}" >&2
exit "${STATUS}"
