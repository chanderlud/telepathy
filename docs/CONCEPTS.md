# Concepts

Shared domain vocabulary for this project — entities, named processes, and status concepts with project-specific meaning. Seeded with core domain vocabulary, then accretes as ce-compound and ce-compound-refresh process learnings; direct edits are fine. Glossary only, not a spec or catch-all.

## Identity Switching

### Prepared Identity Switch
A one-use capability representing a validated target identity and contact snapshot whose commit is bound to the origin CoreState that prepared it.

It holds the exclusive session and call-slot resources required during preparation; dropping it abandons the prepared operation without releasing a later owner's resources.

### Desired Runtime
The identity and contact configuration that the session manager is expected to apply next.

### Runtime Revision
A wrapping identifier advanced for each desired runtime replacement, used to distinguish applied, superseded, and failed readiness requests.

### Runtime Readiness
The state in which the session manager has applied the requested runtime revision, making identity-dependent activity safe to start.

## Session Establishment

### Direct Session Attempt
A peer-scoped request to establish a direct session, identified so that completion and terminal outcomes apply only to the attempt that initiated them.

An attempt remains active while direct dialing or an associated inbound candidate can still publish a usable session; terminal outcomes and runtime teardown end it.

### Session Availability
The observable per-peer state that tells a call request whether a direct session already exists, a direct session attempt may still publish one, or no session can be expected.

Availability changes wake waiting call requests; a request only acquires call ownership after the session is published.

## Session Collisions

### Session
The per-peer state machine wrapping one connection; each peer has at most one current session in the session map. A session outlives individual calls and rooms.

### Glare
Both peers dial each other at the same time, producing two connections. Resolution is deterministic: the connection on which the lower-pubkey side is the client wins on both ends.

### Deferred Candidate
When an existing session is kept over an incoming replacement connection, the replacement parks as a candidate until the predecessor session finishes, then promotes. While parked, the candidate's connection has no reader: call messages sent to it are dropped (issue #81).

## Call Lifecycle

### Call Slot
The single global call ownership token. States: `Idle`, `PendingOutgoing`, `PendingIncoming`, `ActiveDirect`, `RoomCall`, `AudioTest`.

### Call-Slot Generation
A monotonic token bumped on every idle-to-non-idle transition and preserved across a simultaneous-dial match. Exact-generation release ensures a stale holder can never release a re-acquired slot.

### Parked Accept Prompt
A pending incoming-call prompt displaced when the callee's own outbound dial wins a collision mid-prompt. The platform prompt stays open in the transfer registry; the replacement session's re-driven negotiation adopts it. Expires after the caller's offer window (`HELLO_TIMEOUT`) if never adopted.

## Room Lifecycle

### Room Generation
A per-client counter incremented on each room join; orders local room replacements. Not comparable across clients and not part of the room hash (the hash covers members only).

### Goodbye Grace
An outgoing room negotiation that receives a goodbye mid-negotiation waits briefly (500ms) for an affirmative response before ending, because a teardown goodbye from the previous room generation can cross with a fresh join.

## Home Screen Layout

### Wide layout
The home-screen arrangement used when the window is wider than the project's shared width breakpoint: the contacts area and the call area sit side by side. At or below the breakpoint the same content stacks in a single-column narrow arrangement.

### Compact layout
A height-driven density mode entered when the window is shorter than the project's height breakpoint, independent of width. Compact trims vertical chrome across the home screen — paddings, section caps, and dispensable visuals shrink or drop — and combines with Wide layout, so a short-but-wide window runs a "compact wide" arrangement.

Compact is decided from height alone; width-gated variants exist for subtrees that only apply in the narrow arrangement. A subtree that reads a different variant than the one that chose its branch is a recurring overflow source.

### Session Manager
The Rust background process that owns the app's networking and session lifecycle. The contacts header surfaces it as a status chip with a restart action when it enters its failed state.
