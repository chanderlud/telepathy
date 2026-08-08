---
title: Stale Room Teardown Goodbye Wedges the Rejoin Negotiation
date: 2026-08-05
category: docs/solutions/runtime-errors/
module: telepathy-core session and room lifecycle
problem_type: runtime_error
component: service_object
severity: high
symptoms:
  - "concurrent_room_end_immediately_rejoins_without_direct_negotiation intermittently hangs 60s in nextest (~8% of runs)"
  - "One client's join_room never observes RoomJoin for a peer while the peer admitted it"
  - "Room mesh ends up asymmetric after a concurrent end_call + immediate rejoin"
root_cause: async_timing
resolution_type: code_fix
related_components:
  - room-controller
  - session-manager
  - call-slot
tags: [room, rejoin, goodbye, race, grace-window, concurrency]
---

# Stale Room Teardown Goodbye Wedges the Rejoin Negotiation

## Problem

When every room member ends the call and immediately rejoins, the previous room
generation's teardown `Goodbye` can cross with the new generation's `Hello`. An
outgoing room negotiation that reads the stale goodbye treated it as the peer
leaving the *current* room and ended silently — wedging the mesh asymmetrically:
the peer admitted the caller, the caller dropped the leg, and all later re-dials
were rejected as duplicate joins.

## Symptoms

- `concurrent_room_end_immediately_rejoins_without_direct_negotiation` hangs
  until the 60s nextest timeout on roughly 1 in 12 runs (2/10 stress iterations).
- Hang trace signature: the caller's gen-2 negotiation logs
  `room_peer_goodbye_during_negotiation` after reading `[gen-1 Goodbye][HelloAck]`
  in stream order; the dialer's room handshake then receives only KeepAlive.
- The caller never emits `RoomJoin(peer)` for generation 2; the peer does.

## What Didn't Work

- **Generation-tagging the Goodbye.** Room generations are per-client local
  counters (`next_room_generation`), so a tagged value is not comparable across
  clients; room hashes intentionally exclude generation so same-member rooms
  match. No wire field can distinguish "stale teardown" from "genuine refusal".
- **Flagging owed goodbyes locally.** "We sent a gen-1 goodbye, so one may still
  arrive" cannot distinguish the stale case from a genuine gen-2 refusal
  (peer never rejoined and answers the new Hello with a goodbye). Ignoring that
  refusal would hang a real leave for the full 10s Hello timeout.
- **Re-driving the leg after the goodbye.** The peer that already admitted the
  caller runs its room handshake loop, which ignores a re-driven Hello as
  `room_handshake_unexpected_message`, so re-drive alone cannot repair the leg.

## Solution

Outgoing room negotiations hold goodbyes in a short grace window instead of
terminating on the first one (`negotiate_outgoing_call` in
`rust/telepathy-core/src/internal/core.rs`):

```rust
const ROOM_GOODBYE_NEGOTIATION_GRACE: Duration = Duration::from_millis(500);

// in the negotiation read loop, before dispatching to the handler:
if is_in_room && matches!(message, ProtocolMessage::Goodbye { .. }) {
    if room_goodbye_grace_deadline.is_none() {
        room_goodbye_grace_deadline = Some(Instant::now() + ROOM_GOODBYE_NEGOTIATION_GRACE);
    }
    continue;
}
```

A dedicated select branch ends the negotiation silently only when the deadline
elapses with no affirmative response. Any `HelloAck`/`Hello` inside the window
completes the leg through the normal tiebreak path. A genuine refusal simply
costs 500ms in a background negotiation task.

The deterministic regression test
(`stale_room_goodbye_during_rejoin_negotiation_survives_until_peer_affirms`)
parks the callee's first room controller inside its `Connected` callback
(`ConnectedCallbackGate`) before it admits the session handshake, so the
callee's session task waits on admission without reading its stream. The caller
ends and rejoins; the callee's `end_call` then abandons the parked callback and
its handshake writes the gen-1 goodbye straight into the caller's parked gen-2
negotiation — the exact flake message order on every run. A log-capture tee in
the test harness (`init_test_tracing` now writes to a global buffer as well as
stdout) lets the test assert the `room_goodbye_during_negotiation_grace` event
fired.

## Why This Works

A goodbye cannot be proven stale on the wire, but it never needs to be: an
affirmative response (`HelloAck`/`Hello`) is the only message that can complete
the negotiation, and in the crossing case it is already queued directly behind
the stale goodbye. Waiting briefly for it converts the silent asymmetry into a
completed leg, while the deadline preserves the genuine-refusal outcome. The
grace approach also covers older peers that send untagged stale goodbyes, which
a protocol change could not.

Two auxiliary races were exposed while building the deterministic test:
the handshake post-loop drains a latched `start_call` permit, so the rejoin's
trigger must wait for the session to reach its idle loop (verified via a
log-line count barrier), and the loop's log capture makes a missed barrier a
loud failure rather than a vacuous pass.

## Prevention

- `stale_room_goodbye_during_rejoin_negotiation_survives_until_peer_affirms`
  fails deterministically without the grace (the grace log event never fires).
- Full core integration suite passes `--stress-count 10` (1000+ runs) with zero
  flakes; the original flaky test passes 30/30 isolated runs (was ~8% hang).
- When a negotiation must treat a terminal-seeming message as suspect, bound
  the suspicion with an explicit deadline and a log event; never drop the
  terminal path.
- Race-test assertions that cannot observe the wire can observe a log-capture
  buffer instead — deterministic and self-validating.

## Related Issues

- [Deferred Session Candidate Resolution Preserves Pending Calls](deferred-session-candidate-resolution-2026-08-01.md) — the sibling grace window (`wait_for_pending_outgoing_replacement`) for direct calls.
- Follow-up: deferred same-identity candidate connections have no reader
  (call Hellos starve) — tracked as a separate open issue.
