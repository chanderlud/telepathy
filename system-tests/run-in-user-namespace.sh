#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DISCOVERY_HOST="192.0.2.2"
SLIRP_INTERFACE="tp-slirp0"

if [[ "$(uname -s)" != "Linux" ]]; then
  printf 'namespace preflight failed: Linux or WSL is required.\n' >&2
  exit 2
fi

for command in unshare python3 docker slirp4netns flock; do
  if ! command -v "${command}" >/dev/null 2>&1; then
    printf 'namespace preflight failed: required command %s is missing from PATH.\n' "${command}" >&2
    exit 2
  fi
done

if ! docker compose version >/dev/null 2>&1; then
  printf 'namespace preflight failed: Docker Compose v2 is required.\n' >&2
  exit 2
fi

ARTIFACTS_PARENT="${SYSTEM_TEST_ARTIFACTS_DIR:-${SCRIPT_DIR}/artifacts}"
mkdir -p "${ARTIFACTS_PARENT}"
chmod 700 "${ARTIFACTS_PARENT}"
RUN_ROOT="$(mktemp -d "${ARTIFACTS_PARENT}/run-XXXXXX")"
chmod 700 "${RUN_ROOT}"
STATE_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/telepathy-system-tests-XXXXXX")"
NAMESPACE_STARTED="${STATE_ROOT}/namespace-started"
NAMESPACE_GATE="${STATE_ROOT}/namespace-gate"
SLIRP_READY="${STATE_ROOT}/slirp-ready"
SLIRP_LOG="${RUN_ROOT}/slirp.log"
CHILD_PID=""
SLIRP_PID=""

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
  if [[ -n "${CHILD_PID}" ]] && kill -0 "${CHILD_PID}" 2>/dev/null; then
    touch "${NAMESPACE_GATE}"
    kill -TERM "${CHILD_PID}" 2>/dev/null
    wait "${CHILD_PID}" 2>/dev/null
  fi
  if [[ -n "${SLIRP_PID}" ]] && kill -0 "${SLIRP_PID}" 2>/dev/null; then
    kill -TERM "${SLIRP_PID}" 2>/dev/null
    wait "${SLIRP_PID}" 2>/dev/null
  fi
  bash "${SCRIPT_DIR}/capture-discovery-logs.sh" "${RUN_ROOT}"
  bash "${SCRIPT_DIR}/down.sh"
  rm -rf "${STATE_ROOT}"
  exit "${final_status}"
}
trap 'cleanup $?' EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

bash "${SCRIPT_DIR}/up.sh" "${STATE_ROOT}"

export TELEPATHY_HOST_UID="$(id -u)"
export TELEPATHY_DISCOVERY_HOST="${DISCOVERY_HOST}"
export TELEPATHY_SLIRP_INTERFACE="${SLIRP_INTERFACE}"
export SYSTEM_TEST_ARTIFACTS_DIR="${RUN_ROOT}"
export TELEPATHY_RUN_ARTIFACT_DIR="${RUN_ROOT}"
export TELEPATHY_DISCOVERY_LOG_DIR="${RUN_ROOT}"

unshare \
  --user \
  --map-root-user \
  --net \
  --mount \
  bash -c '
    printf "%s\n" "$$" > "$1"
    while [[ ! -e "$2" ]]; do
      sleep 0.05
    done
    exec python3 "$3" -- "${@:4}"
  ' namespace-launcher "${NAMESPACE_STARTED}" "${NAMESPACE_GATE}" \
    "${SCRIPT_DIR}/harness/namespace_runner.py" "$@" \
  2>"${RUN_ROOT}/runner.log" &
CHILD_PID=$!

STARTED=0
for _ in $(seq 1 200); do
  if [[ -s "${NAMESPACE_STARTED}" ]]; then
    STARTED=1
    break
  fi
  if ! kill -0 "${CHILD_PID}" 2>/dev/null; then
    wait "${CHILD_PID}" || true
    cat "${RUN_ROOT}/runner.log" >&2
    printf 'namespace runner failed; artifacts: %s\n' "${RUN_ROOT}" >&2
    exit 2
  fi
  sleep 0.05
done

if [[ "${STARTED}" -ne 1 ]]; then
  printf 'namespace preflight failed: unshare child did not become ready.\n' >&2
  exit 2
fi

slirp4netns \
  --configure \
  --cidr=192.0.2.0/24 \
  --mtu=1500 \
  --enable-sandbox \
  --enable-seccomp \
  --ready-fd 3 \
  "${CHILD_PID}" "${SLIRP_INTERFACE}" \
  3>"${SLIRP_READY}" >"${SLIRP_LOG}" 2>&1 &
SLIRP_PID=$!

READY=0
for _ in $(seq 1 200); do
  if [[ -s "${SLIRP_READY}" ]]; then
    READY=1
    break
  fi
  if ! kill -0 "${SLIRP_PID}" 2>/dev/null; then
    wait "${SLIRP_PID}" || true
    cat "${SLIRP_LOG}" >&2
    exit 2
  fi
  if ! kill -0 "${CHILD_PID}" 2>/dev/null; then
    wait "${CHILD_PID}" || true
    cat "${RUN_ROOT}/runner.log" >&2
    printf 'namespace runner failed; artifacts: %s\n' "${RUN_ROOT}" >&2
    exit 2
  fi
  sleep 0.05
done

if [[ "${READY}" -ne 1 ]]; then
  printf 'namespace preflight failed: slirp4netns did not become ready.\n' >&2
  exit 2
fi

touch "${NAMESPACE_GATE}"
set +e
wait "${CHILD_PID}"
STATUS=$?
set -e

if [[ "${STATUS}" -ne 0 ]]; then
  cat "${RUN_ROOT}/runner.log" >&2
  printf 'namespace runner failed; artifacts: %s\n' "${RUN_ROOT}" >&2
fi
exit "${STATUS}"
