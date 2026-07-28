---
title: Pending Room Admission Registry Closes the Cold-Join Authorization Window
date: 2026-07-28
category: docs/solutions/runtime-errors/
module: telepathy-core room controller and session manager
problem_type: runtime_error
component: service_object
severity: high
symptoms:
  - Inbound room peer dials rejected with `unknown_peer_connected` while the local join_room operation is still between slot acquire and room_state publication
  - "\"Group contact\" room peers (not in the contact list) get their inbound connection closed before the room handshake can run"
  - Call slot leaks held in `RoomCall` after a cancellation during the publication window
  - Stale-generation room handshakes route to a freshly-started different room's channels
root_cause: async_timing
resolution_type: code_fix
related_components:
  - session-manager
  - call-slot
  - room-handshake
tags: [room-races, pending-admission, cold-join, call-slot, atomic-publish, drop-guard, room-handshake]
---

# Pending Room Admission Registry Closes the Cold-Join Authorization Window

## Problem

`join_room_with_operation` acquired the `RoomCall` slot, then performed several awaits (cancellation checks, room-state write lock, member iteration) before publishing `RoomState`. During that window:

- `session_manager`'s inbound-connection handler called `is_in_room(peer)`, which only checked `room_state` (still `None`). Peers not in the contact list were closed with `unknown_peer_connected` before the room handshake could run.
- A cancellation between `try_acquire` and `RoomState` publication left the call slot held in `RoomCall` with nothing to release it.
- `room_handshake_snapshot` returned the active room's `(sender, cancel)` for any caller regardless of membership or generation, so a stale-generation handshake could route to the wrong room's channel.

## Symptoms

- `unknown_peer_connected` log lines for peers that are members of the room being joined.
- Group-only contacts (synthesized `Contact { is_room_only: true, .. }`) get their inbound dial closed during a `join_room` race window.
- `CallAlreadyActive` returned by a subsequent `join_room` because the previous attempt's slot was never released.
- Stale room-generation handshakes deliver `RoomMessage`s to the wrong room's task.

## What Didn't Work

- **Checking `is_in_room` alone for inbound authorization.** That helper only inspects `room_state`, which is `None` between slot acquire and publication. The window is small but the race is deterministic under cold joins (peer A's dial lands while peer B is still in `setup_call`).
- **Calling `try_acquire` then `snapshot` separately.** A yield between them meant the snapshot could belong to a different (superseding) acquirer. The fix had to combine acquire + snapshot under one mutex acquisition.
- **Manually invoking `abort_pending_room_joins` at every early-exit site.** Sixteen call sites accumulated; missing one leaked the call slot. The pattern was unsustainable as `join_room_with_operation` grew more branches.

## Solution

Three coordinated changes, all in `rust/telepathy-core/src/internal.rs`, `internal/core.rs`, `internal/helpers.rs`, and `internal/state.rs`.

**1. Acquire + snapshot atomically; install admission before any await.**

`CallSlot::try_acquire_with_snapshot(CallSlotState::RoomCall)` returns the owning snapshot under one mutex acquisition. The admission lease is installed immediately, before the cancellation check, the room-state write, or member iteration:

```rust
let Some(room_owner) = self
    .inner
    .core_state
    .call_slot
    .try_acquire_with_snapshot(CallSlotState::RoomCall)?
else {
    return Err(ErrorKind::CallAlreadyActive.into());
};
// No await/yield is allowed between slot acquisition and admission publication.
let mut pending_admission = Some(
    self.inner
        .install_pending_room_admission(room_owner, &members),
);
```

The registry entry records the owning snapshot, the parsed member set, and the `expected_room_hash`. It is the source of truth for "is this peer authorized for the room currently being assembled" until `RoomState` publishes.

**2. `is_admitted_room_peer` closes the cold-join window for inbound authorization.**

```rust
async fn is_admitted_room_peer(&self, peer: &PublicKey) -> Result<bool> {
    let owner = self.core_state.call_slot.snapshot()?;
    if self.pending_room_admission.allows(peer, owner) {
        return Ok(true);
    }
    Ok(self.is_in_room(peer).await)
}
```

The inbound-connection handler now consults the pending registry first; if the local node is mid-`join_room` for a room containing the dialing peer, the connection is admitted even though `room_state` is still `None`.

**3. `PendingRoomJoinGuard` automates abort-on-exit via `Drop`.**

`join_room_with_operation` constructs `PendingRoomJoinGuard(Receiver<RoomMessage>)` whose `Drop` impl drains the receiver and sends `RoomJoinAdmission::Aborted` to every queued `Join`:

```rust
impl Drop for PendingRoomJoinGuard {
    fn drop(&mut self) {
        abort_pending_room_joins(&mut self.0);
    }
}
```

When the published `RoomState` is ready, `pending_admission.take().publish()` sends `RoomJoinAdmission::Admitted` to every queued peer. The lease is consumed exactly once: either published (admission granted) or dropped (admission aborted). Sixteen manual abort sites collapse to one `Drop` impl.

**4. `room_handshake_snapshot_for_peer(peer, expected_room_hash)` filters by membership + hash.**

The old `room_handshake_snapshot` returned channels for any caller. The new signature requires the peer to be in `state.peers` AND `state.room_hash()` to equal `expected_room_hash`, so a stale-generation caller cannot pick up the wrong room's sender.

## Why This Works

The registry is a small, synchronous (`StdMutex<Option<PendingRoomAdmission>>`) data structure that publishes the *intent* to admit a member set before the asynchronous work that realizes it. Three properties make it race-free:

1. **No yield between slot acquire and admission install.** The lease exists before any task can observe the slot as `RoomCall`. If the join is cancelled mid-flight, the lease's `Drop` releases pending joins; the slot's `release_if_match(room_owner)` runs in the cleanup path against the same owning snapshot.
2. **`try_acquire_with_snapshot` is atomic under one mutex acquisition.** The snapshot and the acquire share a single critical section, so the registry's owner reference cannot drift to a different acquirer between operations.
3. **Authorization consults both registry and `room_state`.** The cold-join window is closed for inbound dials; once `RoomState` publishes, the registry entry is redundant but harmless (`allows` short-circuits on hash equality with the now-active room).

The `Drop`-based abort guard converts a class of "remember to clean up at every exit" bugs into a structural guarantee. Adding a new early-return to `join_room_with_operation` cannot leak queued joins because the guard's destructor runs unconditionally.

## Prevention

- **Never separate slot acquisition from snapshot capture.** Use `try_acquire_with_snapshot` (or the matching `try_acquire_or_match_with_owner`) whenever downstream state is keyed to the owning generation. Treating them as separate operations resurrects the race.
- **Install intent before the first await.** Any state that other tasks will consult (registry entries, generation counters, pending flags) must be in place before the calling task yields. If the operation is cancelled, the intent's `Drop`/rollback path is responsible for cleanup.
- **Prefer `Drop` guards over manual cleanup at exit sites.** When a function accumulates more than two early-return paths that all need the same cleanup, the cleanup belongs in a guard type. New exits then cannot forget.
- **For any "is X authorized right now" check that spans an async window, consult both the in-flight intent and the realized state.** Checking only the realized state re-opens the cold-window race; checking only the intent leaks authorization after cancellation.
- **Filter active-room lookups by `(peer, hash)`, not by `peer` alone.** A peer can be a member of multiple successive rooms; the room hash disambiguates which generation the caller means.

### Test coverage

- `cold_room_join_admits_member_before_room_state_publication` (room_lifecycle.rs) — gates peer B's `setup_call` via `InputSampleRateGate`, then has peer A dial in while B is parked. Asserts A's dial is admitted and B's `current_room_generation()` is still `None` (proving admission precedes publication). Releases the gate and asserts both reach `RoomCall` with no spurious leaves.
- `reciprocal_room_joins_use_one_canonical_session_without_churn` (room_lifecycle.rs) — both peers issue `join_room` concurrently and end up with exactly one canonical session per peer, no churn.
- `room_session_collision_handoff_keeps_membership_until_replacement_is_admitted` (room_lifecycle.rs) — covers the `Drop`-guard path: a cancelled replacement does not leak queued joins.

## Related Issues

- [Room Dial Reconciliation](../room-dial-reconciliation-2026-07-28.md) — the admission registry is one of three coordination mechanisms (alongside `RoomDialScheduler` and `room_reconcile`) that closes room-race classes on this branch.
- [Deferred Room Predecessor Teardown](../deferred-room-predecessor-teardown-2026-07-28.md) — the registry's owner snapshot is what `release_if_match` uses to release the slot against the exact acquirer.
- Branch commit `9204217` ("resolves race conditions in rooms") introduced the registry; commit `7f77964` ("resolves review feedback") added the `PendingRoomJoinGuard` `Drop` guard and removed the 16 manual abort sites.
