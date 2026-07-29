---
title: Race-Free Test Synchronization Probes Replace Sleep-Based Polling
date: 2026-07-28
category: docs/solutions/conventions/
module: telepathy-core integration test harness
problem_type: convention
component: testing_framework
severity: medium
applies_when:
  - An integration test needs to assert that a callback fired N times or reached a specific state before continuing
  - A test currently uses `tokio::time::sleep` to wait for an asynchronous side effect to materialize
  - A test would race on internal task scheduling without a deterministic precondition
  - A negative assertion (callback did NOT fire) needs a bounded upper bound rather than an unbounded sleep
related_components:
  - session-manager
  - room-controller
tags: [test-infrastructure, race-free, probes, deterministic-tests, mock-callbacks, integration-tests]
---

# Race-Free Test Synchronization Probes Replace Sleep-Based Polling

## Context

The room-race fix work added substantial integration coverage under `rust/telepathy-core/tests/core_integration_test/room_lifecycle.rs`. The first cut of these tests used `tokio::time::sleep` to wait for the session manager to dial, for contact lookups to occur, and for session statuses to propagate. Three problems surfaced immediately:

1. **Flakes on slow CI.** A sleep short enough to keep the suite fast (e.g., 250ms) would occasionally fire before the session manager's dial task was scheduled. The test would assert state that had not yet converged.
2. **Slowness on local runs.** A sleep long enough to be reliable (e.g., 2s) made the suite noticeably slow and made stress runs (`--stress-count 10`) painful.
3. **No deterministic negative assertions.** Asserting "no ghost direct call was issued" required waiting "long enough" — but "long enough" is exactly the wrong frame for a race condition, where the bug fires nondeterministically.

A code-review finding on the branch (P2: "Sleep-synchronized race test") made this explicit: the test in question would pass under the bug and fail only sometimes under the fix. The fix's correctness needed to be observable, not statistical.

## Guidance

**Inject cloneable probe objects into mock callbacks. Each probe records the events the test cares about and exposes a bounded `wait_for(...)` API. Tests await specific conditions, never arbitrary durations.**

Three probes were added in `rust/telepathy-core/tests/core_integration_test/common.rs` and threaded through `construct_mock_callbacks_with_contact_lookup`:

### `ContactLookupProbe` — counts `get_contact` invocations per peer

```rust
#[derive(Clone, Default)]
pub(super) struct ContactLookupProbe {
    counts: Arc<Mutex<HashMap<Vec<u8>, usize>>>,
    changed: Arc<Notify>,
}

impl ContactLookupProbe {
    fn record(&self, peer_id: &[u8]) {
        *self.counts.lock().unwrap().entry(peer_id.to_vec()).or_default() += 1;
        self.changed.notify_waiters();
    }

    pub(super) async fn wait_for(&self, peer_id: &[u8], expected: usize) {
        // loop on `Notify::notified()` until count >= expected,
        // bounded by a 60s timeout that panics with observed vs expected
    }
}
```

Injected into the `get_contact` mock callback. A test that needs peer A to have been looked up twice (e.g., once during contact-list iteration, once during room handshake) calls `probe.wait_for(&peer_a, 2).await` and proceeds the moment the second lookup happens.

### `SessionStatusProbe` — records the latest `SessionStatus` per peer

```rust
#[derive(Clone, Default)]
pub(super) struct SessionStatusProbe {
    statuses: Arc<Mutex<HashMap<Vec<u8>, SessionStatus>>>,
    changed: Arc<Notify>,
}

impl SessionStatusProbe {
    pub(super) async fn wait_for(&self, peer_id: &[u8], expected: SessionStatus) {
        // loop on `Notify::notified()` until discriminant(status) == discriminant(expected),
        // bounded by a 60s timeout
    }
}
```

Injected into the `session_status` mock callback. `wait_for` compares `mem::discriminant` rather than full equality, so the test asserts the right status *kind* (e.g., `SessionStatus::Connected`) without coupling to relay/latency fields that vary by environment.

### `PendingAcceptProbe` — counts direct-call prompts

Already existed on the branch but was applied more systematically. A test that needs to assert *no* ghost direct call uses `accept_probe.opened.load(Relaxed)` after a bounded negative-poll window (2.5s), rather than an unbounded sleep.

### Negative assertions: bounded negative poll

```rust
wait_for_no_extra_room_leave(&call_states_a, &peer_b_str, 0, Duration::from_secs(1)).await;
```

A 1-second bounded poll after the positive assertions confirm that the expected state has stabilized. This is the disciplined version of "sleep to make sure nothing else happens": bounded duration, named helper, explicit assertion at the end.

## Why This Matters

| Approach                          | Failure mode under load                                            |
|-----------------------------------|--------------------------------------------------------------------|
| `sleep(250ms)` then assert        | Sporadic flakes when scheduling slips;CI noise; erodes trust        |
| `sleep(2s)` then assert           | Slow suite; stress runs become painful; flake rate goes down but does not reach zero |
| Probe with `wait_for(condition)`  | Deterministic: test proceeds the instant the condition holds; bounded timeout catches real bugs |
| Probe with bounded negative poll  | Negative assertions become first-class: "no event for 1s after positive convergence" is a meaningful predicate |

A probe-then-`wait_for` test fails for one of two reasons only: the implementation is broken (the event never fires and the probe times out), or the test's expectation is wrong (the event fires the wrong number of times and `wait_for` panics with observed vs expected). Neither is a flake.

The `Notify`-loop pattern in `wait_for` is subtle: the future from `Notify::notified()` must be pinned and `enable()`d before re-checking the condition, otherwise a notification that fires between the condition check and the `await` is lost. The `wait_for` implementations in `common.rs` handle this correctly; copy them rather than re-deriving.

## When to Apply

- An integration test checks that an async callback fired N times, reached a specific state, or did NOT fire within a bounded window.
- A test currently uses `tokio::time::sleep` to wait for an asynchronous side effect to materialize.
- A test would race on internal task scheduling without a deterministic precondition (e.g., asserting on session map state immediately after `start_session` returns).
- A stress run (`--stress-count N`) is flaky in a way that traces back to "the event had not happened yet" rather than to a real correctness violation.

This is *not* required for tests that assert immediate, synchronous postconditions (e.g., a unit test of a pure function). It is required for any test that observes the session manager, the room controller, the call slot lifecycle, or any other system whose effects materialize asynchronously.

## Examples

**Before (race-prone):**

```rust
client_a.telepathy.join_room(room_members).await.unwrap();
tokio::time::sleep(Duration::from_millis(500)).await;  // wait for dials
assert_eq!(call_states_a.lock().unwrap().len(), 2);
```

The 500ms is a guess. Under load, the dials may not have completed; the assertion fails. Or worse: under the bug being tested, the dials happen to complete within 500ms anyway, and the test passes against the broken code.

**After (deterministic):**

```rust
client_a.telepathy.join_room(room_members).await.unwrap();
contact_lookup_a.wait_for(&peer_b.to_vec(), 2).await;  // exactly two lookups
assert!(core_a.current_room_generation().await.is_none(),
        "B must admit A before publishing RoomState");
```

The test proceeds the instant the second lookup happens. If the implementation is broken and the second lookup never fires, the probe times out at 60s with a panic that names the expected count and the observed count. The assertion that follows (`current_room_generation().is_none()`) is now a *precondition check*, not a race-sensitive observation.

**Negative assertion (no ghost direct call):**

```rust
let (rejoin_a, rejoin_b, rejoin_c) = tokio::join!(/* three concurrent rejoins */);
// ... positive assertions on room joins ...

assert_eq!(accept_probe_a.opened.load(Relaxed), 0,
           "client a must not receive a ghost direct-call prompt");
assert_eq!(accept_probe_b.opened.load(Relaxed), 0, /* ... */);
assert_eq!(accept_probe_c.opened.load(Relaxed), 0, /* ... */);

wait_for_no_extra_room_leave(&call_states_a, &peer_b_str, 0, Duration::from_secs(1)).await;
```

The `accept_probe.opened == 0` check is taken *after* positive convergence on the rejoin, then a 1-second bounded poll confirms no late fires. A stale `start_call` notify would trip the probe or the bounded poll, deterministically.

## Related

- The probes were added in commit `9204217` (initial room-race coverage) and refined in commit `7f77964` (review-feedback pass: P2 finding "Sleep-synchronized race test" became a `SessionStatusProbe` precondition + 2.5s bounded negative poll).
- All four bug-track docs on this branch rely on these probes for their regression tests:
  - [Pending Room Admission Registry](../runtime-errors/pending-room-admission-registry-2026-07-28.md) — uses `ContactLookupProbe` to verify the cold-join window.
  - [Room Dial Reconciliation](../runtime-errors/room-dial-reconciliation-2026-07-28.md) — uses `PendingAcceptProbe` for the ghost-direct-call negative assertion.
  - [Deferred Room Predecessor Teardown](../runtime-errors/deferred-room-predecessor-teardown-2026-07-28.md) — uses `wait_for_stable_session_pair` (built on the same probe pattern).
- The `Notify`-loop `enable()` discipline is documented inline in `wait_for` implementations; copy those rather than re-deriving.
