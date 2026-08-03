---
title: Android Startup Chain — Missing Native Library, Bad JNI Version, Desktop-Only Plugins, Read-Only Log Path
date: 2026-08-03
category: docs/solutions/runtime-errors/
module: Android build integration and app startup
problem_type: runtime_error
component: platform_integration
severity: critical
symptoms:
  - "java.lang.UnsatisfiedLinkError: dlopen failed: library \"libtelepathy_core.so\" not found at MainActivity.<init>"
  - "Gradle build succeeds and APK installs, but lib/<abi>/libtelepathy_core.so is absent from the APK"
  - "'Flutter plugin not found, CargoKit plugin will not be applied.' printed during configuration while other Cargokit plugins build fine"
  - "java.lang.UnsatisfiedLinkError: Bad JNI version returned from JNI_OnLoad ... 589824"
  - "MissingPluginException(No implementation found for method ensureInitialized on channel window_manager)"
  - "PanicException(initializing rolling file appender failed ... Read-only file system) from rustSetUp"
  - "SEVERE: ld.lld: error: unable to find library -laaudio during Android Rust build"
root_cause: platform_drift
resolution_type: code_fix
related_components:
  - rust_builder/cargokit
  - telepathy-core JNI_OnLoad
  - telepathy-core tracing setup
  - window_manager usage in lib/main.dart and lib/app.dart
tags: [android, cargokit, flutter-rust-bridge, jni, aaudio, cpal, minsdk, window-manager, tracing, startup-crash, untested-platform]
---

# Android Startup Chain — Six Compounding Failures From Months of Untested Platform Drift

## Problem

Android had no CI coverage for months. During that time Flutter SDK upgrades and
desktop-first development introduced six independent startup failures. Each fix
exposed the next layer; none was detectable without actually building the APK and
launching it on a device or emulator. The failures are listed in the order they
surface.

## Failure 1: Cargokit silently skips the Rust build (library missing from APK)

**Symptom.** `UnsatisfiedLinkError: library "libtelepathy_core.so" not found` at
`System.loadLibrary` in `MainActivity.init`. `./gradlew :app:assembleDebug`
succeeds; the APK contains `libflutter.so` and other plugins' native libs but no
`libtelepathy_core.so`. Build output prints
`Flutter plugin not found, CargoKit plugin will not be applied.`

**Root cause.** Flutter moved its Gradle plugin class from the default package to
`com.flutter.gradle.FlutterPlugin` (verified in Flutter 3.44.8:
`packages/flutter_tools/gradle/build.gradle.kts` sets
`implementationClass = "com.flutter.gradle.FlutterPlugin"`). The vendored Cargokit
copy in `rust_builder/cargokit/gradle/plugin.gradle` detected the plugin with
`plugin.class.name == "FlutterPlugin"`, never matched, printed the warning, and
returned early — no cargo build task, no jniLibs source dir, no merge dependency.
Plugins shipping newer Cargokit copies (`irondash_engine_context`,
`super_native_extensions`) already matched the new name, which is why their
`.so` files packaged fine on the same machine.

**Fix (PR #74).** Match both class names in
`_findFlutterPlugin`. A `projectsEvaluated` deferral was tried first and did NOT
help — detection failed even after all projects were evaluated, proving the
problem was the name check, not evaluation timing. Do not re-introduce timing
changes without evidence.

## Failure 2: `getTargetPlatforms()` no longer exists on the plugin class

**Symptom.** `Failed to apply plugin class 'CargoKitPlugin'. No signature of
method: com.flutter.gradle.FlutterPlugin.getTargetPlatforms()`.

**Root cause.** The Kotlin rewrite removed the instance method; target platforms
now come from `FlutterPluginUtils.getTargetPlatforms(project)`, which reads the
`-Ptarget-platform` project property and falls back to
`FlutterPluginConstants.DEFAULT_PLATFORMS`.

**Fix (PR #74).** Call
`com.flutter.gradle.FlutterPluginUtils.getTargetPlatforms(project).collect()`,
identical to current upstream Cargokit. `internal` in Kotlin compiles to public
bytecode, so Groovy can call it.

## Failure 3: `-laaudio` not found while linking the Rust library

**Symptom.** `ld.lld: error: unable to find library -laaudio` when Cargokit runs
cargo for Android targets.

**Root cause.** cpal 0.18's only Android backend is AAudio (its own
`host/aaudio` module via the `ndk` crate); there is no OpenSLES fallback.
`libaaudio.so` exists in the NDK sysroot only from API 26. Cargokit passes
`--target=<triple><minSdkVersion>` to clang, so with the app's default
`minSdkVersion 24` the linker used the API-24 sysroot, which has no
`libaaudio.so` stub.

**Fix (PR #74).** Raise the default min SDK to 26 in
`android/app/build.gradle`. Linking against API-26 stubs while keeping minSdk 24
would only move the failure to a runtime `dlopen` crash on Android 7.x devices,
so the manifest-level raise is the correct fix. Note: a
`flutter.minSdkVersion` entry in `android/local.properties` overrides the
default.

## Failure 4: `JNI_OnLoad` returns a desktop JNI version

**Symptom.** `UnsatisfiedLinkError: Bad JNI version returned from JNI_OnLoad in
".../libtelepathy_core.so": 589824`. 589824 = 0x90000 = JNI 9.0.

**Root cause.** `rust/telepathy-core/src/lib.rs` returned
`jni::JNIVersion::V9` — a desktop JDK constant. Android ART accepts at most
`JNI_VERSION_1_6` and rejects the load. A second trap: the crate depends on
`jni` 0.22, where the constant is named `V1_6`, not `V6` (0.21 has `V6` and no
`V9`; both versions are in the lockfile via different deps).

**Fix (PR #75).** Return `jni::JNIVersion::V1_6`.

**Verification trap.** Host `cargo clippy`/`cargo check` cannot see the
`#[cfg(target_os = "android")]` block at all — it compiled clean while the
Android build was broken. Only `cargo check --target aarch64-linux-android`
(or a real `flutter build apk`) compiles that code. This is the single most
important lesson of the chain: host checks are not evidence for
platform-gated code.

## Failure 5: Desktop-only `window_manager` called on Android

**Symptom.** `MissingPluginException(No implementation found for method
ensureInitialized on channel window_manager)` at `main.dart`.

**Root cause.** `window_manager` only implements Windows/macOS/Linux, but
`main()` and `TelepathyApp` called `ensureInitialized`, `addListener`, and
`setPreventClose` on every non-web platform.

**Fix (PR #76).** Added `isDesktopPlatform` to the io shim
(`lib/core/utils/io_shim_native.dart` real check, `io_shim_stub.dart` `false`
for web) and gated every `windowManager` call on it. Rule for future platform
checks: guard on the platforms a feature SUPPORTS (allowlist), not on the ones
it doesn't (`!kIsWeb` blocklist) — new platforms then fail safe.

## Failure 6: Tracing writes a log file to a read-only directory

**Symptom.** `PanicException(initializing rolling file appender failed:
InitError { context: "failed to create log file", source: Os { code: 30, kind:
ReadOnlyFilesystem } })` from `rustSetUp` during startup.

**Root cause.** `rust/telepathy-core/src/flutter/logging.rs` created
`tracing_appender::rolling::daily(".", "telepathy-trace.log")` in the process
working directory. On Android the working directory is `/` — read-only (EROFS).

**Fix (PR #77).** Android attaches only the Dart log-stream layer; the rolling
JSON file layer stays desktop-only. Shared setup (Dart layer, env filter,
subscriber registration, panic hook) is deduplicated; only subscriber
construction is `cfg`-split.

## Diagnostic Commands That Pinpointed Each Layer

```sh
# Is the library in the APK at all? (Failure 1)
unzip -l build/app/outputs/flutter-apk/app-debug.apk | rg 'lib/.*/libtelepathy_core\.so'

# Did Cargokit even run? (Failures 1-2)
cd android && ./gradlew :app:assembleDebug --console=plain 2>&1 | rg -i 'cargokit|Flutter plugin not found'
./gradlew :rust_builder:tasks --all | rg -i cargokit

# Which ABI does the crashing device need?
adb shell getprop ro.product.cpu.abi

# Startup crash signatures in one pass
adb logcat -d | rg 'FATAL EXCEPTION|UnsatisfiedLinkError|PanicException|MissingPluginException'
```

## Prevention

The `Smoke` workflow (`.github/workflows/smoke.yml`) builds and LAUNCHES the app
on Android emulator, iOS simulator, macOS, Windows, Linux, and headless Chrome
on every PR, asserting the process survives 30 seconds with no crash signatures.
Build-only CI did not catch any of these six failures; launch-based CI catches
all of them. See
`docs/solutions/conventions/platform-launch-smoke-ci-2026-08-03.md`.
