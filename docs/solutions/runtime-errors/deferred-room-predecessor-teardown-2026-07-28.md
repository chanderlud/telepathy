---
title: Deferred Room Predecessor Teardown Preserves Membership When a Replacement Aborts
date: 2026-07-28
category: docs/solutions/runtime-errors/
module: telepathy-core session lifecycle and room controller
problem_type: runtime_error
component: service_object
severity: high
symptoms:
  - Inbound replacement session wins the collision map entry, then aborts before being admitted to the room, leaving the map owned by a dead session
  - Room membership observes an unexplained `Leave` while the room is otherwise stable
  - The predecessor's connection is closed by collision-loser teardown but no replacement is admitted, producing a window with no session at all
  - "`session_collision_kept_new` log line, then silence, then a `RoomJoin` from a different peer that does not arrive"
root_cause: async_timing
resolution_type: code_fix
related_components:
  - session-manager
  - room-controller
tags: [room-races, session-collision, deferred-teardown, predecessor-restore, room-handshake, arc-strong-count]
---

# Deferred Room Predecessor Teardown Preserves Membership When a Replacement Aborts

## Problem

`initialize_session` resolves collisions by replacing the existing `SessionState` in the map and tearing down the loser. For direct calls this is correct: the new connection is the new session. For room sessions it is unsafe, because a collision-winning replacement may never be admitted to the room — its `room_handshake` can abort (cancellation, stale generation, peer rejection), or its owning task can shut down before room negotiation completes. Tearing down the predecessor eagerly orphans the map entry: the predecessor's room connection is gone, and the replacement never reaches `RoomJoin`.

Two failure modes follow:

1. The replacement aborts. The map still contains the (now-finished) replacement `SessionState`. The remote peer's room connection has no local counterpart; from the room controller's view, the local peer has silently left.
2. A third session (e.g., a same-identity client's reconnect) displaces the replacement before it exits. The replacement's task finishes; its predecessor is still deferred; the predecessor's `Arc` is never dropped, leaking its task and connection.

## Symptoms

- `RoomLeave` event delivered to the frontend during stable room membership.
- `session_states.get(&peer)` returns a `SessionState` whose task has exited but whose predecessor was torn down.
- `Arc<SessionState>` strong-count for the predecessor never drops to zero after the replacement aborts.
- The remote peer observes a connection close (room peer departure) without a corresponding local `end_call`.

## What Didn't Work

- **Eagerly tearing down the predecessor on collision-win.** Closed the cold-replacement window: if the replacement then aborted, the map had no usable session.
- **Always restoring the predecessor when the replacement finishes.** Re-opened a different race: if a third session had taken the map entry in the meantime, restoring would have displaced the newer session.
- **Tracking predecessor state on the room controller.** The room controller does not own the `session_states` map; plumbing restoration through it would have required holding a lock across an async boundary.

## Solution

All changes in `rust/telepathy-core/src/internal/core.rs` and `internal/state.rs`.

**1. Defer predecessor teardown on the new session when the peer is in a room.**

```rust
if self.is_in_room(&peer).await {
    state.defer_room_predecessor(old_state).await;
} else {
    old_state.teardown().await;
}
```

`defer_room_predecessor` stores the predecessor `Arc<SessionState>` in a `Mutex<Option<Arc<SessionState>>>` on the new state. The predecessor's tasks and connection keep running. The new session runs to completion normally.

**2. Set the new session's per-peer volume before deferring.**

`set_peer_output_volume(&contact)` runs *before* the collision check, so the new session is fully configured even if it is later restored to the predecessor. Without this, restoring the predecessor would lose the volume setting.

**3. On session exit, either restore or tear down.**

```rust
state.mark_finished();
let restored_room_predecessor =
    if let Some(predecessor) = state.take_deferred_room_predecessor().await {
        let mut states = self.session_states.write().await;
        if predecessor.can_restore_room_predecessor()
            && states.get(&peer).is_some_and(|current| current.id == state.id)
        {
            states.insert(peer, predecessor);
            true
        } else {
            drop(states);
            predecessor.teardown().await;
            false
        }
    } else {
        false
    };
if restored_room_predecessor {
    connection.close(VarInt::from_u32(0), b"replacement aborted");
}
```

`mark_finished()` distinguishes "session task exited" from "session was cancelled" — restoration is only valid for a finished session whose map entry is still itself.

`can_restore_room_predecessor()` gates restoration:

```rust
pub(crate) fn can_restore_room_predecessor(&self) -> bool {
    !self.stop_session.is_cancelled() && !self.is_finished()
}
```

- The predecessor must not have had its `stop_session` token cancelled (e.g., by `reset_sessions` or manager shutdown).
- The predecessor must not already be finished (a predecessor that exited concurrently cannot serve as a restore target).

The map-entry check (`current.id == state.id`) closes the third-session-displaces-replacement race: if a newer session has taken the map entry, restoration is skipped and the predecessor is torn down instead.

**4. If restored, close the replacement's connection.**

`connection.close(...)` ensures the remote peer sees the replacement go away, so its side of the room connection is cleaned up. The predecessor's connection is still alive (it was never torn down).

## Why This Works

Deferral moves the predecessor teardown decision out of collision-resolution time (when we cannot know whether the replacement will succeed) into session-exit time (when we know definitively). At session exit, four cases are distinguishable:

| Replacement outcome          | Map state at exit                  | Action                          |
|------------------------------|------------------------------------|---------------------------------|
| Aborted, map still self      | `current.id == state.id`           | Restore predecessor            |
| Aborted, map displaced       | `current.id != state.id`           | Tear down predecessor          |
| Predecessor cancelled        | `!predecessor.can_restore_...`     | Tear down predecessor          |
| No predecessor deferred      | `None`                             | No-op                          |

The restore condition is conservative on every dimension: it requires (a) a deferred predecessor, (b) the predecessor still viable, and (c) the map still owned by the exiting session. Anything else results in teardown, which is the safe default.

The `Arc::strong_count(&predecessor) >= 4` wait in `room_replacement_map_mismatch_tears_down_deferred_predecessor` is the test-side verification that deferral actually happens — a fresh `SessionState` has count 1, +1 in the map, +1 in the replacement's `deferred_room_predecessor`, +1 in the test's local binding.

## Prevention

- **Do not tear down a session you may need to restore.** When a replacement's success depends on a future event (handshake, negotiation, admission), defer the predecessor's teardown until the replacement's outcome is known.
- **Distinguish "cancelled" from "finished" on long-lived state objects.** A `CancellationToken` for stop plus a separate `mark_finished()` for natural exit lets restoration logic tell the cases apart.
- **Re-check map ownership before restoring.** Holding a write lock on the map at restore time and comparing `current.id == state.id` closes the third-session-displacement race.
- **Order side effects before deferral.** Any setup that the predecessor-or-replacement will need (volume, configuration) must run before the defer call, so both code paths see a fully-configured state.

### Test coverage

- `room_session_collision_handoff_keeps_membership_until_replacement_is_admitted` (room_lifecycle.rs) — a same-identity replacement that never starts room negotiation is shut down; asserts the old session ID is restored to the map, no `RoomLeave` is observed, and the room connection remains effective throughout.
- `room_replacement_map_mismatch_tears_down_deferred_predecessor` (room_lifecycle.rs) — a third session state takes the map entry before the replacement exits; asserts both the replacement and the deferred predecessor are torn down (their `Weak` handles drop) and the remote peer's session is removed. Uses `Arc::strong_count` and `Weak::upgrade` polling with bounded timeouts to verify lifecycle without sleeping.

## Related Issues

- [Pending Room Admission Registry](./pending-room-admission-registry-2026-07-28.md) — admission is what determines whether a replacement reaches `RoomJoin` or aborts; the deferral pattern here handles the abort case.
- [Room Dial Reconciliation](./room-dial-reconciliation-2026-07-28.md) — `request_room_reconcile` fires from session install and cleanup, so the scheduler launches a fresh dial for the restored predecessor if needed.
- Commit `9204217` introduced the deferral mechanism; commit `7f77964` added the `can_restore_room_predecessor` gate and the map-mismatch teardown in response to a P2 review finding (predecessor was previously orphaned without teardown when the map had been displaced).
