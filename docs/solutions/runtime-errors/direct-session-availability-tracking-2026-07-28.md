---
title: Direct Session Availability Tracking Closes the start_call Wait Window
date: 2026-07-28
category: docs/solutions/runtime-errors/
module: telepathy-core session manager and call lifecycle
problem_type: runtime_error
component: service_object
severity: high
symptoms:
  - "`start_call` can observe no session before a requested direct session publishes"
  - "A waiting `start_call` needs to wake when shutdown or a terminal direct-dial outcome makes a session impossible"
  - "A coalesced, self, or closed-channel direct dial must not leave a call request waiting"
root_cause: async_timing
resolution_type: code_fix
related_components:
  - session-manager
  - call-slot
  - room-dial-scheduler
tags: [session-availability, direct-dial, start-call, cancellation, generation-stamp]
---

# Direct Session Availability Tracking Closes the `start_call` Wait Window

## Problem

`start_call_with_operation` needs a published session, but `start_session` only queues the direct dial. A call request between queueing and session installation previously had no per-peer state that could distinguish a viable direct attempt from an absent or terminal one.

## Symptoms

- A caller can request a direct session and immediately start a call before `session_states` contains that peer.
- Cancellation and shutdown must release a waiting call request without acquiring a call slot.
- Self-dials, room-dial coalescing, and a closed manager channel must make the pending direct attempt terminal.

## What Didn't Work

- Sending only a `PublicKey` through `start_session` did not identify the direct attempt that a completion or terminal outcome belonged to.
- Inspecting only `session_states` could report no session while a direct attempt was still legitimately in flight.
- Clearing outbound state during reset did not provide a dedicated signal for call requests waiting on direct-session availability.

## Solution

`SessionAvailability` tracks an active direct attempt for each peer, its monotonically advanced attempt ID, and a generation for observable state changes. `try_start_session` creates or joins the peer's direct attempt and sends `(peer, attempt_id)` to the manager.

The manager ignores stale attempt IDs and terminalizes the current attempt when it rejects a self-dial, detects an existing session, coalesces with an in-flight room dial, or receives direct-dial completion. Inbound candidates receive an `IncomingCandidateLease`; the attempt remains available until direct completion and all candidate leases have been released.

`start_call_with_operation` now loops before call-slot acquisition: it proceeds when a session exists, waits on the availability generation while a direct attempt is active, returns `NoSessionForContact` when no attempt is active, and returns cleanly when its operation is cancelled. `reset_sessions` terminalizes all availability entries and wakes waiters during teardown.

## Why This Works

Attempt IDs make completion peer-scoped and prevent late manager events from terminalizing a newer attempt. The availability generation gives a waiter a snapshot-based change predicate rather than a polling loop. Candidate leases keep a still-initializing inbound session from being treated as impossible after the outbound dial completes.

The call slot is acquired only after `session_states` contains the peer, so cancellation or terminalization while waiting cannot claim a direct-call slot.

## Prevention

- Route direct-dial lifecycle transitions through `SessionAvailability`; do not infer availability from `session_states` alone.
- Carry the attempt ID across every direct-dial completion path and ignore stale IDs.
- Terminalize availability during reset or shutdown so waiters always observe a change.
- Cover availability waits through the public `start_session(&Contact)` path, including cancellation, shutdown, and room-dial coalescing.

## Test Coverage

- `start_call_waits_for_trusted_session_attempt_and_cancellation_leaves_no_call` verifies a waiting call can be cancelled without claiming the call slot.
- `shutdown_wakes_waiting_start_call_without_claiming_call_slot` verifies reset wakes the waiter.
- `start_call_without_trusted_attempt_returns_no_session_promptly`, `closed_session_sender_terminalizes_its_attempt`, and `self_session_request_terminalizes_without_stranding_start_call` cover terminal outcomes.
- `public_start_session_then_start_call_waits_for_publication_and_connects` verifies the public direct-session path reaches a call.
- `direct_start_terminalizes_while_room_dial_is_in_flight` verifies room-dial coalescing does not leave direct availability pending.

## Related Issues

- [Room Dial Reconciliation Closes the start_call and Redial Race Classes](./room-dial-reconciliation-2026-07-28.md) covers room-level dial coalescing and stale reconciliation permits.
- [Non-Blocking Manager Start with Bounded Downstream Readiness](../architecture-patterns/non-blocking-manager-start-bounded-readiness-2026-07-26.md) covers runtime readiness before this direct-session availability wait.
- [Race-Free Test Synchronization Probes Replace Sleep-Based Polling](../conventions/race-free-test-synchronization-probes-2026-07-28.md) describes the deterministic status probe used by these lifecycle tests.
