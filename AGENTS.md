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

- Unit tests may be executed directly with `cargo test --manifest-path ./rust/Cargo.toml`
- Integration tests must be executed with `cargo test --manifest-path ./rust/Cargo.toml --test core_integration_test --features integration-testing`
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
