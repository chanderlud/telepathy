---
name: linux-gui-debugging
description: Run and drive the Telepathy Flutter app (or two P2P instances) headlessly on Linux — Xvfb, keyring, xdotool/scrot UI automation, DTD access
---

# Linux GUI Debugging (headless)

Run the real Flutter UI on a headless Linux box, drive it with
`xdotool`/`scrot`, and attach the dart-mcp-server over DTD. Use this for
layout bugs, visual regressions, and anything widget tests can't reproduce
faithfully (real fonts, real icon metrics, real backend state).

## When to Use

- UI bug reports that widget tests don't reproduce (font/asset metric gaps)
- Verifying layout across window sizes against the real renderer
- Flows needing a live backend (sessions, incoming/outgoing calls)

## Single Instance

```sh
./scripts/run-linux-debug.sh
```

The script (`scripts/run-linux-debug.sh`) handles the environment pitfalls:

- `xvfb-run --auto-servernum` — virtual display (1440x900)
- `LIBGL_ALWAYS_SOFTWARE=1` — no GPU required
- `dbus-run-session` + `gnome-keyring-daemon --unlock` (empty password) —
  without this, `FlutterSecureStorage` throws `KeyringLocked` and the app
  dies during profile init. If the daemon is not on PATH, the script falls
  back to `/tmp/xvfb-local/root/usr/bin/gnome-keyring-daemon` (see
  "Local .deb extraction" below) and sets `LD_LIBRARY_PATH`.
- `XDG_RUNTIME_DIR` — gnome-keyring fails with a cryptic
  `/run/user/0: Permission denied` when it points at an unwritable dir;
  the script substitutes `/tmp/runtime-telepathy`.
- `--target=lib/driver_main.dart` — entrypoint that calls
  `enableFlutterDriverExtension()` before booting the real app, so the
  dart-mcp-server `flutter_driver_command` tool works.
- `--print-dtd` — prints the Dart Tooling Daemon URI for
  `dart-mcp-server_dtd connect`.

### Finding the display and authority

`xvfb-run --auto-servernum` picks the display and generates an
Xauthority file. Discover both:

```sh
ps aux | grep "[X]vfb"                 # note the :N display and -auth path
ls -t /tmp | grep xvfb-run | head -1   # newest auth dir
export DISPLAY=:N XAUTHORITY=/tmp/xvfb-run.XXXX/Xauthority
```

### Driving the UI

```sh
xdotool search --name "Telepathy"      # window ids; pick the one with a real geometry
xdotool getwindowgeometry <id>
xdotool windowsize <id> 807 910        # reproduce a reported window size
xdotool mousemove X Y click 1          # tap
xdotool type --delay 15 "text"         # text input
scrot /tmp/shot.png                    # screenshot
```

Always screenshot after resizing/clicking to confirm coordinates — dialogs
recenter when the window size changes, so re-measure instead of reusing
old coordinates. Dialogs also recenter when their *content* changes
(e.g. adding a member chip in Add Room), so re-screenshot after any
state-changing click before aiming the next one.

`look_at` on a screenshot is a fast way to get widget coordinates, but it
can time out waiting on its analysis session. The `Read` tool renders
PNGs directly and is the reliable fallback.

### Text input and finder pitfalls (flutter_driver over MCP)

- `xdotool type` is a silent no-op against Flutter `TextField`s — the
  keystrokes never reach the framework. Instead: focus the field with an
  `xdotool` click, then use the dart-mcp-server
  `flutter_driver_command` with `command=enter_text`. Called with only
  `text` (no finder) it types into the currently focused field, which
  sidesteps finder ambiguity entirely.
- `ByType` finders fail with `Bad state: Too many elements` whenever more
  than one widget of that type is on screen (multiple `TextField`s,
  `IconButton`s). Prefer `ByText` on unique labels, or fall back to
  screenshot + `xdotool` coordinates.
- `Descendant`/`Ancestor` finders passed through the MCP tool can hang
  ("Timed out waiting for Flutter Driver response") even when the driver
  is healthy (`get_health` ok). Don't retry them; use coordinates.

Clipboard (for peer IDs etc.):

```sh
xclip -selection clipboard -o
```

### DTD / dart-mcp-server

```text
dart-mcp-server_dtd listDtdUris   # or read the URI from the run log
dart-mcp-server_dtd connect uri=ws://127.0.0.1:PORT/TOKEN=
```

Then `widget_inspector`, `get_runtime_errors`, `hot_reload`, and
`flutter_driver_command` (e.g. `get_offset` by semantics label) are
available.

## Mock Mode (no Rust backend)

`lib/mock_main.dart` boots the real app UI against the fake backend in
`lib/core/testing/mock_backend.dart` — seeded contacts, rooms, session
states, and a simulated call lifecycle (`MockTelepathy` drives
`StateController` through the same public transitions the real callbacks
use). No Rust build, no network, no keyring profile needed. Run it with:

```sh
TARGET=lib/mock_main.dart ./scripts/run-linux-debug.sh \
  --dart-define=MOCK_SCENARIO=<demo|room-active|empty>
```

- `demo` (default): 5 contacts (online/connecting/offline/inactive) + 2 rooms
- `room-active`: starts already inside a room call with peers online
- `empty`: fresh profile, no contacts

Calls and room joins succeed after a short delay; room members trickle in.
Use this for visual iteration on contacts/rooms/call UI without two real
instances. Text entry does not work through `xdotool` (GTK input method);
drive text-dependent flows with widget tests or the clipboard paste paths.

## Two Instances (P2P flows)

Calls/sessions need two real peers. Run a second, fully isolated instance
from the already-built bundle — do **not** run a second `flutter run` in
the same worktree (both write to `build/`, which corrupts the build).

```sh
HOME=/tmp/inst2home \
XDG_CONFIG_HOME=/tmp/inst2/config \
XDG_DATA_HOME=/tmp/inst2/data \
XDG_RUNTIME_DIR=/tmp/runtime-t2 \
KEYRING_DAEMON=/tmp/xvfb-local/root/usr/bin/gnome-keyring-daemon \
LD_LIBRARY_PATH=/tmp/xvfb-local/root/usr/lib/x86_64-linux-gnu \
GDK_BACKEND=x11 LIBGL_ALWAYS_SOFTWARE=1 \
xvfb-run --auto-servernum \
  --server-args="-screen 0 1440x900x24 -ac +extension GLX +render -noreset" \
  dbus-run-session -- bash -c '
    echo "" | "$KEYRING_DAEMON" --unlock --components=secrets >/dev/null 2>&1
    "$KEYRING_DAEMON" --start --components=secrets >/dev/null 2>&1
    exec ./build/linux/x64/debug/bundle/telepathy'
```

Isolation notes:

- `shared_preferences` lives under `$XDG_DATA_HOME/<app-id>/` — separate
  profiles/contacts per instance.
- `HOME=/tmp/inst2home` with an `.asoundrc` inside is **required** on
  machines without a sound card; the incoming-call ringtone otherwise
  hangs the renderer (black window). Use:

  ```
  pcm.!default { type null }
  ctl.!default { type null }
  ```

- `XDG_RUNTIME_DIR` must differ per instance (separate keyrings).

### Connecting the peers

1. In each instance: Settings → Profiles → copy Peer ID (icon next to the
   profile; read it back with `xclip -selection clipboard -o`).
2. Add each instance as a contact of the other (Contacts `+` → Add Contact
   → nickname + peer ID). Both directions are required — a peer with no
   matching contact is dropped as `unknown_peer_connected`.
3. Sessions show `relayed usw1-1` once connected. Place the call from one
   side, accept on the other.

Watch `telepathy-trace.log.<date>` in the repo root for session events —
note both instances write to the same file when run from one worktree.

## Local .deb extraction (no root)

If `Xvfb`, `gnome-keyring-daemon`, or `xclip` are missing and there is no
sudo, extract packages locally. Do not assume `/tmp/xvfb-local` already
exists — check first (`ls /tmp/xvfb-local/root/usr/bin/`); on a fresh box
you must run the extraction below before `run-linux-debug.sh` can find a
keyring daemon:

```sh
mkdir -p /tmp/xvfb-local && cd /tmp/xvfb-local
apt-get download xvfb xserver-common libxfont2 libxkbfile1 x11-xkb-utils \
  libfontenc1 xdotool libxdo3 xclip scrot gnome-keyring \
  libgcr-base-3-1 libgcr-ui-3-1 libgck-1-0 libsecret-1-0
for f in *.deb; do dpkg -x "$f" root; done
export LD_LIBRARY_PATH=/tmp/xvfb-local/root/usr/lib/x86_64-linux-gnu
export PATH=/tmp/xvfb-local/root/usr/bin:$PATH
```

## Cleanup

Prefer graceful shutdown over `pkill` — hard kills have corrupted
`build/linux` ninja state before (fix: `rm -rf build/linux` and rebuild).

```sh
tmux send-keys -t <session> q       # flutter run quits, xvfb-run cleans up
```

Verify nothing is left with `ps aux | grep -E "[X]vfb|[g]nome-keyring-daemon"`.
Never kill `dart mcp-server`, codegraph, or other agent tooling processes.
