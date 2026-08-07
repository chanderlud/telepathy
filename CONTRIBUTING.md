# Contributing to Telepathy

Thank you for considering a contribution to Telepathy. Contributions of code, tests, documentation, bug reports, and platform-specific fixes are welcome.

Telepathy is a cross-platform Flutter application backed by a Rust workspace. Changes can affect several operating systems, the Flutter-to-Rust bridge, real-time audio processing, and peer-to-peer networking, so please keep portability and performance in mind.

## Before You Start

- Search the existing issues and pull requests before opening a duplicate.
- Open an issue for significant features, protocol changes, architectural changes, or large refactors before investing substantial work.
- Keep pull requests focused. Unrelated cleanup makes review more difficult and increases the risk of cross-platform regressions.
- Never include secrets, private keys, access tokens, personal data, or captured user content in commits, logs, tests, or issue reports.

## Repository Layout

The most commonly relevant directories are:

- `lib/`: Flutter and Dart application code.
- `test/`: Flutter unit and widget tests.
- `rust/`: Rust workspace.
  - `rust/telepathy-core/`: application core, networking, and Flutter bridge API.
  - `rust/telepathy-audio/`: real-time audio capture, playback, processing, and codecs.
  - `rust/telepathy-cli/`: command-line client used by development and system tests.
- `system-tests/`: Docker Compose-backed end-to-end and networking tests with unprivileged local and privileged CI entrypoints.
- `assets/`: sounds, models, and icons bundled with the application.
- `android/`, `ios/`, `linux/`, `macos/`, `windows/`, and `web/`: platform-specific build and integration files.
- `.github/workflows/`: the authoritative CI configuration.

## Development Requirements

For general development, install:

- Git
- Flutter `3.35.x` and its bundled Dart SDK
- Rust stable through `rustup`
- The `rustfmt` and `clippy` Rust components

Use Flutter `3.35.x` for all Telepathy development at this time. The CI currently uses Flutter `3.35.7`, so that exact patch version is preferred when possible. Do not upgrade the project to a newer Flutter release as part of an unrelated pull request.

Install the Rust components with:

```sh
rustup toolchain install stable
rustup component add rustfmt clippy --toolchain stable
```

Install Flutter dependencies from the repository root:

```sh
flutter pub get
```

### Platform-specific requirements

- **Windows:** Visual Studio with the Desktop development with C++ workload.
- **Linux:** Clang, CMake, Ninja, GTK 3, ALSA, LZMA, libsecret, and a suitable C++ standard library toolchain.
- **macOS and iOS:** Xcode and the required Apple SDKs.
- **Android:** Android Studio, the Android SDK, and the Android NDK.
- **Web:** `wasm-pack`, `wasm-opt`, `flutter_rust_bridge_codegen`, and a nightly Rust toolchain with `rust-src`.

On Ubuntu, the primary native dependencies can be installed with:

```sh
sudo apt-get update
sudo apt-get install \
  clang cmake git ninja-build pkg-config \
  libgtk-3-dev liblzma-dev libstdc++-12-dev \
  libasound2-dev libsecret-1-dev
```

## Running Telepathy

Run the Flutter application from the repository root and select an available target:

```sh
flutter devices
flutter run -d <device>
```

Examples include `windows`, `linux`, `macos`, `chrome`, or a connected mobile device identifier.

Build the Rust workspace independently with:

```sh
cd rust
cargo build
```

For web development, use the repository's build script so the Rust/WASM and Flutter build steps remain consistent:

```sh
./web/build_web.sh
```

## Generated Flutter/Rust Bridge Code

The bridge configuration is defined in `flutter_rust_bridge.yaml`, with generated Dart output under `lib/core/rust/`.

When changing exported Rust bridge APIs:

1. Run the configured Flutter Rust Bridge generator:

   ```sh
   flutter_rust_bridge_codegen generate
   ```

2. Review the generated changes.
3. Commit the generated files together with the source API change.

Do not manually edit generated bridge files. Regenerate them from the source API definitions instead.

## Formatting and Linting

CI treats formatting and lint warnings as failures. Run these checks before opening a pull request.

From the repository root:

```sh
dart format --output=none --set-exit-if-changed .
flutter analyze --fatal-infos --fatal-warnings
```

For Rust:

```sh
cd rust
cargo fmt -- --check
cargo clippy -- -D warnings
```

To apply formatting locally:

```sh
dart format .
cd rust
cargo fmt
```

Avoid suppressing analyzer or Clippy warnings unless the suppression is narrow, documented, and preferable to restructuring the code.

## Tests

### Flutter tests

Run the Flutter test suite from the repository root:

```sh
flutter test test
```

Add or update tests for Dart state management, UI behavior, serialization, and regressions whenever practical.

### Rust tests

CI uses `cargo-nextest`. Install it with:

```sh
cargo install cargo-nextest --locked
```

Then run the main Rust test suite:

```sh
cd rust
cargo nextest run --all-targets -E 'not kind(=bench) and not binary(=core_integration_test)'
```

Run the core integration stress tests with:

```sh
cargo nextest run \
  -p telepathy_core \
  --test core_integration_test \
  --features integration-testing \
  --stress-count 10
```

A targeted `cargo test` command is acceptable during development, but the relevant CI-equivalent `cargo-nextest` commands should pass before submission.

### System tests

System tests require Python 3.12, Docker Compose, Linux networking tools (`ip`,
`iptables`, `ping`, and `tc`), and either unprivileged user namespaces plus
`slirp4netns` or non-interactive `sudo`. Local development uses the non-privileged entrypoint:

```sh
python -m pip install -r system-tests/requirements.txt
bash system-tests/build.sh
SYSTEM_TEST_ARTIFACTS_DIR=system-tests/artifacts \
  system-tests/run-in-user-namespace.sh python -m pytest \
  system-tests/tests \
  --save-artifacts failures
```

The local runner starts the Compose-pinned Iroh relay and DNS containers, connects the
unprivileged namespace to host services through `slirp4netns`, and always captures
logs and tears Compose down. Docker socket access is still required and is
host-root-equivalent; `sudo` is not. CI instead runs the privileged entrypoint:

```sh
system-tests/run-privileged.sh python -m pytest system-tests/tests
```

Both paths preserve nested client namespaces and per-run artifacts. See
`docs/SYSTEM-TESTS.md` for support and artifact details.

## Coding Guidelines

### Rust

- Keep `cargo clippy -- -D warnings` clean.
- Prefer explicit error propagation and useful context over panics in recoverable runtime paths.
- Do not perform blocking work directly on asynchronous executors. Use an appropriate blocking task or dedicated thread.
- Be especially careful in real-time audio paths. Avoid allocations, blocking locks, logging, filesystem access, and unpredictable work in callbacks whenever possible.
- Keep platform-specific `cfg` sections narrow and test the affected target.
- Add regression tests for bugs and unit tests for nontrivial protocol, buffering, sequencing, and DSP behavior.

### Dart and Flutter

- Keep `flutter analyze --fatal-infos --fatal-warnings` clean.
- Use `dart format` rather than manually aligning code.
- Keep business and session logic out of widgets when it can live in testable controllers or services.
- Preserve accessibility, keyboard navigation, focus behavior, and responsive layouts.
- Include screenshots or recordings for visible UI changes.

### Networking, audio, and protocol changes

Changes to networking, packet formats, serialization, buffering, timing, codecs, or call state can introduce subtle compatibility and latency problems.

For these changes:

- Update both producers and consumers of the changed format or API.
- Consider malformed, duplicated, reordered, delayed, and missing messages.
- Preserve bounded memory usage and latency under poor network conditions.
- Add tests for compatibility assumptions and failure behavior.
- Document intentional wire-format or behavioral incompatibilities in the pull request.
- Avoid logging private message content, cryptographic material, or unnecessary peer-identifying data.

## Cross-platform Changes

A change that works on one platform may not work correctly on others.

When a pull request affects platform code:

- State which platforms you tested.
- Build or test at least one affected target locally when possible.
- Keep shared logic outside platform directories unless the behavior is genuinely platform-specific.
- Do not remove or disable another platform to make the current platform pass.
- Call out platform limitations clearly in the pull request description.

## Commits and Pull Requests

Use clear, focused commit messages that describe the change. Conventional Commit prefixes such as `fix:`, `feat:`, `test:`, `refactor:`, and `docs:` are welcome but not required.

A pull request should include:

- A concise summary of what changed.
- The reason for the change.
- Related issue links, when applicable.
- Tests and checks performed.
- Platforms tested.
- Screenshots or recordings for UI changes.
- Known limitations, follow-up work, or compatibility concerns.

Before submitting, verify the relevant items:

- [ ] Dart code is formatted.
- [ ] Flutter analysis passes without warnings or infos.
- [ ] Rust code is formatted.
- [ ] Clippy passes with warnings denied.
- [ ] Relevant Flutter, Rust, integration, or system tests pass.
- [ ] Generated bridge files are updated when exported Rust APIs changed.
- [ ] No secrets, private data, build artifacts, or unrelated files are included.
- [ ] User-visible or protocol behavior changes are documented.

## Reporting Bugs

A useful bug report should include:

- Operating system and version.
- Telepathy version or commit SHA.
- Hardware relevant to the issue, especially audio devices.
- Exact reproduction steps.
- Expected and actual behavior.
- Whether the issue is consistent or intermittent.
- Sanitized logs, screenshots, or recordings.

For audio or call-quality issues, also include the input/output devices, sample rate when known, selected codec or noise-suppression mode, network type, and whether the problem occurs with multiple peers.

## License

By contributing to Telepathy, you agree that your contributions will be licensed under the repository's [MIT License](LICENSE).

Please keep discussions technical, constructive, and respectful. Review comments should address the work rather than the person doing it.
