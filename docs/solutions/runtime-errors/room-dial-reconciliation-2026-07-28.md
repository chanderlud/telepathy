---
title: Room Dial Reconciliation Closes the start_call and Redial Race Classes
date: 2026-07-28
category: docs/solutions/runtime-errors/
module: telepathy-core session manager and room dial scheduler
problem_type: runtime_error
component: service_object
severity: high
symptoms:
  - "A `start_session` call queued for a room peer spawns a redundant direct dial alongside the in-flight room dial"
  - "Stale `start_call` or `reconcile_room_call` permit fires after teardown or during `room_handshake` exit, latching a ghost direct-call prompt or deadlocking both sessions in a `hello_ack_timeout` loop"
  - Disappearing/reappearing peer triggers unbounded 2-second redial loops
  - Retained sessions re-negotiate the room handshake every second under `Hello`/`Reject` churn
  - "\"Group contact\" dials observed after `end_call()` completes"
root_cause: async_timing
resolution_type: code_fix
related_components:
  - session-manager
  - room-controller
  - call-slot
tags: [room-races, dial-scheduler, reconcile, backoff, generation-stamp, notify-drain, start-call]
---

# Room Dial Reconciliation Closes the `start_call` and Redial Race Classes

## Problem

`session_manager` had no unified view of in-flight dials. Three race classes fell out:

1. **Redundant dial launch.** `start_session` for a peer already being dialed by `join_room_with_operation`'s member loop spawned a second `open_session` task. Both completed; the loser tore down via collision rules; the user saw churn.
2. **Stale `start_call` notify.** Room teardown cancelled the room task and dropped the room-state read guard, but the per-`SessionState` `start_call` `Notify` could already have a permit latched from the room's member iteration. The next `session_outer` loop consumed that permit after teardown and dispatched a ghost direct call.
3. **Unbounded redial + churn.** A disappearing/reappearing canonical peer retried the room dial on a fixed 2-second interval forever, and retained sessions re-negotiated the room handshake at 1Hz under `Hello`/`Reject` churn.
4. **Stale-permit deadlock after concurrent end-and-rejoin.** A reconcile permit latched on a stale dialer session *during* `room_handshake` was consumed *after* the handshake exited, re-launching `negotiate_outgoing_call` against a peer that had already admitted a different session. The peer treated the late `Hello` as `room_handshake_unexpected_message` and never sent `HelloAck`, deadlocking both sessions in a 10-second `hello_ack_timeout` loop until the 60-second test timeout fired.

## Symptoms

- `ignored_redundant_outgoing` log lines, then `session_collision_kept_new` churn.
- `accept_call` prompts reaching the frontend after the user already pressed end-call, with no in-flight `start_call` to attribute them to.
- A peer going offline and back online every 2 seconds in a tight loop.
- Sustained 1Hz `room.handshake` log spam while a room is otherwise idle.
- `room_handshake_unexpected_message other=Hello {...}` followed by `hello_ack_timeout` every 10 seconds until the 60-second timeout fires; both sessions log `simultaneous_dial_detected_yielding` for the prior generation.

## What Didn't Work

- **Coalescing on `session_states` alone.** Checking `session_states.read().await.get(&peer_id).is_none()` before dialing did not catch the case where a room dial was in flight but the session had not yet been inserted. A new `direct_dials` in-memory set was needed to track the dial task itself.
- **Cancelling `start_call` notifies on teardown.** `Notify` does not support draining pending permits without a reader. The fix had to make the notify *generation-stamped* so the consumer could discard stale permits at receive time.
- **Fixed-interval redial.** A peer that flapped faster than the redial interval never converged. Exponential backoff with a retry bound was needed both for the dial task and for the per-session re-arm notification.
- **Generation-stamping the produce side only.** The first cut of the generation-stamped `notify_room_reconcile`/`take_room_reconcile_generation` pair closed the stale-generation case but missed the *current*-generation permit latched during `room_handshake`. The `is_current_room_dial` guard in the select branch did not help: the stale session genuinely was not admitted to the new generation yet, and the peer genuinely was in the new dial set, so the guard passed. The produce-side fix needed a matching consume-side drain at the end of `room_handshake` (mirroring the existing `start_call` drain four lines above it). Without both halves, `concurrent_room_end_immediately_rejoins_without_direct_negotiation` timed out at 60s; with both halves, it passed in 1.8s.

## Solution

All changes in `rust/telepathy-core/src/internal/core.rs` and `internal/state.rs`.

**1. In-memory in-flight dial tracking.**

```rust
let mut direct_dials = HashSet::<PublicKey>::new();
let mut room_dials = RoomDialScheduler::default();
```

`direct_dials.insert(peer)` returns `false` if a dial is already running; the manager loop coalesces instead of spawning a second task. Room dials go through `RoomDialScheduler`, which tracks per-peer attempts, generations, and backoff.

**2. Single reconciliation entry point, called from three triggers.**

```rust
_ = self.room_reconcile.notified() => { self.reconcile_room_dials(...).await; }
_ = room_reconcile_timer.tick() => { self.reconcile_room_dials(...).await; }
Some(event) = dial_event_receiver.recv() => {
    match event {
        ManagerDialEvent::Room(event) => room_dials.complete(event, Instant::now()),
        ManagerDialEvent::DirectCompleted(peer) => { direct_dials.remove(&peer); }
    }
    self.reconcile_room_dials(...).await;
}
```

`reconcile_room_dials` reads `room_state`, computes the desired member set (peers where `local < peer`, sorted), filters by admission state, and launches the resulting dials. It is the *only* place that decides what to dial.

**3. `request_room_reconcile` Notify fires from every state change.**

```rust
self.inner.request_room_reconcile();
```

Called from: session install (`initialize_session`), session cleanup (`session_outer` exit), room teardown (`cleanup_room_controller`), and `remove_session`. This is the reactive trigger; the 1Hz timer is the safety net.

**4. Generation-stamped `reconcile_room_call` Notify.**

```rust
pub(crate) fn notify_room_reconcile(&self, generation: u64) {
    self.reconcile_room_generation.store(generation, Release);
    self.reconcile_room_call.notify_one();
}

pub(crate) fn take_room_reconcile_generation(&self) -> Option<u64> {
    match self.reconcile_room_generation.swap(0, Acquire) {
        0 => None,
        generation => Some(generation),
    }
}
```

The consumer in `session_outer` calls `take_room_reconcile_generation()` before re-arming; if the generation is stale (older than the session's room admission) or zero (already consumed), the permit is discarded. This closes the stale-`start_call` race without draining the `Notify` itself.

**5. Permit drain at the end of `room_handshake` (consume-side fix).**

The generation-stamp on the produce side (component 4) is necessary but not sufficient. A reconcile permit latched *during* `room_handshake` — while the session is busy running the long-running handshake branch — is still current-generation from the consumer's view. When `room_handshake` exits and the session re-enters its select loop, `session_inner` consumes the stale permit and re-launches `negotiate_outgoing_call` against a peer that has already moved on to a different session for the same generation.

The fix mirrors the existing `start_call` drain four lines above it:

```rust
// Discard any `reconcile_room_call` permit latched on this session
// during the handshake. Without this drain, `session_inner` re-launches
// `negotiate_outgoing_call` against a peer that has already moved on,
// deadlocking both sessions in a hello_ack_timeout loop. Clearing the
// generation only on an actual drain avoids dropping a fresh reconcile
// that races between the two calls.
if timeout(Duration::ZERO, session.reconcile_room_call.notified())
    .await
    .is_ok()
{
    debug!(
        event = "room_handshake_discarded_stale_reconcile",
        peer.id = %peer_id
    );
    session.take_room_reconcile_generation();
}
session.leave_room(room_generation);
self.request_room_reconcile();
```

`timeout(Duration::ZERO, notified())` is the load-bearing idiom: it drains a pending permit non-blockingly (the `Notified` future resolves immediately when a permit is latched) and is a no-op otherwise. The generation clear is conditional on an actual drain to avoid dropping a fresh reconcile that races between the two calls. (Source: `rust/telepathy-core/src/internal/core.rs:2314-2329`.)

**6. Teardown idempotency: cancel room intent before any await.**

`cleanup_room_controller` now calls `end_sessions.cancel()` as its very first action (helpers.rs:728), before any I/O teardown await. This prevents the room's per-peer task group from issuing stale `request_room_reconcile()` notifies — and from consuming queued `RoomMessage`s — during the teardown window. Combined with component 3 (`request_room_reconcile` fires from teardown), this converts teardown from a race-prone sequence (cancel-after-I/O) into an idempotent one (cancel-then-teardown).

**7. Bounded backoff on both dial and re-arm.**

```rust
const ROOM_DIAL_BACKOFF_BASE_MS: u64 = 100;
const ROOM_DIAL_BACKOFF_MAX_MS: u64 = 30_000;
const ROOM_DIAL_MAX_RETRIES: u32 = 10; // ~80s of active retrying before exhaustion
```

`RoomDialScheduler::rearm()` applies the same backoff curve to per-session re-arm notifications: first re-arm per room generation fires immediately, subsequent ones throttle 100ms → 30s. The 1Hz churn drops to at most one negotiation per 30s steady-state, while mesh healing (the departing peer's own rejoin) is unaffected.

## Why This Works

The reconciler is event-driven (Notify + dial-event channel) with a periodic safety net (1Hz timer). State changes trigger reconciliation; reconciliation reads the desired state from `room_state` + `session_states` + the pending admission registry and computes the diff. This is the standard "desired vs. actual" controller pattern, applied at the session-manager level.

The stale-permit race has **two** load-bearing pieces, both required:

1. **Produce-side generation-stamp** (component 4). Plain `Notify::notify_one()` latches one permit; if no consumer is waiting, the next `notified()` call returns immediately. Without a generation check, a permit latched during teardown would dispatch a ghost direct call on the next session. The generation counter converts the notify into a "this specific generation wants reconciliation" signal, and the consumer-side swap-then-check converts consume into a CAS. This closes the *stale-generation* case.

2. **Consume-side drain at long-branch exit** (component 5). The generation-stamp does *not* close the *current-generation-permit-latched-during-handshake* case. A `Notify` permit consumed after a long-running branch (`room_handshake`) exits will re-launch the branch's caller (`negotiate_outgoing_call`) even though the peer has already moved on within the same generation. The drain at branch exit — `timeout(Duration::ZERO, notified())` — is the only way to drop a latched permit without a reader being currently parked on `notified()`. This mirrors the existing `start_call` drain pattern and is a general requirement for any Notify-driven select loop with a long-running branch.

The `Acquire`/`Release` ordering on the generation counter is mandatory: `Release` on the producer side ensures the `notify_one()` is not reordered before the store; `Acquire` on the consumer side ensures the load is not reordered before the `Notify::notified()` future is polled. Relaxed ordering on both sides would permit the notify to fire before the generation is visible, defeating the guard.

## Prevention

- **For any cross-task signaling that must be conditional ("reconcile only if X is still true"), use a version-stamped Notify, not a bare Notify.** The version (generation, revision, epoch) is the consumer's discard predicate.
- **Track in-flight dial tasks in memory, not just realized sessions.** Two dials for the same peer can both complete before either inserts a session; the resulting collision is a symptom, not a fix.
- **Use one reconciler for one resource type.** Multiple ad-hoc "should I dial now?" checks in different code paths will drift apart. Routing every trigger through one function makes the policy explicit.
- **Bound retries.** A peer that flaps forever should not produce load forever. `ROOM_DIAL_MAX_RETRIES = 10` with a 30s cap gives ~80 seconds of active retrying before the dial is exhausted, after which human intervention is required.
- **Apply the same backoff curve to re-arm notifications as to dial launches.** Otherwise the re-arm path becomes a churn amplifier even when the dial path is bounded.
- **For any Notify-driven select loop with a long-running branch, drain the Notify at branch exit.** `timeout(Duration::ZERO, notified())` drops a pending permit non-blockingly. Without this drain, a permit latched during the branch is consumed after exit, re-launching the branch against stale state. The generation-stamp on the produce side does not help when the permit's generation is still current.
- **Cancel room task groups before any await in teardown.** `end_sessions.cancel()` runs as the first line of `cleanup_room_controller`, before I/O teardown. This prevents the room tasks from issuing stale reconciles or consuming queued messages during the teardown window.

### Test coverage

**Race / integration (room_lifecycle.rs):**
- `concurrent_room_end_immediately_rejoins_without_direct_negotiation` — three peers tear down and rejoin concurrently; asserts no ghost direct prompts (`accept_probe.opened == 0`), no `CallEnded` states, exactly two Join events per peer pair.
- `room_reconciles_a_missing_session_without_another_join` — a missing session is reconciled into existence without re-issuing `join_room`.
- `room_reconcile_discards_stale_generation_after_teardown` — directly targets the generation-stamped Notify: a stale notify after teardown must not latch a direct call.
- `room_retries_when_the_canonical_peer_starts_late` — the backoff curve admits a peer whose manager starts later than the local room join.
- `room_goodbye_rearms_a_persistent_session_for_peer_rejoin` — `Goodbye` on a retained session triggers re-arm, not full session replacement.
- `six_peer_room_builds_the_canonical_mesh` — six-peer mesh converges to one canonical session per peer pair.
- `reciprocal_room_joins_use_one_canonical_session_without_churn` — both sides `join_room` concurrently without churn.

**Unit (core.rs `#[cfg(test)] mod tests`):**
- `stops_redialing_a_peer_after_the_retry_bound` — `RoomDialScheduler` halts after `ROOM_DIAL_MAX_RETRIES` attempts.
- `throttles_session_rearms_with_backoff_per_room_generation` — `rearm()` gate throttles per-generation re-arms at 100ms → 30s and resets on generation change.

**System test (system-tests/scenarios/call_timeout_then_room_join.yaml):** extended to assert the dialer's `accept_call_canceled` event precedes the subsequent room join, covering the timeout-then-rejoin path end-to-end.

## Related Issues

- [Pending Room Admission Registry](./pending-room-admission-registry-2026-07-28.md) — the registry's `allows(peer, owner)` check is what `is_admitted_room_peer` uses to reconcile room peers during the cold-join window.
- [Deferred Room Predecessor Teardown](./deferred-room-predecessor-teardown-2026-07-28.md) — session replacement deferral prevents the reconciler from tearing down the predecessor of an in-flight replacement.
- The desired-vs-actual controller shape mirrors the existing `desired_runtime`/`wait_for_runtime_applied` pattern documented in [Non-Blocking Manager Start with Bounded Downstream Readiness](../architecture-patterns/non-blocking-manager-start-bounded-readiness-2026-07-26.md).
- Commit `9204217` introduced the scheduler and reconcile loop; commit `7f77964` added the generation-stamped Notify and backoff caps in response to P1/P2 review findings; commit `0a2aad5` added the consume-side `reconcile_room_call` drain at the end of `room_handshake` after a 60-second deadlock surfaced under stress (the generation-stamp alone was insufficient — see component 5).
