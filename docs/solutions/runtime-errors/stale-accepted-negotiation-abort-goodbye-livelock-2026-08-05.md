---
title: A Stale Accepted Negotiation Must Abort Without Sending a Goodbye
date: 2026-08-05
category: docs/solutions/runtime-errors/
module: telepathy-core session and call lifecycle
problem_type: runtime_error
component: service_object
severity: high
symptoms:
  - "Caller terminalizes with 'did not respond' while the callee shows Connected"
  - "Under repeated session collisions the caller never terminalizes (infinite supersede/re-drive loop)"
  - "System test call_hello_ack_timeout hangs: caller's CallEnded never arrives"
root_cause: async_timing
resolution_type: code_fix
related_components:
  - session-collision-registry
  - call-slot
  - accept-prompt
tags: [session-collision, hello-ack, livelock, goodbye, concurrency]
---

# A Stale Accepted Negotiation Must Abort Without Sending a Goodbye

## Problem

Two coupled defects around session replacement mid-accept. First, an incoming
direct-call negotiation never re-checked session currency after the accept
prompt resolved, so an accepted call could write its `HelloAck` on a session
the manager had already replaced — the callee Connected while the caller (whose
re-driven Hello landed on the replacement) starved to the HelloAck timeout.
Second, the first attempt at the fix aborted the stale negotiation with the
shared `abort_negotiation_session_stopped`, whose explicit session-stopped
goodbye created a livelock under sustained collision churn: the caller's grace
path treats each such goodbye as superseded-by-replacement and silently
re-drives, so every abort fed the next goodbye and the caller never
terminalized.

## Symptoms

- System test `session_simultaneous_dial_then_call`: callee `Connected`, then
  "The call ended unexpectedly"; caller `CallEnded: "did not respond"`.
- System test `call_hello_ack_timeout` (after the goodbye-sending abort):
  caller's `CallEnded` never arrives within the scenario window.

## What Didn't Work

- **Aborting with the shared helper.** `abort_negotiation_session_stopped`
  writes a `Goodbye { SessionStopped }` before releasing. Under churn, the
  caller's `wait_for_pending_outgoing_replacement` reads that goodbye as
  evidence that a replacement is in flight and re-drives; the re-driven Hello
  hits another stale negotiation, which aborts with another goodbye, and so on.

## Solution

The currency check (extended from the room-only check to all incoming
negotiations in `negotiate_incoming_call`) aborts by releasing the pending
generation *without* writing anything:

```rust
if !is_session_still_current(&self.session_states, peer, io.state.id).await {
    release_pending(&self.session_states, peer, io.state.id, &mut pending_slot).await?;
    return Ok(IncomingNegotiationOutcome::SessionStopped);
}
```

The caller learns about the dead session through the connection teardown that
the stale session's exit already causes (its critical-error path), so no wire
message is needed — and none can be misread.

Deterministic coverage:
`accepted_prompt_on_stale_session_aborts_before_hello_ack` holds the callee's
session-map write lock, accepts, removes the session entry, and releases the
lock; the negotiation must abort, release the slot, and complete no call (the
caller terminalizes either by the session-stopped path or its HelloAck timeout
— the abort's goodbye write races the connection close, so the test accepts
either terminal state and asserts the caller never Connected).

## Why This Works

The abort's only obligation is to free local ownership. The peer's view of the
session ending is carried by the transport (connection close from the stale
session's teardown), which cannot be reinterpreted as a collision-supersession
signal. Removing the explicit goodbye breaks the livelock cycle: supersession
decisions are driven by session-map reality, not by messages authored by a
session that is definitionally out of date.

## Prevention

- Any abort path for a stale session should release locally and stay silent on
  the wire; the teardown's connection close is the authoritative signal.
- Terminal call states must be emitted only after the slot is released (see
  companion doc) so observers never act on a half-torn-down call.
- Under collision machinery, always ask of any new wire message: "what does the
  peer's supersede/re-drive logic make of this?" before sending it.

## Related Issues

- [Accept Prompts Leak When Their Caller Vanishes Across a Session Transfer](accept-prompt-expiry-collision-leaks-2026-08-05.md)
- [Terminal Call States Must Follow Slot Release](terminal-call-state-before-slot-release-2026-08-05.md)
