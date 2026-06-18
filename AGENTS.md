# Agent Instructions

## Project Layout

- Rust backend code: ./rust/telepathy-core
- Audio crate: ./rust/telepathy-audio
- CLI for system tests: ./rust/telepathy-cli
- Flutter frontend code: ./lib
- Generated code (do NOT read these files): ./lib/core/rust/* and frb_generated.rs
- Documentation: ./docs
- System test suite: ./system-tests

## Formatting and Lint Rules

- Run `cargo fmt --manifest-path ./rust/Cargo.toml --all` in the project root directory after editing Rust files.
- After editing Rust files in a specific package, run `cargo clippy --manifest-path ./rust/Cargo.toml -p <package_name>` from the project root directory.
- For example, after editing files in telepathy-core, you should run `cargo clippy --manifest-path ./rust/Cargo.toml -p telepathy_core`.
- After editing Dart files, run `flutter analyze` in the project root directory.

## Test Execution Rules

- Prefer `cargo nextest run` over `cargo test` for Rust tests. The nextest config lives at `./rust/.config/nextest.toml`, so run nextest commands from `./rust` or pass `--manifest-path ./rust/Cargo.toml`.
- Main Rust test pass: from the project root, run `cd rust && cargo nextest run --all-targets -E 'not kind(=bench) and not binary(=core_integration_test)'`. Use this after ordinary Rust changes; it runs unit and non-benchmark test targets without enabling the `integration-testing` feature.
- Core integration stress pass: from the project root, run `cd rust && cargo nextest run -p telepathy_core --test core_integration_test --features integration-testing --stress-count 10`. Use this after changes touching telepathy-core sessions, calls, rooms, networking, teardown, or anything that may affect `./rust/telepathy-core/tests/core_integration_test.rs`.
- Full CI-equivalent Rust test sequence: run the main Rust test pass first, then the core integration stress pass. Use this before handing off substantial Rust changes.
- Targeted package tests: use `cd rust && cargo nextest run -p <package_name> -E 'not kind(=bench)'` for quick package-local validation when the core integration suite is unrelated.
- Targeted single test debugging: use `cd rust && cargo nextest run -p <package_name> <test_name>` while iterating on a specific failure, then run the broader applicable pass before finalizing.
- Only fall back to `cargo test` when nextest cannot run a required test mode; explain why in your handoff if you do.
- System tests must be manually executed in WSL by the developer, prompt them to do so

## Flutter Rust Bridge Rules

- After editing pub members of telepathy-core, you must run EXACTLY `flutter_rust_bridge_codegen generate` to regenerate the bindings.
- If the codegen command is unavailable, try running `cargo install flutter_rust_bridge_codegen`.

## Test Quality Policy

- Tests must verify real behavior through the full stack where possible
- Mocks are ONLY acceptable for external services (third-party APIs, email, payment providers)
- If you mock a database query or internal service, justify WHY in a code comment
- NEVER mock the thing you are testing
- Prefer integration-style tests over heavily mocked unit tests
- Fixtures must reflect realistic data, not minimal placeholders
- Include edge cases in fixture data (empty strings, unicode, boundary values)
- If a fixture represents a user, give it realistic attributes - not 'name="test" email="test@test.com"
- Test five scenarios per feature: happy path, validation errors, auth failures, downstream failures, edge cases
  For every test, ask: "If someone subtly breaks this feature, will THIS test actually fail?"
- For every test, ask: "Am I testing that the code works, or just that it runs without errors?"

### Anti-Patterns

- Write tests that import non-existent classes
- Claim tests pass without showing actual test output
- Mock internal code just to make tests easier to write
- Create fixtures with placeholder data like 'name="test"' or value=123
- Write tests that only verify "no exception was raised"
