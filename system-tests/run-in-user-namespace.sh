#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

if [[ "$(uname -s)" != "Linux" ]]; then
  printf 'namespace preflight failed: Linux or WSL is required; no privilege fallback is supported.\n' >&2
  exit 2
fi

for command in unshare python3; do
  if ! command -v "${command}" >/dev/null 2>&1; then
    printf 'namespace preflight failed: required command %s is missing from PATH.\n' "${command}" >&2
    exit 2
  fi
done

export TELEPATHY_HOST_UID="$(id -u)"
python3 "${SCRIPT_DIR}/harness/namespace_runner.py" --prepare-cache
ARTIFACTS_PARENT="${SYSTEM_TEST_ARTIFACTS_DIR:-${SCRIPT_DIR}/artifacts}"
mkdir -p "${ARTIFACTS_PARENT}"
chmod 700 "${ARTIFACTS_PARENT}"
PREFLIGHT_ROOT="$(mktemp -d "${ARTIFACTS_PARENT}/preflight-XXXXXX")"
chmod 700 "${PREFLIGHT_ROOT}"

set +e
unshare \
  --user \
  --map-root-user \
  --net \
  --mount \
  python3 "${SCRIPT_DIR}/harness/namespace_runner.py" -- "$@" \
  2>"${PREFLIGHT_ROOT}/runner.log"
STATUS=$?
set -e

if [[ "${STATUS}" -ne 0 ]]; then
  cat "${PREFLIGHT_ROOT}/runner.log" >&2
  if rg -q '^namespace preflight failed:' "${PREFLIGHT_ROOT}/runner.log"; then
    printf 'namespace preflight failed; artifacts: %s\n' "${PREFLIGHT_ROOT}" >&2
  else
    printf 'namespace runner failed; artifacts: %s\n' "${PREFLIGHT_ROOT}" >&2
  fi
  exit "${STATUS}"
fi

rm -rf "${PREFLIGHT_ROOT}"
