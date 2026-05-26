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
