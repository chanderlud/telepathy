#!/usr/bin/env bash
# Runs the app on Linux headlessly (Xvfb + software GL + a private D-Bus
# session), exposing DTD for the dart-mcp-server and enabling the Flutter
# driver extension for automated QA.
#
# A gnome-keyring daemon is started inside the D-Bus session when available
# (the app's secure storage aborts startup when the Secret Service is
# unreachable). Set GNOME_KEYRING_DAEMON to override the daemon path.
set -euo pipefail

cd "$(dirname "$0")/.."

export GDK_BACKEND=x11
export LIBGL_ALWAYS_SOFTWARE=1

KEYRING_DAEMON="${GNOME_KEYRING_DAEMON:-$(command -v gnome-keyring-daemon || true)}"
if [[ -z "$KEYRING_DAEMON" && -x /tmp/xvfb-local/root/usr/bin/gnome-keyring-daemon ]]; then
  KEYRING_DAEMON=/tmp/xvfb-local/root/usr/bin/gnome-keyring-daemon
  export LD_LIBRARY_PATH="/tmp/xvfb-local/root/usr/lib/x86_64-linux-gnu${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
fi
export KEYRING_DAEMON

# gnome-keyring fails with a cryptic "Permission denied" when XDG_RUNTIME_DIR
# points at an unwritable dir (e.g. /run/user/0 outside a login session).
if [[ -z "${XDG_RUNTIME_DIR:-}" || ! -w "${XDG_RUNTIME_DIR:-/nonexistent}" ]]; then
  export XDG_RUNTIME_DIR=/tmp/runtime-telepathy
  mkdir -p "$XDG_RUNTIME_DIR"
  chmod 700 "$XDG_RUNTIME_DIR"
fi

exec xvfb-run \
  --auto-servernum \
  --server-args="-screen 0 1440x900x24 -ac +extension GLX +render -noreset" \
  dbus-run-session -- \
  bash -c '
    if [[ -n "${KEYRING_DAEMON:-}" ]]; then
      echo "" | "$KEYRING_DAEMON" --unlock --components=secrets >/dev/null 2>&1 || true
      "$KEYRING_DAEMON" --start --components=secrets >/dev/null 2>&1 || true
    fi
    exec flutter run \
      -d linux \
      --target=lib/driver_main.dart \
      --print-dtd \
      "$@"
  ' _ "$@"
