# Telepathy Tracing Policy

All modules across `telepathy-core`, `telepathy-audio`, and `relay-server` now use `tracing::` macros exclusively. The `LogTracer` bridge has been removed.

Tracing outputs include:

- `telepathy-trace.log` (native in `telepathy-core`): newline-delimited JSON for agent analysis.
- Dart log stream (`telepathy-core`): compact human-readable lines for the Flutter logs UI.
- wasm console layer (`telepathy-core`): browser-compatible tracing output.
- Standard formatter output (`relay-server`): terminal logs controlled by `EnvFilter`.

## Subscriber Setup By Crate

- `telepathy-core`: initialized in `rust_set_up()` via `tracing_subscriber::registry()` with JSON file layer, Dart stream layer, and WASM console layer.
- `relay-server`: initialized in `main()` via `tracing_subscriber::fmt` with `EnvFilter`.
- `telepathy-audio`: library crate, no subscriber initialization; relies on the host application subscriber.

## Structured Vocabulary

| Field | Type | Where used |
|---|---|---|
| `peer.id` | `PeerId` Display | manager, session, call, room, screenshare, relay-server startup |
| `peer.nickname` | `&str` | session, call |
| `session.id` | `Uuid` | session lifecycle |
| `session.role` | `"dialer" \| "listener"` | `session.run` |
| `connection.id` | `ConnectionId` | session manager |
| `relayed` | `bool` | session manager, call setup |
| `latency_ms` | `usize` | ping events, statistics |
| `room.hash` | `Option<u64>` | session, room |
| `call.kind` | `"direct" \| "room" \| "audio_test"` | call handshake / call run |
| `event` | snake_case verb_noun | explicit emits |
| `case` | short id | only with `event = "edge_case"` |
| `retries` | `usize` | manager retry, open session retry |
| `error` | `%Display` of `Error` | error events |
| `device.id` | `&str` | `telepathy-audio` device fallback warnings |
| `address` | `Multiaddr` Display | relay-server listen events |
| `elapsed_ms` | `u64` | capabilities load timing |
| `video.peer_id` | `PeerId` Display | remote owner of a video session |
| `video.session_id` | `VideoSessionId` Display | initiator-created wire identity, echoed by both peers |
| `video.source` | snake_case source id | generic source such as `display` |
| `video.role` | `"sender" \| "receiver"` | local role for this session identity |
| `video.phase` | `"offering" \| "starting" \| "active" \| "stopping" \| "terminal"` | bounded lifecycle transition |
| `video.reason` | snake_case terminal reason | `stopped`, `rejected`, `failed`, `transport_ended`, or `teardown` |
| `video.adapter` | stable adapter id | selected platform adapter, for example `desktop_ffmpeg` or `unsupported` |
| `video.frames` | `u64` | cleanup-summary aggregate frame count |
| `video.bytes` | `u64` | cleanup-summary aggregate payload bytes |
| `video.cleanup_elapsed_ms` | `u64` | elapsed cancellation, process reap, and worker join time |

## Generic Video Lifecycle

Video tracing is lifecycle-only and correlated by `video.peer_id` plus
`video.session_id`. Emit one start transition and one terminal cleanup summary
per local generation. Intermediate phase transitions and failures may be
emitted once when they change the lifecycle outcome.

Do not log payloads, encoded frames, chunks, frame contents, or one event per
frame. Media measurements belong only in bounded, sampled aggregates. A cleanup
summary may include `video.frames`, `video.bytes`, and
`video.cleanup_elapsed_ms`; counters are scoped to one local generation and are
discarded after its joined cleanup.

Use `video.reason` only on terminal/failure events. Use `video.adapter` for the
selected platform boundary, never executable arguments or unbounded process
output. The terminal summary is emitted after adapter termination, child reap,
and worker join, before the video slot is observable as idle.

## Agent Query Examples

```sh
jq 'select(.fields["peer.id"]=="12D3KooW...")' telepathy-trace.log
```

```sh
jq 'select(.fields.event=="edge_case")' telepathy-trace.log
```

```sh
RUST_LOG=telepathy_core=debug,telepathy_audio=info,relay_server=info
```
