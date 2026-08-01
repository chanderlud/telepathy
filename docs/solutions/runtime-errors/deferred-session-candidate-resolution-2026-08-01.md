---
title: Deferred Session Candidate Resolution Preserves Pending Calls
date: 2026-08-01
category: docs/solutions/runtime-errors/
module: telepathy-core session and call lifecycle
problem_type: runtime_error
component: service_object
severity: high
symptoms:
  - A pending outgoing call can end without one terminal CallEnded callback after repeated session collisions
  - Accept-prompt cancellation can become visible before its pending call slot is released
root_cause: async_timing
resolution_type: code_fix
related_components:
  - session-collision-registry
  - call-slot
  - accept-prompt
tags: [session-collision, pending-call, terminal-resolution, cancellation-ordering, concurrency]
---

# Deferred Session Candidate Resolution Preserves Pending Calls

## Problem

A session collision can defer a replacement while the predecessor still owns a pending outgoing call. The predecessor must preserve that call when a replacement promotes, but must release it and emit one terminal `CallEnded` when every deferred replacement aborts.

## Root Cause

The pending-candidate registry initially attached the predecessor's terminal obligation to one candidate resolution. When candidate A aborted, `try_install` removed it and created candidate B with a fresh `terminal_pending = false`. If B also aborted, predecessor cleanup released the slot through generic cleanup but no longer knew it owed the frontend a terminal call state.

Carrying the flag alone left another race. Cleanup could snapshot A, wait for A to complete, and resume after B replaced it. An ID check prevented cleanup from deleting B, but cleanup still read A's stale terminal flag and could end the pending call while B remained viable.

Prompt cancellation had a separate ordering race: notifying the platform callback before releasing `PendingIncoming` allowed observers to start another call or room operation while the old slot was still occupied.

## Solution

When `try_install` replaces an aborted candidate for the same predecessor, it carries `terminal_pending` into the new resolution. A candidate for a different predecessor starts clean. Promotion still marks the active resolution as promoted before completion, removes it from the registry, and preserves the pending call.

After each wait, predecessor cleanup atomically resolves the completed ID against the registry's current entry. A matching final abort is removed and may consume the inherited terminal obligation. A different candidate for the same predecessor is returned as the successor, so cleanup drops session-map locks and waits for that resolution instead. A viable successor can therefore promote without a stale predecessor resolution releasing its call.

Only the registry operation that matches and removes the current final aborted candidate can authorize terminalization. Cleanup then releases the matching pending slot, drops session-map locks, and invokes `CallEnded`.

Accept-prompt interruption and session-stop paths now release pending ownership before notifying prompt cancellation. Callback awaits remain outside session and call-slot locks.

## Why This Works

Terminal intent belongs to the predecessor call, not to one replacement connection. Carrying it only across candidates with the same predecessor keeps that intent alive through A abort, B install, and B abort without leaking it to a newer session generation. Promotion remains non-terminal because the promoted flag is recorded before candidate completion.

The current-entry comparison and terminal decision happen under one registry lock. Cleanup never awaits while holding that lock or the session map lock. If B supersedes completed A, A cannot authorize terminalization; cleanup follows B until B promotes or becomes the final abort.

Moving prompt notification after slot release makes cancellation observation a post-condition: once the callback sees cancellation, the slot is already available.

## Regression Coverage

- `terminal_resolution_survives_sequential_aborted_candidates` deterministically covers candidate A abort, candidate B installation, and candidate B abort.
- `stale_aborted_resolution_waits_for_viable_successor` covers cleanup observing completed A after viable B installs, then verifies B promotion remains non-terminal.
- `promoted_candidate_does_not_inherit_abort_outcome` protects successful collision promotion.
- `aborted_deferred_collision_terminalizes_original_call_once` verifies one terminal `CallEnded` and an idle caller slot through the integration stack.
- `reset_sessions_clears_pending_incoming_slot` observes reset-driven prompt cancellation only after the pending slot is idle.
- `accept_prompt_cancellation_is_visible_only_after_pending_slot_release` covers peer-message interruption ordering.

Verification completed with 7 focused regressions, all 23 `call_lifecycle` tests, all 95 core integration tests, and all 10 full integration stress iterations passing.

## Prevention

- Store deferred outcomes at the lifecycle scope that owns them, then transfer them across replaceable attempts in that scope.
- Consume deferred terminal outcomes only while atomically matching the current attempt; an ID-safe delete alone does not make a stale decision safe.
- Preserve generation or predecessor identity when carrying state across candidate replacement.
- Signal cancellation only after releasing resources that observers may immediately reacquire.
- Synchronize race tests with completed callbacks and lock barriers, not sleeps, retries, or scheduler yields.
