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
- After editing Dart files, run `flutter analyze` in the project root directory, then run `dart format .` when all cleanup if finished.

## Rust Tests

* Prefer `cargo nextest run`; use `cargo test` only when nextest cannot support the required mode, and note why.
* Run from the project root with `--manifest-path ./rust/Cargo.toml` for Bash and PowerShell compatibility.

Main Rust pass:

```sh
cargo nextest run --manifest-path ./rust/Cargo.toml --all-targets -E 'not kind(=bench) and not binary(=core_integration_test)'
```

Core integration suite:

```sh
cargo nextest run --manifest-path ./rust/Cargo.toml -p telepathy_core --test core_integration_test --features integration-testing
```

Focused integration module or test:

```sh
cargo nextest run --manifest-path ./rust/Cargo.toml -p telepathy_core --test core_integration_test --features integration-testing <module>::
cargo nextest run --manifest-path ./rust/Cargo.toml -p telepathy_core --test core_integration_test --features integration-testing <module>::<test>
```

Modules: `session_lifecycle`, `call_lifecycle`, `audio_streams`, `device_failures`, `room_lifecycle`, `call_end_copy`.

Package or single-test validation:

```sh
cargo nextest run --manifest-path ./rust/Cargo.toml -p <package> -E 'not kind(=bench)'
cargo nextest run --manifest-path ./rust/Cargo.toml -p <package> <test>
```

After changes affecting core sessions, calls, rooms, networking, or teardown, run:

```sh
cargo nextest run --manifest-path ./rust/Cargo.toml -p telepathy_core --test core_integration_test --features integration-testing --stress-count 10
```

Before handing off substantial Rust changes, run the main pass, then the stress pass.

System tests must be run manually by the developer in WSL; prompt them when applicable.

## Flutter Rust Bridge Rules

- After editing pub members of telepathy-core, you must run EXACTLY `flutter_rust_bridge_codegen generate` to regenerate the bindings.
- If the codegen command is unavailable, try running `cargo install flutter_rust_bridge_codegen`.

## Test Quality Policy

- Tests must verify real behavior through the full stack where possible
- Test production paths only. Never add test-only production code, feature-gated test hooks, or runtime switches solely to make tests possible.
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

## API Surface Rules

When extending the public or crate-level API, prefer behavior over raw state.

- Do NOT expose new `pub` fields that callers can read or mutate freely. Fields
  that are part of the construction-time contract should be `pub(crate)` and
  reached through methods, constructors, or `Deref`-style accessors.
- Prefer adding new public structs, enums, methods, traits, or functions over
  widening an existing struct's visibility. Constructors (`Self::new(...)`,
  builders) and focused methods (`fn X(&self) -> &T`, `fn set_X(&mut self, ...)`)
  are the preferred shape — they keep invariants inside the type and let it
  evolve without a breaking change.
- The same rule applies to test harnesses and fixtures in `tests/`/
  `system-tests/`: expose builder methods and accessors rather than `pub` fields
  that future tests can poke into an invalid state.
- If a value genuinely must be transparent to callers (e.g. plain DTOs crossing
  the FFI boundary, newtype wrappers with `#[transparent]`), keep the type
  trivial and document why free field access is safe.

Rationale: mutating public fields lets callers bypass invariants the type's own
methods enforce. Once a field is `pub`, any future tightening (validation, lazy
init, change of representation) is a breaking change across every downstream
including the generated Flutter bindings.
