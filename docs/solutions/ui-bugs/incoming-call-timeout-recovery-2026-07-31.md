---
title: Incoming Call Timeout Recovery Prevents Dialog Teardown Races
date: 2026-07-31
category: docs/solutions/ui-bugs/
module: Flutter incoming-call prompt handling
problem_type: ui_bug
component: frontend_stimulus
severity: medium
symptoms:
  - "Backend cancellation could dismiss whichever navigator route was current instead of the incoming-call prompt."
  - "A prompt timeout could later pop the home route after cancellation had already removed the prompt."
  - "A cancellation could dismiss a newer dialog stacked above the incoming-call prompt."
root_cause: async_timing
resolution_type: code_fix
related_components:
  - lib/core/utils/dialog_utils.dart
  - lib/main.dart
  - test/core/utils/dialog_utils_test.dart
tags: [flutter, incoming-call, dialog-route, cancellation, timeout]
---

# Incoming Call Timeout Recovery Prevents Dialog Teardown Races

## Problem

The incoming-call prompt and backend cancellation raced to dismiss UI routes. The pre-fix caller raced the prompt against backend cancellation, then called `Navigator.pop` on the navigator's current route when cancellation won; the prompt's timeout independently used a context-relative `pop`.

## Symptoms

- Cancellation could remove a newer dialog rather than the incoming-call prompt.
- The prompt's timeout could run after cancellation and remove the original home route.
- A cancelled incoming call did not have one owner for prompt dismissal and timer cleanup.

## What Didn't Work

- Racing `acceptCallPrompt` with `cancel.notified()` in `main.dart` left caller-side cancellation to pop the current navigator route.
- Calling `Navigator.of(context).pop(false)` from the timeout did not identify the route that owned the prompt.
- Cancelling the timeout only from button handlers did not cover backend cancellation.

## Solution

`acceptCallPrompt` now receives the backend cancellation future and owns every terminal path. It creates and pushes a `DialogRoute<bool>`, retains that route, and uses `navigator.removeRoute(promptRoute, result)` for accept, deny, timeout, and cancellation.

`closePrompt` is idempotent through `dialogOpen`, cancels the timer before removing the route, and the `finally` block cancels any remaining timer. `main.dart` awaits this single prompt future, then cancels the ringtone and promotes the pending call only after an accepted result.

## Why This Works

Removing the retained `DialogRoute` targets the incoming-call prompt even when it is no longer the navigator's top route. A single prompt-owned close path prevents both cancellation and a delayed timeout from popping an unrelated route.

## Prevention

- Give asynchronous dialogs one owner for timeout, cancellation, and user-decision cleanup.
- Remove a retained route when correctness depends on dismissing a specific dialog; do not pop the navigator's current route.
- Keep regression tests for cancellation followed by timeout and for cancellation beneath a newer dialog.

## Test Coverage

- `backend cancellation cannot let an incoming-call timeout pop the original scaffold` verifies the home route remains after cancellation and the delayed timeout.
- `backend cancellation removes the incoming prompt beneath a newer dialog` verifies cancellation removes only the prompt and preserves the newer dialog.

## Related Issues

- [PR #60](https://github.com/chanderlud/telepathy/pull/60) contains this unmerged fix for [issue #57](https://github.com/chanderlud/telepathy/issues/57).
- [Direct Session Availability Tracking Closes the `start_call` Wait Window](../runtime-errors/direct-session-availability-tracking-2026-07-28.md) documents adjacent backend cancellation handling; it does not own Flutter dialog routes.
- [Room Dial Reconciliation Closes the `start_call` and Redial Race Classes](../runtime-errors/room-dial-reconciliation-2026-07-28.md) covers separate Rust stale-notification races.
