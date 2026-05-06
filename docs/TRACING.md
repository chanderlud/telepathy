# Telepathy Core Tracing Policy

`core.rs` uses `tracing::` macros and `#[instrument]` spans for structured correlation across async state machines.

All other modules in this workspace continue using `log::` macros. `tracing_log::LogTracer` bridges those records into the same subscriber pipeline, so both styles end up in the same outputs:

- `telepathy-trace.log` (native): newline-delimited JSON for agent analysis.
- Dart log stream: compact human-readable lines for the existing Flutter logs UI.
- wasm console layer: browser-compatible tracing output.

## Structured Vocabulary

| Field | Type | Where used |
|---|---|---|
| `peer.id` | `PeerId` Display | manager, session, call, room, screenshare |
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

## Agent Query Examples

```sh
jq 'select(.fields["peer.id"]=="12D3KooW...")' telepathy-trace.log
```

```sh
jq 'select(.fields.event=="edge_case")' telepathy-trace.log
```

```sh
RUST_LOG=telepathy_core=debug,libp2p=info,wgpu=warn
```
