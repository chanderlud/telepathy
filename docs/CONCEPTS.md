# Concepts

> Shared domain vocabulary for this project — entities, named processes, and status concepts with project-specific meaning. Seeded with core domain vocabulary, then accretes as ce-compound and ce-compound-refresh process learnings; direct edits are fine. Glossary only, not a spec or catch-all.

## Session & connection lifecycle

- **Session** — the per-peer state machine wrapping one QUIC connection; each peer has at most one *current* session in `session_states`. A session outlives individual calls and rooms.
- **Glare** — both peers dial each other at the same time, producing two connections. Resolution is deterministic: `should_keep_new_session` keeps the connection on which the lower-pubkey side is the client, on both ends.
- **Session collision outcome** — one of three: `kept_new` (the new connection replaces the old session immediately), `kept_existing` (the new connection is torn down), or *deferred candidate*.
- **Deferred candidate** — when the existing session is kept, the incoming replacement connection parks as a candidate until the predecessor session finishes; it then promotes. Known gap: while parked, the candidate's connection has no reader, so call messages sent to it are dropped (issue #81).

## Call lifecycle

- **Call slot** — the single global call ownership token. States: `Idle`, `PendingOutgoing`, `PendingIncoming`, `ActiveDirect`, `RoomCall`, `AudioTest`.
- **Call-slot generation** — a monotonic token bumped on every Idle→non-idle transition and preserved across a simultaneous-dial match. Exact-generation release (`release_if_match`) ensures a stale holder can never release a re-acquired slot.
- **Parked accept prompt** — a pending incoming-call prompt displaced when the callee's own outbound dial wins a collision mid-prompt. The platform prompt stays open in the transfer registry; the replacement session's re-driven negotiation adopts it. Expires after `HELLO_TIMEOUT` (the caller's offer window) if never adopted.

## Room lifecycle

- **Room generation** — a per-client counter incremented on each `join_room`; orders local room replacements. Not comparable across clients, and not part of the room hash (the hash covers members only).
- **Goodbye grace** — an outgoing room negotiation that receives a `Goodbye` mid-negotiation waits up to 500ms for an affirmative response before ending, because a teardown goodbye from the previous room generation can cross with a fresh join.
