---
title: Joined Video Session Teardown Keeps Slots Safe for Reuse
date: 2026-07-25
category: docs/solutions/architecture-patterns/
module: telepathy-core video sessions
problem_type: architecture_pattern
component: tooling
severity: high
applies_when:
  - A peer-scoped session owns transport workers or platform resources
  - Multiple asynchronous paths can terminate the same session
  - A slot may be reused after cancellation
related_components:
  - session-manager
  - flutter-rust-bridge
  - testing-framework
tags: [video-sessions, teardown, cancellation, task-ownership, iroh]
---

# Joined Video Session Teardown Keeps Slots Safe for Reuse

## Context

A video session can finish through local stop, remote control, a timeout, transport failure, or session teardown. Cancellation alone does not release the transport worker or its platform I/O. Reusing the peer slot before that worker joins lets stale work overlap a new session.

## Guidance

Make the per-peer `VideoSlot` the single owner of each `VideoAttempt` and its worker. A local start records a fresh session identity and generation in a reservation before it sends an offer. The worker installs only while that exact attempt is still `Starting`; a stale installation cancels and joins itself instead of attaching to a replacement reservation.

Terminal paths must claim the reservation by moving it to `Stopping`, cancel its token, take and join the worker, then clear the reservation and notify idle. A second terminal path waits for idle instead of joining or clearing the same worker again. `VideoSlot::cancel_and_join` implements this ordering in `rust/telepathy-core/src/internal/video.rs`.

Session teardown must invoke `cancel_current_and_join` after signalling call and session cancellation. `SessionState::teardown` does this before returning in `rust/telepathy-core/src/internal/state.rs`.

## Why This Matters

The corrected teardown regression showed that cancelling only the session token leaves an installed video worker blocked on its own token. The session could appear torn down while the worker still held transport or platform resources.

The reservation remains occupied until the worker join completes, so a new generation cannot reuse the slot early. The full attempt identity also prevents a late worker result from mutating or terminating a replacement session.

## When to Apply

- A logical slot owns sockets, streams, subprocesses, device handles, or long-lived tasks.
- More than one event can end that work.
- A stale task could act after its slot has been reused.

## Examples

The installation guard requires matching attempt identity, `Starting` phase, and no installed worker before storing the handle. Otherwise it cancels the launch and awaits the worker.

```rust
if reservation.attempt == launch.attempt
    && reservation.phase == VideoPhase::Starting
    && reservation.worker.is_none()
{
    reservation.phase = VideoPhase::Active;
    reservation.worker = Some(worker);
} else {
    launch.cancellation.cancel();
    let _ = worker.await;
}
```

The sender and receiver race stream creation against cancellation and reset or stop interrupted streams in `rust/telepathy-core/src/internal/video/transport.rs`. Integration coverage verifies that teardown does not make the slot idle before a blocked worker is released and joined in `rust/telepathy-core/tests/core_integration_test/video_sessions/lifecycle.rs`.

## Related

- [Prepared Identity Switching Requires Runtime Readiness and Token-Owned Commit](prepared-identity-switch-runtime-readiness.md) applies the same ownership principle to a prepared identity operation.
- The generic video-session implementation plan is `docs/plans/2026-07-17-001-refactor-generic-video-sessions-plan.md`.
