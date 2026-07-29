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

## Session Establishment

### Direct Session Attempt
A peer-scoped request to establish a direct session, identified so that completion and terminal outcomes apply only to the attempt that initiated them.

An attempt remains active while direct dialing or an associated inbound candidate can still publish a usable session; terminal outcomes and runtime teardown end it.

### Session Availability
The observable per-peer state that tells a call request whether a direct session already exists, a direct session attempt may still publish one, or no session can be expected.

Availability changes wake waiting call requests; a request only acquires call ownership after the session is published.
