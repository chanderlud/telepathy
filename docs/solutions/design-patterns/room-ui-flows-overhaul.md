---
title: Room UI Flows Overhaul — Card Chooser, Inline Validation, Always-Available Active Room Panel
date: 2026-08-03
last_updated: 2026-08-04
category: docs/solutions/design-patterns/
module: Flutter contacts/room UI
problem_type: design_pattern
component: frontend_stimulus
severity: medium
applies_when:
  - Building or revising add/create dialogs that collect structured input (contacts, rooms, members)
  - A call or session panel must remain reachable in every layout breakpoint, including narrow windows
  - Widgets need Rust-bridge-derived objects (contacts, peer-id validation) in tests or a mock backend
  - List rows need at-a-glance reachability state (online counts, direct vs relayed connection)
related_components:
  - lib/widgets/contacts/add_entry_dialog.dart
  - lib/widgets/call/room_details_widget.dart
  - lib/widgets/contacts/room_widget.dart
  - lib/screens/home/home_page.dart
  - lib/controllers/profiles_controller.dart
  - lib/core/testing/mock_backend.dart
tags: [flutter, rooms, contacts, inline-validation, status-chips, mock-backend, layout-breakpoints, provider]
---

# Room UI Flows Overhaul — Card Chooser, Inline Validation, Always-Available Active Room Panel

## Context

The add contact / add room flow and the active room panel were reworked in PR #16. Before the change, the "add" button opened a bare `SimpleDialog` wrapping a single `ContactForm`; validation failures surfaced as modal error popups stacked on top of the form dialog. The active room panel was a static "Room Details" title plus a hangup icon and two comma-joined name strings, it carried a hardcoded wide-layout max-height constraint, and it was only mounted in the wide layout — narrow windows had no room panel at all, which meant no room state and no hangup control during an active room call. Widgets also called the Rust bridge directly (`Contact.new`, `validatePeerId`), which made the forms untestable without the native core.

## Guidance

Five patterns, each grounded in the current tree:

**1. Chooser cards with a view-swapping dialog state machine.** `AddEntryDialog` holds a `_AddEntryView { chooser, contact, room }` enum and swaps the dialog body via `setState` (`lib/widgets/contacts/add_entry_dialog.dart:41-48`). The chooser is two `_OptionCard`s — icon, title, one-line description ("Call one person directly" / "Group call with several peers") — with an animated hover border. The header shows a back button on every non-chooser view (`add_entry_dialog.dart:67-69`), so the dialog is one route that never stacks.

**2. Inline validation instead of modal error popups.** Both forms keep a `String? _error` rendered as a `_ErrorLine` under the fields, clear it on any input change, and gate the submit button with a `canSubmit` check (disabled until nickname and peer input are non-empty). All rejection paths — duplicate contact, adding yourself, duplicate/invalid peer id, existing room — set `_error` and return instead of opening a dialog (`add_entry_dialog.dart:270-285`, `378-399`, `444-448`). `DartError` from the bridge is caught at the submit boundary and converted to the same inline line.

**3. Members as removable chips with nickname resolution.** The add-room form accumulates peer ids in a `List<String>` and renders them as `_MemberChip` pills whose label resolves through the contacts map, falling back to a truncated peer id (`add_entry_dialog.dart:538-548`, `_truncatePeerId` at `574-577`). Enter or the plus button adds; a paste-room-details button parses clipboard text through `parseRoomDetails` (`lib/core/utils/room_format_utils.dart:20`), validates every parsed id, prefills the room name, and dedupes/self-filters members. The same chip visual language (`MemberStatusChip`) is reused for the active room panel and the edit-room dialog, so "person pill" looks identical everywhere.

**4. The active room panel replaces the contacts list in every breakpoint.** `RoomDetailsWidget` now shows the room name, an `N/M online` counter, a Leave-room button, and members as status-dot chips grouped into Online/Offline sections (`lib/widgets/call/room_details_widget.dart:52-125`). The widget returns `SizedBox.shrink()` when no room is active (`room_details_widget.dart:27-28`) and carries no height constraint of its own — the parent `SizedBox` in `home_page.dart` owns the breakpoint-specific height (`lib/screens/home/home_page.dart:65-79` for wide, `129-143` for narrow). The narrow layout mirrors the wide one: while `stateController.activeRoom != null`, the panel mounts in place of the contacts list, so narrow windows finally expose room state and a hangup control.

**5. Rooms lead the list; inject bridge touches at the controller.** Rooms render ahead of contacts in the list so a long contact list never pushes them out of view (`lib/widgets/contacts/contacts_list.dart:41-45`). To keep widgets off the Rust bridge, `ProfilesController` accepts an injectable `ContactFactory` and `PeerIdValidator` (`lib/controllers/profiles_controller.dart:16-22`, `44-55`); that seam is what lets `lib/mock_main.dart` boot the real UI against `lib/core/testing/mock_backend.dart` (scenario selected with `--dart-define=MOCK_SCENARIO=demo|room-active|empty`, target selected via `TARGET` in `scripts/run-linux-debug.sh`) for headless visual QA.

**Dropped after review: contacts-list status indicators.** The first iteration added a green/amber direct/relayed dot on contact rows and an `online/total` session-count badge on room rows. Both were removed: a room's online count is unreliable because anonymous members have no session to count, and the direct/relayed dot added noise next to the existing `direct`/`relayed` text. The `N/M online` counter in the active room panel survives because it reads the room's authoritative `online` list from the call itself, not session guesses. Section labels in that panel are count-first (`3 online`, `1 offline`) to match the header counter's phrasing rather than introducing a new `Label — N` separator style.

## Why This Matters

Modal error popups force the user to dismiss a second dialog before fixing input, and they stack routes on the navigator — the failure mode documented in `docs/solutions/ui-bugs/incoming-call-timeout-recovery-2026-07-31.md`. Inline errors keep focus in the form and make the disabled-until-valid submit self-explanatory. Mounting the room panel in narrow layouts closes a real hole: previously a narrow window during a room call had no hangup control at all. The injectable controller seam is what makes both the widget tests (`test/widgets/contacts/add_entry_dialog_test.dart`) and the mock-backend visual-QA entrypoint possible without test-only production hooks.

## When to Apply

- Any multi-entity "add" flow: prefer a single dialog with a view enum over stacked dialogs, and inline error lines over modal alerts.
- Any call/session UI: the active-session panel must mount in every layout breakpoint, with hangup reachable everywhere; the widget should own no breakpoint-specific sizing — the parent does.
- Any widget that needs a bridge-constructed object: add a factory/validator typedef on the controller rather than calling the bridge from the widget tree.

## Examples

Before (add flow): the contacts-list add button built a `SimpleDialog` wrapping `ContactForm` inline at the call site; errors were separate modal popups. After: one call —

```dart
onPressed: () => showAddEntryDialog(context),
```

and the dialog owns chooser → form navigation internally. Validation is a state field, not a route:

```dart
final bool canSubmit = _nicknameInput.text.trim().isNotEmpty &&
    _peerIdInput.text.trim().isNotEmpty;
// ...
Button(text: 'Add Contact', disabled: !canSubmit, onPressed: () => _submit(...));
```

Before (narrow layout): the narrow branch of `HomePage` always mounted `contactsList` in the top section. After:

```dart
child: context.watch<StateController>().activeRoom != null
    ? SizedBox(
        height: isCompact
            ? AppConstants.topSectionMaxHeightNarrowCompact
            : AppConstants.topSectionMaxHeightNarrow,
        child: const RoomDetailsWidget())
    : contactsList,
```

## Related

- docs/solutions/ui-bugs/incoming-call-timeout-recovery-2026-07-31.md — why stacked modal routes are fragile around call lifecycles
- .agents/skills/linux-gui-debugging/SKILL.md — headless QA workflow that drives the mock-backend entrypoint
- PR #16 — the overhaul this doc captures
