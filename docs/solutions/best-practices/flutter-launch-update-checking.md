---
title: Launch-Time Update Checking Against GitHub Releases
date: 2026-08-03
category: docs/solutions/best-practices/
module: Flutter app lifecycle and settings
problem_type: best_practice
component: development_workflow
severity: low
applies_when:
  - "Adding or modifying update-check or version-check behavior"
  - "Adding any network call that runs during app startup"
  - "Comparing semantic versions across tag formats (v-prefix, pre-release suffixes)"
related_components:
  - lib/core/utils/update_checker.dart
  - lib/core/utils/dialog_utils.dart
  - lib/controllers/preferences_controller.dart
  - lib/screens/settings/sections/general.dart
  - lib/app.dart
tags: [update-check, version-compare, github-releases, startup, preferences, flutter]
---

# Launch-Time Update Checking Against GitHub Releases

## Context

Telepathy needed a way to notify desktop users when a newer release exists
(GitHub issue #35). Update checking is a startup network call, which makes it
easy to get wrong in familiar ways: blocking first paint on an HTTP request,
crashing the app when the network is down or GitHub rate-limits, and
misfiring on version strings that carry `v` prefixes or pre-release suffixes.

## Guidance

The implementation has four parts, each with a deliberate shape:

**1. A result type, never a thrown exception** (`lib/core/utils/update_checker.dart`).
`UpdateChecker.check()` returns an `UpdateCheckResult` that is one of
`upToDate`, `updateAvailable`, or `failed`. Every failure path — non-200
status, malformed JSON, missing `tag_name`/`html_url`, invalid release URL,
timeout, transport error — is funnelled into `UpdateCheckResult.failed` and
logged via `DebugConsole.warn`. The UI can then stay simple: show the dialog
only when `availableUpdate != null`, and the launch path ignores failures
entirely.

**2. Injectable seams for the network and the installed version.**
`UpdateChecker` takes an optional `http.Client` and an optional
`installedVersion` callback (defaulting to `package_info_plus`). Tests inject
a `MockClient` and a fixed version string, so the full check logic —
parsing, comparison, failure mapping — is covered without real network or
platform channels. This is the only mock-worthy seam here: the GitHub API is
an external service.

**3. Tolerant three-segment version comparison.**
`UpdateChecker.isNewerVersion` strips a leading `v`/`V`, pads missing
segments with zero, and takes the numeric prefix of each segment (so
`v2.8.2-beta.1` compares as `2.8.2`). It compares major, minor, patch in
order and returns true only when the release is strictly newer. This avoids
pulling in a full semver package for a comparison the project's own tag
format controls.

**4. A preference-gated, post-frame launch hook.**
In `lib/app.dart`, `_TelepathyAppState.initState` schedules the check with
`WidgetsBinding.instance.addPostFrameCallback`, so first paint never waits on
HTTP. The check reads `PreferencesController.automaticUpdateChecks`
(persisted via the options store, default `true`) and shows
`showUpdateAvailableDialog` through the global `navigatorKey`, since the
post-frame `context` is not reliable for navigation. The dialog's "View
Release" action opens the release page with `url_launcher` in
`LaunchMode.externalApplication` mode. The same checker backs a manual
"check now" button and an opt-out toggle in Settings → General
(`lib/screens/settings/sections/general.dart`).

## Why This Matters

Startup is the worst place for unguarded I/O: a hung or slow request delays
first paint for every user on every launch, and an unhandled exception in
`initState` can break the whole app for users on flaky networks. Routing
every failure into a result type keeps the launch path crash-proof by
construction rather than by try/catch discipline at each call site. The
10-second timeout on the request bounds the worst case.

The manual settings button reuses the same `UpdateChecker`, so the opt-out
preference, the dialog, and the comparison logic have exactly one
implementation — a fix to any of them applies everywhere.

## When to Apply

- Any network call scheduled from `initState`: use a post-frame callback, a
  persisted opt-out preference, and a result type instead of exceptions.
- Any version comparison in this repo: reuse `UpdateChecker.isNewerVersion`
  rather than writing a new parser — GitHub tags carry a `v` prefix while
  `package_info_plus` reports the bare version.
- Any external-URL action: prefer `url_launcher` with
  `LaunchMode.externalApplication` and handle a `false` return (log, don't
  throw).

## Examples

Launch hook in `lib/app.dart`:

```dart
WidgetsBinding.instance.addPostFrameCallback((_) {
  _checkForUpdates();
});

Future<void> _checkForUpdates() async {
  if (!context.read<PreferencesController>().automaticUpdateChecks) {
    return;
  }

  final update = (await UpdateChecker().check()).availableUpdate;
  final navigator = navigatorKey.currentState;
  if (update != null && navigator?.mounted == true) {
    await showUpdateAvailableDialog(navigator!.context, update);
  }
}
```

Test seam in `test/core/utils/update_checker_test.dart`:

```dart
final checker = UpdateChecker(
  client: MockClient((request) async {
    return http.Response(
      '{"tag_name":"v2.9.0","html_url":"https://github.com/chanderlud/telepathy/releases/tag/v2.9.0"}',
      200,
    );
  }),
  installedVersion: () async => '2.8.1',
);
```

## Related

- GitHub issue #35 ("Latest version/update check") — the originating request
- PR #71 (`feature/version-checking`) — the implementation, open as of this writing
- `docs/solutions/conventions/platform-launch-smoke-ci-2026-08-03.md` — the
  launch-smoke CI convention; an update dialog shown on launch is startup
  behavior the smoke tests exercise
