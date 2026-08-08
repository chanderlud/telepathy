#!/usr/bin/env bash
set -euo pipefail

die() {
  printf 'Error: %s\n' "$*" >&2
  exit 1
}

require_command() {
  local command_name="$1"
  local package_hint="$2"

  command -v "$command_name" >/dev/null 2>&1 || die \
    "required command '$command_name' not found; install '$package_hint' before retrying (this script does not install tools)"
}

[[ "$(uname -s)" == "Linux" ]] || die \
  "Linux host required; native Linux DEB/RPM builds cannot run on $(uname -s)"

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
REPO_ROOT="$(cd -- "$SCRIPT_DIR/.." && pwd -P)"
cd "$REPO_ROOT"

[[ -f "$REPO_ROOT/distribute_options.yaml" ]] || die \
  "distribute_options.yaml not found at repository root: $REPO_ROOT"

require_command flutter "Flutter SDK"
require_command flutter_distributor "project-required legacy flutter_distributor dependency"
require_command rpm "RPM tooling (rpm)"
require_command rpmbuild "RPM tooling (rpmbuild)"
require_command dpkg-deb "Debian package tooling (dpkg-deb)"
require_command patchelf "patchelf"
require_command rustup "Rustup"

if ! stable_rustc_info="$(rustup run stable rustc -vV 2>/dev/null)"; then
  die "Rust toolchain 'stable' is unavailable; install Rust toolchain 'stable' before retrying (this script does not install tools)"
fi
stable_host="$(printf '%s\n' "$stable_rustc_info" | sed -n 's/^host: //p')"
[[ -n "$stable_host" ]] || die \
  "could not determine host target from Rust toolchain 'stable'; repair that toolchain before retrying"

if ! rustup target list --toolchain stable --installed | grep -Fxq "$stable_host"; then
  die "Rust target '$stable_host' is not installed for toolchain 'stable'; run 'rustup target add --toolchain stable $stable_host' before retrying (this script does not install tools)"
fi

printf 'Fetching Flutter packages...\n'
flutter pub get

printf 'Building configured Linux DEB and RPM packages...\n'
package_marker="$(mktemp "${TMPDIR:-/tmp}/telepathy-linux-packages.XXXXXX")"
trap 'rm -f -- "$package_marker"' EXIT
flutter_distributor release \
  --name=distribution \
  --jobs=release-distribution-linux-deb,release-distribution-linux-rpm

DIST_DIR="$REPO_ROOT/dist"
mapfile -d '' deb_packages < <(
  find "$DIST_DIR" -type f -name 'telepathy-*-linux.deb' -newer "$package_marker" -print0
)
mapfile -d '' rpm_packages < <(
  find "$DIST_DIR" -type f -name 'telepathy-*-linux.rpm' -newer "$package_marker" -print0
)

if (( ${#deb_packages[@]} != 1 )); then
  die "expected exactly one matching DEB under dist, found ${#deb_packages[@]}"
fi
if (( ${#rpm_packages[@]} != 1 )); then
  die "expected exactly one matching RPM under dist, found ${#rpm_packages[@]}"
fi
[[ -s "${deb_packages[0]}" ]] || die "generated DEB is missing or empty: ${deb_packages[0]}"
[[ -s "${rpm_packages[0]}" ]] || die "generated RPM is missing or empty: ${rpm_packages[0]}"

printf 'DEB: %s\n' "${deb_packages[0]#"$REPO_ROOT/"}"
printf 'RPM: %s\n' "${rpm_packages[0]#"$REPO_ROOT/"}"
