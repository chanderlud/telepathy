# Concepts

Shared domain vocabulary for this project — entities, named processes, and status concepts with project-specific meaning. Seeded with core domain vocabulary, then accretes as ce-compound and ce-compound-refresh process learnings; direct edits are fine. Glossary only, not a spec or catch-all.

## Identity Switching

### Prepared Identity Switch
A one-use capability representing a validated target identity and contact snapshot whose commit is bound to the origin CoreState that prepared it.

It holds the exclusive session and call-slot resources required during preparation; dropping it abandons the prepared operation without releasing a later owner's resources.

### Desired Runtime
The identity and contact configuration that the session manager is expected to apply next.

### Runtime Revision
A wrapping identifier advanced for each desired runtime replacement, used to distinguish applied, superseded, and failed readiness requests.

### Runtime Readiness
The state in which the session manager has applied the requested runtime revision, making identity-dependent activity safe to start.

## Video Sessions

### Video Session
A peer-scoped exchange that coordinates one display-media source, its lifecycle controls, and its media transport without owning the underlying call.

### Video Attempt
One incarnation of a Video Session, scoped so late asynchronous work from an earlier incarnation cannot affect a later use of the same peer slot.

### Video Slot
The per-peer lifecycle boundary that admits at most one Video Attempt and remains occupied until that attempt is fully finished.

### Joined Teardown
The terminal process that keeps a Video Slot unavailable until all work for its current Video Attempt has finished.

## Relationships

A Video Slot owns one Video Attempt at a time. Joined Teardown preserves that ownership until the attempt has fully finished, after which the slot may admit a replacement attempt.
