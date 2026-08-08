# AGENTS.md

## Paths

* Rust workspace: `rust/`
* Core: `rust/telepathy-core`
* Audio: `rust/telepathy-audio`
* System-test CLI: `rust/telepathy-cli`
* Flutter: `lib/`
* Docs: `docs/`
* System tests: `system-tests/`
* Generated, never read/edit: `lib/core/rust/*`, `frb_generated.rs`

`docs/solutions/` holds documented solutions to past problems (bugs, best practices, workflow patterns), organized by category with YAML frontmatter (`module`, `tags`, `problem_type`) — relevant when implementing or debugging in documented areas. `docs/CONCEPTS.md` is the shared domain vocabulary for the session/call/room machinery.

Run all commands from the repository root.

## Checks

After Rust edits:

```sh
cargo fmt --manifest-path rust/Cargo.toml --all
cargo clippy --manifest-path rust/Cargo.toml -p <package>
```

Use package names such as `telepathy_core`.

After Dart edits:

```sh
flutter analyze
dart format .
```

Format only after cleanup is complete.

## Rust Tests

Use the rust-testing skill.

## Flutter Rust Bridge

After changing public `telepathy-core` members, use the frb-bindings skill.

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

## API Design

Expose behavior, not mutable state.

* Avoid new `pub` fields.
* Use `pub(crate)` for construction-only state.
* Prefer constructors, builders, methods, traits, enums, and focused accessors.
* Apply the same rules to test harnesses and fixtures.
* Public fields are acceptable only for intentionally transparent, trivial DTOs, FFI types, or newtypes; document why.

Public fields bypass invariants and make later validation, lazy initialization, or representation changes breaking changes, including for generated Flutter bindings.
