---
title: Race Tests Must Poll the Authoritative State, Not Its Observable Proxy
date: 2026-08-05
category: docs/solutions/conventions/
module: telepathy-core integration test harness
problem_type: convention
component: testing_framework
severity: medium
applies_when:
  - A test waits for a session replacement or candidate promotion using a SessionStatus event
  - A test asserts a future is Pending immediately after spawning an operation whose failure is fast
  - A test asserts an exact event sequence across a teardown boundary
related_components:
  - session-manager
  - room-controller
tags: [test-infrastructure, flaky-tests, race-free, integration-tests, assertions]
---

# Race Tests Must Poll the Authoritative State, Not Its Observable Proxy

## Context

Three integration tests flaked only under full-suite stress (roughly 1 in 10
iterations each). None of the production code was wrong; the assertions gated
on proxies that are not causally ordered with the state under test.

## Guidance

**Assert on the authoritative state (session map id, call-slot snapshot), not
on an event that merely correlates with it.**

- `active_room_retains_same_identity_candidate_until_predecessor_finishes` and
  `stale_predecessor_promotes_same_identity_replacement_and_allows_call` waited
  for a new connection-level `SessionStatus::Connected` and then asserted the
  session-map id changed. The status fires when the replacement's dial
  *connects*; a deferred candidate's promotion (map install) can lag it by an
  unbounded wait. Fix: poll `session_states` for the id change directly, with a
  deadline.
- `start_call_waits_for_trusted_session_attempt_and_cancellation_leaves_no_call`
  polled the `start_call` future immediately after `start_session` returned and
  asserted Pending. The unreachable peer's dial can exhaust its retries and
  abandon within ~3ms, before the first poll runs — then `start_call` returns
  promptly and the assertion flakes. The already-present second poll (gated on
  the `Connecting` emission) covers the same property deterministically; the
  racy first poll was deleted.
- `concurrent_room_end_immediately_rejoins_without_direct_negotiation` asserted
  an exact `[Join, Join]` room-event sequence. When a peer's teardown goodbye
  reaches the controller before the local `end_call` does, a `RoomLeave` is
  legitimately delivered between the joins. The assertion now locks exactly two
  joins with no Leave after the final one (`assert_room_rejoin_sequence` in
  `common.rs`), tolerating a sandwiched Leave while keeping the post-rejoin
  window strict.

A fourth, subtler barrier: when a test must observe a race path that produces
no callback, capture logs. `init_test_tracing` tees formatted log lines into a
global buffer; `wait_for_log_line(&[event, peer_id], ...)` asserts the exact
event fired for the exact peer. A missing event fails loudly instead of letting
the test pass vacuously.

## Why This Matters

Proxy-gated assertions pass in isolation and fail under load, because load is
exactly what reorders proxy and state. An assertion on the authoritative state
fails only when the system is actually broken. Likewise, an exact event-sequence
assertion that ignores a legitimate interleaving manufactures flakes from
correct behavior.

## When to Apply

- Any assertion that follows a `SessionStatus`/`Connected`-style event and
  checks session identity, promotion, or slot ownership.
- Any "must still be pending" poll that can be overtaken by a fast failure of
  the thing being waited on — gate the poll on the in-flight signal instead.
- Any exact event-sequence assertion spanning a teardown: enumerate the
  legitimate interleavings first, then lock only what is actually forbidden.

## Examples

Before (proxy-gated, flaky under stress):

```rust
probe.wait_for_connected_after(peer, baseline).await;
assert_ne!(session_states.read().await.get(&peer).map(|s| s.id()), Some(old_id));
```

After (authoritative-state poll):

```rust
tokio::time::timeout(Duration::from_secs(60), async {
    loop {
        if session_states.read().await.get(&peer).map(|s| s.id()) != Some(old_id) {
            return;
        }
        sleep(Duration::from_millis(25)).await;
    }
}).await.expect("candidate should promote");
```

## Related

- [Race-Free Test Synchronization Probes Replace Sleep-Based Polling](race-free-test-synchronization-probes-2026-07-28.md) — the probe/`wait_for` foundation this doc extends; the two should be read together.
