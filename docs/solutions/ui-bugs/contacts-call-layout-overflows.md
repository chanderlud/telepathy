---
title: Contacts and Call Layout Overflows from Flag-Driven Responsive Decisions
date: 2026-08-03
category: docs/solutions/ui-bugs/
module: Flutter home screen layout (contacts list, call controls, call details)
problem_type: ui_bug
component: frontend_stimulus
severity: medium
symptoms:
  - "Contacts header row overflowed at narrow card widths when the Session Manager label, status icon, and restart button exceeded the available width."
  - "The call controls' bottom button bar was pushed out of view on short windows."
  - "Call details card contents overflowed below the card on short layouts."
  - "Wide compact layouts showed 2 contact rows stretched to fill a viewport tall enough for 3."
  - "At 620-640px canvas widths the narrow home branch applied the tall non-compact top-section cap, crushing the tab view below."
root_cause: logic_error
resolution_type: code_fix
related_components:
  - lib/widgets/contacts/contacts_list.dart
  - lib/widgets/contacts/contact_widget.dart
  - lib/widgets/contacts/room_widget.dart
  - lib/widgets/call/call_controls.dart
  - lib/widgets/call/call_details_widget.dart
  - lib/screens/home/home_page.dart
  - lib/core/constants/app_constants.dart
  - lib/core/utils/layout_context.dart
  - test/widgets/home/support/layout_harness.dart
tags: [flutter, layout-overflow, layoutbuilder, mediaquery, responsive-layout, compact-layout, contacts-list, issue-58]
---

# Contacts and Call Layout Overflows from Flag-Driven Responsive Decisions

## Problem

GitHub issue #58 reported rendering overflows across the home screen's contacts and call widgets at small window sizes. Every case traced to one root pattern: layout decisions were driven by global flags (`MediaQuery`-derived compact getters, an `isCompact` boolean) instead of the constraints each widget actually received, and rows/columns had no designated slack absorber, so tight space became a hard overflow instead of a controlled shrink.

## Symptoms

- The contacts header row overflowed when the "Session Manager" label, status icon, and (in the failed state) restart button exceeded the card width.
- The call controls' bottom button bar was pushed off-screen on short windows because the slider column plus a `Spacer` exceeded the available height.
- The call details card overflowed below its container on short layouts because the loss chart, level meters, and stats row are fixed-size children.
- Wide compact layouts showed 2 contact rows stretched to fill a viewport tall enough for 3, because the row count came from the compact flag rather than the measured viewport height.
- At canvas widths of 620-640px the home page's narrow branch applied the tall non-compact top-section cap, crushing the tab view: the branch was chosen from a padded `LayoutBuilder` width while `isCompactContacts` read the unpadded `MediaQuery` width, and the two disagree in that range.

## What Didn't Work

- **Flooring the contact item height** — an intermediate commit floored each row's height and was later reverted on the same branch. A floor sizes rows without addressing why the viewport was divided by the wrong count, and the flush-right button fix made it redundant. Driving the count from the measured height is the direct fix.
- **Reusing `isCompactContacts` for the narrow-branch cap selection** — `isCompactContacts` is gated on width (`isCompactControls && !isWideLayout`, `lib/core/utils/layout_context.dart:13`), so it re-introduces exactly the width disagreement between the padded branch condition and the unpadded `MediaQuery` that caused the 620-640px crush.

## Solution

Six coordinated fixes, landed on branch `fix/issue-58-ui-layout` (merged via PR #66):

1. **Constraint-driven row count** (`lib/widgets/contacts/contacts_list.dart:23`, `:235`). The list viewport always fits a whole number of rows, chosen from the measured height — 3 when the viewport clears `minThreeItemListHeight` (160px), otherwise 2:

   ```dart
   final itemHeight = constraints.maxHeight /
       (constraints.maxHeight >= minThreeItemListHeight ? 3 : 2);
   ```

2. **Height-only compact decision** (`lib/screens/home/home_page.dart:114`). The narrow branch now reads `context.isCompactControls` (height-only, `lib/core/utils/layout_context.dart:8`) instead of the width-gated `isCompactContacts`, so the cap selection can no longer disagree with the width-based branch condition.

3. **Sized compact-wide cap** (`lib/core/constants/app_constants.dart:14`). `topSectionMaxHeightWideCompact` was raised from 170 to 225 so it fits the compact call-details layout (title + input/output levels + stats row, without the loss chart).

4. **Slack-absorber contact/room rows** (`lib/widgets/contacts/contact_widget.dart:228`, `:263`; `lib/widgets/contacts/room_widget.dart:91`). The bare `Spacer` was replaced by a tight `Expanded` nickname slot — the nickname is now the row's designated slack absorber, left-aligned at natural width and ellipsizing only when narrow. The connected-status group sits in its own tight, end-aligned `Expanded` slot so it hugs the call button instead of letting it float mid-row; the address ellipsizes inside. Trailing buttons stay flush right in all states.

5. **Scrollable slider region** (`lib/widgets/call/call_controls.dart:69`). The audio sliders live in an `Expanded` + `SingleChildScrollView`, replacing the old `Spacer`, so the bottom control bar is never pushed out of view on short layouts; when everything fits, the scroll view simply fills the space the spacer absorbed.

6. **Dispensable content gating** (`lib/widgets/call/call_details_widget.dart:19`). The loss chart — the only dispensable element — is dropped when the measured height cannot fit it plus the levels and stats row:

   ```dart
   final bool showChart = constraints.maxHeight >= 240;
   ```

   The contacts header applies the same idea horizontally: a `LayoutBuilder` width budget drops the "Session Manager" label (keeping the status icon and restart button) when the card is too narrow for the full chip.

## Why This Works

`MediaQuery` and flag-derived getters describe the window, not the slot a widget was actually given — padding, branch selection, and ancestor caps can all make them disagree, and every disagreement becomes a silent overflow at some intermediate window size. `LayoutBuilder` measures the real constraints at the point of decision, so the branch and the content sizing can never drift apart. Designating exactly one slack absorber per row (a tight `Expanded`) gives the flex algorithm a defined place to put negative slack, converting overflow into ellipsization. Gating or scroll-wrapping the one dispensable element converts a hard vertical overflow into graceful degradation while keeping every essential control visible.

## Prevention

- **Layout test harness with a size matrix.** `test/widgets/home/support/layout_harness.dart` plus the four `test/widgets/home/*_layout_test.dart` suites pump the home layout across a width × height matrix and assert zero overflow exceptions. The harness loads the bundled Nunito font so test text metrics match production — without it, overflow boundaries measured in tests drift from the real app.
- **One constraints source per decision.** Any responsive branch must be driven by the same constraint source that chose the branch (`LayoutBuilder` measurements), never by `MediaQuery` or a width-gated flag read deeper in the tree.
- **Designated slack absorber in every row.** Rows with trailing buttons get one tight `Expanded` text slot; never a bare `Spacer` between variable-width content, which lets buttons float mid-row.
- **Mark the dispensable element.** When adding content to a fixed-height card, decide which element is droppable and gate it on measured height (or wrap the region in a scroll view) at the same time.

## Related Issues

- GitHub issue #58 — the overflow report this branch fixes.
- `docs/solutions/ui-bugs/incoming-call-timeout-recovery-2026-07-31.md` — separate ui-bugs entry (dialog-route races); same UI layer, unrelated mechanism.
