---
title: Prepared Identity Switching Requires Runtime Readiness and Token-Owned Commit
date: 2026-07-24
category: docs/solutions/architecture-patterns/
module: telepathy-core identity switching and Flutter Rust Bridge
problem_type: architecture_pattern
component: authentication
severity: high
applies_when:
  - A frontend persists profile state before committing a backend identity change
  - A state transition needs exclusive call-slot ownership across an asynchronous boundary
  - A caller must not report success until a replacement runtime is active
related_components:
  - session-manager
  - flutter-rust-bridge
  - profile-controller
tags: [identity-switching, runtime-readiness, prepared-token, flutter-rust-bridge, async-commit]
---

# Prepared Identity Switching Requires Runtime Readiness and Token-Owned Commit

## Context

Profile switching crosses a persistence boundary in Flutter and an asynchronous runtime boundary in Rust. A receiver-owned commit API can detach the commit from the prepared operation that reserved the call slot, and a scheduling acknowledgement does not prove that the replacement runtime is usable.

## Guidance

Represent a prepared identity switch as an opaque, consuming capability token. It owns the validated identity and contact snapshot, origin `CoreState`, session gate, and exact call-slot lease. The token is the only object allowed to commit its prepared operation.

```rust
pub async fn commit(self) -> Result<()> {
    let revision = self
        .core_state
        .replace_desired_runtime_infallible(self.identity, self.contacts);
    self.core_state.wait_for_runtime_applied(revision).await
}
```

Expose `PreparedIdentitySwitch.commit()` through Flutter Rust Bridge rather than a `Telepathy.commitIdentitySwitch(prepared)` receiver method. The bridge wrapper consumes its stored inner token once, so a prepared operation cannot be redirected to another backend or committed twice.

Runtime readiness must be revision-specific. A waiter returns success only when its own revision becomes applied. It returns a terminal error when a newer revision supersedes it, setup fails for it, or the manager stops.

Flutter prepares the token, persists the target profile ID, updates its in-memory active profile, and awaits `prepared.commit()`. A persistence failure disposes the token before commit, releasing its owned lease without replacing the desired Rust runtime.

## Why This Matters

Capability ownership matches lifecycle ownership. Preparation validates data and reserves the session gate and call-slot lease once; committing that same token releases resources through normal Rust ownership rather than reconstructing a transaction from ambient receiver state.

Revision-specific readiness prevents stale requests from succeeding and prevents callers from waiting forever after supersession, setup failure, or shutdown. A successful profile switch means the manager has installed the requested identity and processed the requested contact snapshot into manager startup, not merely accepted a restart request.

## When to Apply

- A backend preparation step reserves exclusive resources across an FFI boundary.
- A frontend must persist durable selection before backend activation.
- An asynchronous manager can replace, reject, or stop work after a caller requests it.

## Examples

`CoreState::wait_for_runtime_applied` checks whether its revision is still desired, is applied, failed setup, or has been stopped before waiting for another notification. See `rust/telepathy-core/src/internal/state.rs`.

`ProfilesController` keeps its pending flag set through `await prepared.commit()`, so competing profile actions remain blocked until a successful runtime application or terminal failure. See `lib/controllers/profiles_controller.dart`.

Integration coverage exercises setup failure, supersession, shutdown, and token readiness in `rust/telepathy-core/tests/core_integration_test/runtime_readiness.rs`.

## Related

- No related solution documents existed when this learning was captured.
