---
title: Platform Launch Smoke CI — Prove the App Starts, Not Just Builds
date: 2026-08-03
category: docs/solutions/conventions/
module: GitHub Actions CI
problem_type: process_gap
component: ci_pipeline
severity: high
symptoms:
  - "Build-only CI passes for months while a platform cannot start the app"
  - "Platform-gated code (cfg(target_os), desktop-only plugins) breaks invisibly because host checks never compile or execute it"
root_cause: missing_coverage
resolution_type: convention
related_components:
  - .github/workflows/smoke.yml
  - .github/workflows/build-flutter.yml
tags: [ci, smoke-test, android-emulator, ios-simulator, xvfb, headless-chrome, platform-coverage, regression-prevention]
---

# Platform Launch Smoke CI — Prove the App Starts, Not Just Builds

## Convention

Every pull request must launch the production-built app on every supported
platform and assert it survives 30 seconds of startup with no crash signatures.
Implemented in `.github/workflows/smoke.yml`.

Build success is not evidence of a working platform. Six Android startup
failures (see
`docs/solutions/runtime-errors/android-native-build-and-startup-chain-2026-08-03.md`)
all shipped under green build-only CI.

## Per-Platform Approach

| Platform | Runner | Build | Launch | Crash detection |
|---|---|---|---|---|
| Android | ubuntu-latest | `flutter build apk --debug` | `android-emulator-runner` (API 34, x86_64), `monkey` start | `pidof` alive + logcat grep for `FATAL EXCEPTION`, `UnsatisfiedLinkError`, `PanicException`, `MissingPluginException` |
| iOS | macos-15 | `flutter build ios --simulator --debug --no-codesign` | `simctl create/boot/install/launch` | process still in `launchctl list` after 30 s |
| macOS | macos-14 | `flutter build macos --debug` | run bundle binary directly | `kill -0` alive + log grep |
| Linux | ubuntu-24.04 | `flutter build linux --debug` | `dbus-run-session` + `gnome-keyring` + `xvfb-run` | `kill -0` alive + log grep |
| Windows | windows-2022 | `flutter build windows --debug` | `Start-Process -PassThru` | `HasExited` false + log grep |
| Web | ubuntu-24.04 | `web/build_web.sh` + `flutter build web --debug` | `google-chrome --headless=new --virtual-time-budget=20000 --dump-dom` | DOM contains Flutter view element + console log has no `Uncaught`/`Panicked` |

## Pitfalls and Rationale

- **Debug builds are intentional.** Debug Cargokit config adds x86/x86_64
  Android targets, so the emulator job exercises the same artifacts a developer
  gets from `flutter run`. Release-mode packaging differences are already
  covered by `build-flutter.yml`.
- **Android emulator runs on the same job as the APK build.** Splitting build
  and launch across jobs forces artifact upload/download and doubles queue
  time; KVM is available on all Linux runners.
- **iOS must build for the simulator.** `flutter build ios --no-codesign`
  produces a device (arm64-ios) binary that cannot run on the simulator. Use
  `--simulator`; the bundle id is `com.cflm-studios.telepathy` (hyphen, unlike
  Android's `com.cflmstudios.telepathy`).
- **Linux needs a secret service.** `flutter_secure_storage` talks to
  `org.freedesktop.secrets` at startup; without `dbus-run-session` +
  `gnome-keyring` the app fails for environmental reasons, not code reasons.
  `xvfb-run` supplies the display.
- **macOS binary path comes from `Info.plist`.** Read `CFBundleExecutable`
  instead of hardcoding the product name.
- **Web uses `--virtual-time-budget`, not `sleep`.** Headless Chrome's
  `--dump-dom` returns after the virtual time budget has driven the page's
  event loop, so Flutter's async bootstrap actually runs before the DOM is
  captured. `--enable-logging=stderr` captures JS console errors.
- **Start with environment setup inline, promote to composite actions only if
  a second workflow needs them.** The smoke jobs deliberately mirror
  `build-flutter.yml` (Flutter 3.35.7, cache keys, apt packages) so a fix in
  one place has an obvious twin in the other.

## When a Smoke Job Fails

1. Read the job log for the crash signature first — the grep patterns name the
   failure class directly (native link, JNI, missing plugin, panic).
2. Reproduce locally with the exact build command from the job, then launch.
   Host-level `flutter analyze` / `cargo clippy` cannot see platform-gated
   failures.
3. Fix forward; never weaken the assertion (shorter wait, narrower grep) to
   get green. If a platform has a legitimate environmental limitation, document
   it in this file next to the table.
