---
title: Same Run Artifact Reuse for Platform Launch Smoke CI
date: 2026-08-03
last_updated: 2026-08-08
category: docs/solutions/conventions/
module: GitHub Actions CI
problem_type: workflow_issue
component: tooling
severity: high
applies_when:
  - "Adding or changing a non-release Flutter platform smoke check"
  - "Changing artifact names, upload paths, or download destinations in CI"
  - "Diagnosing a Smoke job that cannot locate its built application"
root_cause: missing_workflow_step
resolution_type: workflow_improvement
related_components:
  - .github/workflows/ci.yml
  - .github/workflows/build-flutter.yml
  - .github/workflows/smoke.yml
tags: [github-actions, smoke-ci, workflow-call, artifacts, flutter, platform-launch, ci]
---

# Same Run Artifact Reuse for Platform Launch Smoke CI

## Context

Platform launch checks must test output from the same CI run that produced it.
Rebuilding inside every Smoke job wastes runner time and can hide a mismatch
between the build workflow and launch test. `CI` now calls `Smoke` only after
`build_flutter` completes, and excludes tag refs because release builds do not
need this debug-launch gate. See `.github/workflows/ci.yml`.

`Smoke` receives Android, macOS, Linux, Windows, and web artifacts through
`actions/download-artifact@v8`. iOS remains intentional exception: its job
checks out source and locally builds a simulator-compatible app with
`flutter build ios --simulator --debug --no-codesign`. A device app from the
build matrix cannot launch in an iOS simulator. See
`.github/workflows/smoke.yml`.

PR #93 reported all 26 checks passing with this arrangement.

## Per-Platform Approach

| Platform | Runner | Artifact source | Launch | Assertion |
| --- | --- | --- | --- | --- |
| Android | ubuntu-latest | Download `telepathy_windows-2022_apk` into build/app/outputs/flutter-apk | Install telepathy.apk on API 34 x86_64 emulator and start with `monkey` | `pidof` remains present after 30 s; logcat excludes startup crash signatures |
| iOS | macos-15 | Local simulator build, not an artifact | Create, boot, install, and launch with `simctl` | Process remains listed after 30 s |
| macOS | macos-14 | Download `telepathy_macos-14_macos` into telepathy-macos.app | Run bundle executable discovered from Info.plist | Process remains alive; log excludes Dart, Rust, plugin, and segmentation failures |
| Linux | ubuntu-24.04 | Download `telepathy_ubuntu-24.04_linux` into build/linux/x64/debug/bundle | Run nested telepathy binary in D-Bus, keyring, and Xvfb session | Process remains alive; log excludes startup crash signatures |
| Windows | windows-2022 | Download `telepathy_windows-2022_windows` into build/windows/x64/runner/Debug | Start nested telepathy.exe with PowerShell | Process has not exited; output excludes Dart, Rust, and plugin failures |
| Web | ubuntu-24.04 | Download `telepathy_ubuntu-24.04_web` into build/web | Serve nested telepathy directory and open it in headless Chrome | DOM contains Flutter host; console excludes JavaScript and Flutter startup failures |

`build-flutter.yml` names each uploaded artifact with
`telepathy_${{ matrix.info.image }}_${{ matrix.info.target }}`. Smoke must use
the exact matching name and launch downloaded output without rebuilding it.
macOS uploads one path, so downloading into telepathy-macos.app restores bundle
contents directly there. Linux, Windows, and web use the multi-path upload step,
which retains the nested telepathy directory under each download destination.

## Why This Matters

Artifact reuse proves two boundaries in one run: `build_flutter` produced a
launchable platform bundle, and `Smoke` can consume its published shape. A
fresh build in Smoke could pass while artifact naming, upload layout, or
download paths are broken.

Tag exclusion keeps release packaging separate from debug-launch smoke checks.
The guard belongs on the reusable-workflow call in `ci.yml`, so tags never
schedule Smoke.

Build success alone is not evidence that a platform starts. Platform-gated
native libraries, JNI, plugins, log paths, and browser startup failures require
the app to launch on its target runtime.

## Pitfalls and Rationale

- **Debug checks are intentional.** Non-tag build matrix jobs use debug mode,
  and Smoke validates those debug artifacts. Release packaging stays in
  `build-flutter.yml` and tag refs do not schedule Smoke.
- **Android consumes a prior-job artifact.** Do not describe Android as building
  and launching in one job. CI waits for `build_flutter`, then the Android Smoke
  job downloads and installs its APK.
- **iOS must build for the simulator.** The build matrix produces an iPhoneOS
  app, while Smoke installs an iPhoneSimulator app. Keep
  `flutter build ios --simulator --debug --no-codesign`; the iOS bundle id is
  `com.cflm-studios.telepathy`, unlike Android's `com.cflmstudios.telepathy`.
- **Linux needs runtime services and libraries.** `flutter_secure_storage`
  reaches the secret service at startup, so Smoke supplies `dbus-run-session`,
  `gnome-keyring`, and `xvfb`. Ubuntu 24.04 also requires the runtime package
  `libasound2t64`, alongside GTK and secret-service libraries.
- **macOS binary path comes from Info.plist.** Read `CFBundleExecutable` instead
  of hardcoding a product name.
- **Web uses virtual time, not sleep.** Chrome's `--virtual-time-budget=20000`
  drives Flutter asynchronous bootstrap before DOM capture;
  `--enable-logging=stderr` captures console failures.

## When to Apply

- Add a platform launch job that should exercise output from `build_flutter`.
- Rename a matrix image, target, artifact, upload path, or download path.
- Change an artifact upload from one path to multiple paths, or back again.
- Update Ubuntu runtime dependencies for Linux application startup.

## Verification and Failure Triage

When adding or changing a platform artifact, verify mapping in this order:

1. Derive artifact name from `matrix.info.image` and `matrix.info.target` in
   `.github/workflows/build-flutter.yml`.
2. Match that exact name in `actions/download-artifact@v8` in
   `.github/workflows/smoke.yml`.
3. Inspect whether upload uses one path or the multi-path list. One path strips
   its directory root; multi-path output retains the telepathy directory.
4. Set Smoke launch path or web server directory to downloaded artifact shape.
5. Keep iOS local simulator build unless a simulator-specific artifact replaces
   it.
6. Run CI on a pull request and confirm every Smoke job passes. PR #93 is the
   recorded green verification, with all 26 checks passing.

When a Smoke job fails:

1. Read job log for first crash signature. Assertion patterns name native-link,
   JNI, missing-plugin, panic, and browser failure classes.
2. Reproduce with the job's build or downloaded artifact and matching runtime.
   Host `flutter analyze` and `cargo clippy` do not execute platform-gated code.
3. Fix failure without weakening startup wait or crash assertions. Document any
   verified environmental limitation beside affected platform guidance.

## Related

- `docs/solutions/runtime-errors/android-native-build-and-startup-chain-2026-08-03.md`
  explains why launch checks exist for platform-gated startup failures.
