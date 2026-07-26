---
title: Non-Blocking Manager Start with Bounded Downstream Readiness
date: 2026-07-26
category: docs/solutions/architecture-patterns/
module: telepathy-core session manager and Flutter/CLI bindings
problem_type: architecture_pattern
component: service_object
severity: high
applies_when:
  - A blocking manager-start API delays UI render or first-response latency
  - A non-blocking refactor must preserve the readiness contract for callers that immediately issue dependent operations
  - An unbounded readiness wait is being relocated from a synchronous API path into a downstream operation
related_components:
  - session-manager
  - flutter-rust-bridge
  - telepathy-cli
tags: [non-blocking-start, runtime-readiness, bounded-wait, session-manager, async-gate, flutter-rust-bridge]
---

# Non-Blocking Manager Start with Bounded Downstream Readiness

## Context

`Telepathy::start_manager` blocked the calling thread until the session manager reached `ManagerState::Active`. On a fresh launch the manager performs iroh endpoint setup, relay negotiation, and pkarr publication — routinely 2–5 seconds. The Flutter UI (`runApp`) was scheduled *after* `await telepathy.startManager()` in `lib/main.dart`, so the window appeared blank for those seconds. The same blocking path also gated every CLI `start_manager` ack.

A prior commit (`c9320ab`, "await manager readiness before Flutter startup") had introduced the blocking semantic to fix a separate race; reverting it naively (returning immediately from `start_manager`) re-opened that race and broke `test_session_client_disappears_and_reappears`, because `start_session` issued immediately afterwards errored with `RuntimeNotReady` — the manager had not yet reached Active.

The challenge: keep `start_manager` non-blocking for UI responsiveness while ensuring dependent operations (`start_session`, `start_call`, `join_room`, `audio_test`) still succeed when issued immediately after.

## Guidance

**Move the readiness wait out of `start_manager` and into the runtime-dependent operations, bounded by a precondition + timeout.**

Three pieces, in this order:

1. **Non-blocking `start_manager`** — spawn the manager task and return immediately. The eventual `Active` transition is observed via the existing `managerActive` callback. The Flutter `StateController._sessionManagerState` field already powers the UI status pill, so the UI renders in `Starting` and promotes itself to `Active` asynchronously.

   ```rust
   // flutter.rs / native.rs
   pub async fn start_manager(&mut self) {
       self.handle.start_manager().await;
   }
   ```

   The `()` return type is a compile-time guard: reintroducing blocking semantics requires changing the signature, which is visible in review and triggers FRB regeneration.

2. **Persist desired state before waiting.** The contact list must land in `desired_runtime` regardless of when the manager spins up. The manager picks up `desired_runtime.contacts` when it iterates; a fire-and-forget `start_session` cannot rely on the manager having already captured the snapshot.

3. **Single bounded readiness helper used by every runtime-dependent operation.** Encapsulates precondition (no manager started → fast-fail), bounded timeout (panic/stall damage control), and cancellation-token-aware wait (so a cancelled `start_call` / `join_room` doesn't block on a stalled manager):

   ```rust
   const START_SESSION_RUNTIME_TIMEOUT: Duration = Duration::from_secs(30);

   async fn await_runtime_applied(&self, operation: &CancellationToken) -> Result<()> {
       // Fast-fail when no manager task exists. wait_for_runtime_applied has
       // no exit condition without a running manager.
       if self.inner.start_session.is_none() {
           return Err(ErrorKind::RuntimeNotReady.into());
       }
       let revision = self.inner.core_state.desired_runtime()?.revision;
       let wait = async {
           tokio::select! {
               biased;
               _ = operation.cancelled() => Ok(()),
               result = self.inner.core_state.wait_for_runtime_applied(revision) => result,
           }
       };
       match timeout(START_SESSION_RUNTIME_TIMEOUT, wait).await {
           Ok(result) => result,
           Err(_) => Err(ErrorKind::RuntimeNotReady.into()),
       }
   }
   ```

   All four runtime-dependent operations route through this helper:

   ```rust
   // try_start_session
   self.inner.core_state.add_desired_contact_infallible(contact.clone());
   self.await_runtime_applied(&CancellationToken::new()).await?;
   self.dial_session(contact).await;

   // start_call_with_operation / join_room_with_operation (cancellation-aware)
   self.await_runtime_applied(operation).await?;

   // audio_test
   self.await_runtime_applied(&CancellationToken::new()).await?;
   ```

   This unifies what was previously an asymmetric contract: `start_session` waited, `start_call` / `join_room` / `audio_test` fast-failed. After this refactor, all four share the same bounded, cancellation-aware behavior, so an agent or UI issuing any of them immediately after `start_manager` succeeds.

## Why This Matters

The naive non-blocking refactor moved `wait_for_runtime_applied` (unbounded, no timeout, no panic observer) onto the public `start_session` path while holding the `identity_session_gate` critical section. A code review surfaced **four interlocking hang modes**, all sharing that root cause:

- **No manager started** → `wait_for_runtime_applied` loops forever (revision 0, applied/failed = u64::MAX, `stop_manager` never cancelled). Old `ensure_runtime_applied` returned `RuntimeNotReady` immediately.
- **Manager task panic** → `spawn_task` has no `catch_unwind`; a panic in `session_manager` ends the task silently without `mark_runtime_setup_failed`. The gate is held forever.
- **Slow/stalled setup** → DNS, relay, pkarr can take tens of seconds. The gate is held for the duration.
- **Holding `identity_session_gate` across the unbounded wait** → serializes all `start_session` and identity-switch calls behind whichever wait lands first.

The bounded helper closes all four: the precondition restores the fast-fail for "no manager started," the timeout caps panic/stall damage at 30s, and the cancellation token lets a cancelled caller exit early. Critically, the helper is the *single* place that owns this contract — every runtime-dependent operation benefits from the fix instead of each call site re-implementing it.

The reconciliation preserves the identity-switch contract. `wait_for_runtime_applied` still returns `RuntimeSuperseded` when the desired revision changes (e.g., `set_identity` bumps revision), so `try_start_session`'s existing error path (`start_session_identity_switch_blocked` warning) fires unchanged.

## When to Apply

- A blocking initialization API gates UI render or first-response latency.
- A non-blocking refactor must preserve the readiness contract for callers that issue dependent operations immediately.
- An unbounded readiness wait (`Notify`-loop with no timeout) is being relocated from a synchronous API path into a downstream operation that holds a critical section.
- A code review surfaces "asymmetric readiness contract" findings — some operations wait, others fast-fail, for the same precondition.

## Examples

**Before (blocking UI render):**

```rust
// flutter.rs
pub async fn start_manager(&mut self) -> Result<(), DartError> {
    self.handle.start_manager_and_wait().await.map_err(DartError::from)
}
```

```dart
// main.dart
await telepathy.startManager();   // blocks 2-5s on iroh setup
for (Contact contact in profilesController.contacts.values) {
  telepathy.startSession(contact: contact);  // fire-and-forget
}
runApp(...);  // scheduled after the await — window appears late
```

**After (non-blocking with bounded downstream wait):**

```rust
// flutter.rs — returns immediately, Active observed via callback
pub async fn start_manager(&mut self) {
    self.handle.start_manager().await;
}
```

```rust
// internal.rs — all runtime-dependent ops share the bounded helper
async fn await_runtime_applied(&self, operation: &CancellationToken) -> Result<()> {
    if self.inner.start_session.is_none() {
        return Err(ErrorKind::RuntimeNotReady.into());
    }
    let revision = self.inner.core_state.desired_runtime()?.revision;
    let wait = async {
        tokio::select! {
            biased;
            _ = operation.cancelled() => Ok(()),
            result = self.inner.core_state.wait_for_runtime_applied(revision) => result,
        }
    };
    match timeout(START_SESSION_RUNTIME_TIMEOUT, wait).await {
        Ok(result) => result,
        Err(_) => Err(ErrorKind::RuntimeNotReady.into()),
    }
}
```

**Test coverage:** `try_start_session_waits_for_runtime_then_dials` in `rust/telepathy-core/tests/core_integration_test/runtime_readiness.rs` uses a `ManagerStartingGate` to hold the manager in setup, asserts `try_start_session` is Pending while the gate holds (proves it waits), then releases the gate and asserts completion within 5s (proves it proceeds once the runtime is applied). The CLI system test `test_start_manager_ack_precedes_active_event` in `system-tests/tests/test_scenarios.py` asserts the ack returns in under 1s and the Active event arrives strictly after.

## Related

- [Prepared Identity Switching Requires Runtime Readiness and Token-Owned Commit](./prepared-identity-switch-runtime-readiness.md) — companion pattern covering the revision-specific `wait_for_runtime_applied` contract from the identity-switch angle. This doc relies on that contract for the `RuntimeSuperseded` exit path.
- Commit `c9320ab` ("await manager readiness before Flutter startup") introduced the blocking semantic that this pattern replaces.
- Commit `aecd9b8` ("wait for manager runtime before CLI acknowledgment") was the prior CLI-side fix on the same path.
