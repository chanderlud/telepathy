---
title: Accept Prompts Leak When Their Caller Vanishes Across a Session Transfer
date: 2026-08-05
category: docs/solutions/runtime-errors/
module: telepathy-core session and call lifecycle
problem_type: runtime_error
component: service_object
severity: high
symptoms:
  - "A parked accept prompt stays open forever when the caller hangs up before the replacement session re-drives the Hello"
  - "An adopted (transferred) prompt waits on user input forever when the caller's process dies after adoption"
  - "The retained pending-incoming generation holds the call slot, so join_room fails with CallAlreadyActive"
root_cause: async_timing
resolution_type: code_fix
related_components:
  - call-slot
  - accept-prompt
  - session-collision-registry
tags: [accept-prompt, prompt-transfer, leak, expiry, session-collision, concurrency]
---

# Accept Prompts Leak When Their Caller Vanishes Across a Session Transfer

## Problem

The collision accept-prompt transfer parks a displaced prompt for adoption by
the replacement session. Two windows leaked: a caller that hangs up (or crashes)
before its Hello is re-driven never triggers adoption, so the prompt and its
retained pending generation were held forever; and an adopted prompt whose
caller died afterwards waited on user input indefinitely, because incoming
offers had no expiry at all.

## Symptoms

- Callee's incoming-call prompt stays open after the caller ended the call
  during a session-collision swap.
- `join_room` fails with `CallAlreadyActive` because the retained
  pending-incoming generation is never released.
- System test `call_prompt_survives_session_restart`: no `accept_call_canceled`
  after the caller's process restarted.

## What Didn't Work

- **Signaling from the caller.** The caller's terminal paths (end_call,
  candidate abort) emit `CallEnded` locally but send no goodbye on the
  *replacement* session; the callee's idle session loop treats a bare goodbye
  as an unexpected message and never consults the transfer registry, so nothing
  reaches the parked prompt.
- **Relying on adoption.** Adoption only happens when a re-driven Hello starts
  an incoming negotiation on the replacement session; a dead caller never
  re-drives.

## Solution

Two expiries, both anchored to the caller's own offer window (`HELLO_TIMEOUT`,
10s — after which no re-driven Hello can legitimately arrive):

1. **Parked-transfer reaper** (`PendingAcceptTransferRegistry::park`): each
   park spawns a task that, after `PARKED_ACCEPT_TRANSFER_TIMEOUT`, removes the
   entry only if its id still matches (a newer park or an adoption supersedes
   it), cancels the prompt, and releases the exact retained generation via
   `release_if_match`.

2. **Offer expiry in the incoming negotiation's accept select**
   (`negotiate_incoming_call`): a `sleep(HELLO_TIMEOUT)` branch releases the
   pending slot and lets the prompt guard drop (cancel, or park — which the
   reaper then covers). An open offer can never outlive the caller's own
   negotiation timeout.

Both are covered by deterministic integration tests
(`parked_accept_prompt_expires_when_caller_hangs_up_before_adoption`,
`reset_sessions_cancels_parked_accept_prompt`,
`unanswered_accept_prompt_expires_with_caller_offer_window`); the first fails
without the reaper (the prompt is never cancelled).

## Why This Works

The offer's legitimacy is bounded by the caller's timeout, not by the callee's
session topology. Anchoring both expiries to `HELLO_TIMEOUT` means a prompt can
only ever outlive a *live* offer by zero seconds: any prompt still open after
that point represents an offer the caller has already given up on, so expiring
it is always safe. The reaper's id check keeps a newer park or a completed
adoption from being cancelled by a stale reaper.

## Prevention

- Any state parked across a session replacement needs an expiry or an explicit
  transfer failure path; "the replacement will pick it up" is not a guarantee.
- When releasing retained call-slot generations, always use exact-generation
  release (`release_if_match`) so a re-acquired slot is never clobbered.
- Registry unit tests cover the park-replaces-park and `take_valid`
  invalidation branches (`park_replacing_previous_cancels_previous_prompt`,
  `take_valid_*` in `core.rs`).

## Related Issues

- [Deferred Session Candidate Resolution Preserves Pending Calls](deferred-session-candidate-resolution-2026-08-01.md)
- [Stale Accepted Negotiation Must Abort Without a Goodbye](stale-accepted-negotiation-abort-goodbye-livelock-2026-08-05.md)
