---
title: Room Task Bounded-Join Timeouts Are Peer-Local, Not Terminal
date: 2026-07-28
category: docs/solutions/runtime-errors/
module: telepathy-core room controller task teardown
problem_type: runtime_error
component: service_object
severity: medium
symptoms:
  - A single peer's slow output-shutdown tears down the entire room with a terminal error
  - "`peer_output_timed_out_on_room_event` warnings escalate to user-visible room failure notifications"
  - One participant on a slow connection (e.g., a flaky relay) repeatedly ends the room for everyone
root_cause: logic_error
resolution_type: code_fix
related_components:
  - room-controller
tags: [room-races, task-timeout, peer-local, terminal-classification, room-task-join, blast-radius]
---

# Room Task Bounded-Join Timeouts Are Peer-Local, Not Terminal

## Problem

`join_room_task_bounded` enforces a per-task shutdown deadline (`ROOM_TASK_JOIN_TIMEOUT`) to keep room teardown bounded. When a room task (peer output, peer input, statistics) exceeded the deadline, the verdict was `RoomTaskOutcome::Terminal(error)`, which the room controller treated as evidence of corrupted shared state — it tore down the entire room, surfaced a terminal error to the user, and notified every participant.

That classification was too aggressive for the most common cause of the timeout: a slow peer-output shutdown. A peer whose connection was already closed locally (the owning connection is removed by the caller before join is invoked) and whose task is aborted before this verdict is returned has no path to corrupt shared state. Its only effect is on its own peer stream.

## Symptoms

- `peer_output_timed_out_on_room_event` log warning, immediately followed by `room_terminal_error`.
- A single participant on a slow network path (high-latency relay, congested link) ends the room for everyone, even though the other peer connections are healthy.
- User-visible room failure notifications reference a timeout rather than an actual panic or join error.

## What Didn't Work

- **Increasing `ROOM_TASK_JOIN_TIMEOUT`.** Reduced the false-positive rate but did not change the classification. A slow peer could still eventually exceed the larger bound and tear down the room; the bound also lost its purpose (bounding teardown).
- **Special-casing per task kind.** Required maintaining a matrix of which task kinds on which events qualified as terminal. The classification question is not about task kind, it is about what the timeout actually implies.

## Solution

In `rust/telepathy-core/src/internal/helpers.rs`, reclassify the bounded-join timeout verdict.

Before:

```rust
Err(error) => {
    abort_room_task(handle);
    warn!(event = %format!("{task_kind}_timed_out_on_{event_kind}"), ?error);
    RoomTaskOutcome::Terminal(error.into())
}
```

After:

```rust
Err(error) => {
    abort_room_task(handle);
    warn!(event = %format!("{task_kind}_timed_out_on_{event_kind}"), ?error);
    RoomTaskOutcome::PeerLocal
}
```

The enum docstring is updated to reflect the actual semantics:

```rust
/// `PeerLocal` covers expected connection closure: the task completed cleanly,
/// returned an `Err` from a peer-side condition (e.g. socket close), or was
/// aborted after exceeding `ROOM_TASK_JOIN_TIMEOUT`. The owning connection is
/// already removed by the caller and the task itself is aborted before this
/// verdict is returned, so a slow peer-output shutdown stays scoped to that
/// peer and never tears down the rest of the room.
///
/// `Terminal` covers genuinely unexpected failures: a panic or `JoinError`
/// observed while awaiting the handle. These can indicate corrupted shared
/// state and propagate as terminal room errors with user-visible notification.
```

`abort_room_task(handle)` runs before the verdict is returned, so the offending task is gone by the time the controller sees `PeerLocal`. The caller has already removed the owning connection, so there is no path for the timed-out task to affect anyone but its own peer.

## Why This Works

The blast radius of a bounded-join timeout is one peer. The caller invokes `join_room_task_bounded` only after removing the owning connection from the room's connection map. The task being awaited is the task being aborted; the timeout means "this task did not exit on its own within the bound," not "this task corrupted shared state." Treating it as peer-local matches what is actually happening.

Genuine corruption-class events — panics and `JoinError`s observed on the handle — remain `Terminal`. These signal that the task ended abnormally, which can indicate a poisoned mutex, a violated invariant, or memory unsafety. Escalating those to a room-wide terminal error is still correct.

The classification now answers the right question: *what can this failure affect?* A timeout can affect one peer; a panic can affect the whole room.

## Prevention

- **Match failure verdicts to blast radius, not to severity.** A timeout is not necessarily severe; it is necessarily scoped. Classifying by scope keeps the room resilient to peer-local failures.
- **Document the precondition that justifies a verdict.** The `PeerLocal` docstring names the load-bearing facts: "the owning connection is already removed by the caller and the task itself is aborted before this verdict is returned." If those preconditions change, the verdict must be revisited.
- **Reserve `Terminal` for genuinely unknown-state failures.** A panic or a `JoinError` means the task's state at exit is unknown. A timeout means the task was still running and is now aborted; its state at exit is "did not complete," which is well-defined.
- **Re-check classification whenever the calling context changes.** If a future refactor moves the connection-removal step to *after* the join, the `PeerLocal` classification is no longer safe: the connection would still be live when the verdict is returned, and a slow output task could still be writing to shared state.

### Test coverage

The change is exercised by every existing room-lifecycle test that closes a peer connection and triggers the bounded-join path. Stress runs (10 iterations, `cargo nextest run ... --stress-count 10`) under the `room_lifecycle` module confirm the reclassification does not regress teardown behavior. The dedicated regression for this branch (`concurrent_room_end_immediately_rejoins_without_direct_negotiation`) exercises three-peer concurrent teardown and would fail under the old classification if any single peer's output task ran slow.

## Related Issues

- [Pending Room Admission Registry](./pending-room-admission-registry-2026-07-28.md) — the room controller that consumes these verdicts is the same controller that owns the admission registry.
- The branch's overall race-resolution pattern (defer, snapshot, generation-stamp) applies here in miniature: the verdict is meaningful only because the precondition (connection removed, task aborted) is established before the verdict is returned.
