---
title: Terminal Call States Must Follow Slot Release
date: 2026-08-05
category: docs/solutions/runtime-errors/
module: telepathy-core call slot lifecycle
problem_type: runtime_error
component: service_object
severity: medium
symptoms:
  - "join_room immediately after observing CallEnded fails with CallAlreadyActive"
  - "System test call_timeout_then_room_join intermittently fails on the concurrent join step"
root_cause: async_timing
resolution_type: code_fix
related_components:
  - call-slot
  - room-controller
tags: [call-slot, release-before-notify, callback-ordering, concurrency]
---

# Terminal Call States Must Follow Slot Release

## Problem

Several outgoing-negotiation terminal branches emitted `CallEnded` to the
frontend *before* releasing the pending call slot. A frontend that reacts to
`CallEnded` by starting another operation (for example `join_room`) can win the
race against the release and fail with `CallAlreadyActive`.

## Symptoms

- System test `call_timeout_then_room_join` intermittently failed with
  `A call is already active` on the concurrent `join_room` step.

## What Didn't Work

- Nothing to un-do here: the branches were written callback-first and the race
  window (callback delivery vs. the release a few microseconds later) is simply
  wide enough to hit in system tests.

## Solution

In `negotiate_outgoing_call`, all three terminal branches — the hello-timeout
branch, the `HelloResponse::EndedWith` branch, and the `setup_call` error
branch — now call `release_pending` first and emit `CallState::CallEnded`
after:

```rust
release_pending(&self.session_states, peer, io.state.id, &mut pending_slot).await?;
self.callbacks
    .call_state(CallState::CallEnded(message.into_string(), true))
    .await;
```

This extends the branch's existing release-before-notify rule (prompt
cancellation is visible only after its pending slot is released) to the
caller's terminal states.

## Why This Works

Observing `CallEnded` is a post-condition: once the frontend sees it, the slot
must already be available for whatever the user does next. Delivering the
callback first makes the observation a lie for a few microseconds — exactly
long enough for an automated caller (or a fast user) to lose.

## Prevention

- Rule of thumb for this codebase: any frontend-visible terminal or
  cancellation event is emitted only after the resources it frees are actually
  released.
- `accept_prompt_cancellation_is_visible_only_after_pending_slot_release` and
  `reset_sessions_clears_pending_incoming_slot` cover the prompt side; the
  system scenario `call_timeout_then_room_join` covers the caller side.

## Related Issues

- [Stale Accepted Negotiation Must Abort Without a Goodbye](stale-accepted-negotiation-abort-goodbye-livelock-2026-08-05.md)
