---
name: rust-tests
description: "Use this skill when testing Rust changes in Telepathy. Covers nextest commands, core integration tests, focused tests, stress testing, and handoff requirements. For end-to-end system testing, use the system-tests skill."
------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------

# Rust Testing

Prefer `cargo nextest`. Use `cargo test` only when required, and state why.

## Main Suite

```sh
cargo nextest run --manifest-path rust/Cargo.toml --all-targets -E 'not kind(=bench) and not binary(=core_integration_test)'
```

## Core Integration Suite

```sh
cargo nextest run --manifest-path rust/Cargo.toml -p telepathy_core --test core_integration_test --features integration-testing
```

Focused module:

```sh
cargo nextest run --manifest-path rust/Cargo.toml -p telepathy_core --test core_integration_test --features integration-testing <module>::
```

Focused test:

```sh
cargo nextest run --manifest-path rust/Cargo.toml -p telepathy_core --test core_integration_test --features integration-testing <module>::<test>
```

Integration modules:

* `session_lifecycle`
* `call_lifecycle`
* `audio_streams`
* `device_failures`
* `room_lifecycle`
* `call_end_copy`

## Package Tests

Package suite:

```sh
cargo nextest run --manifest-path rust/Cargo.toml -p <package> -E 'not kind(=bench)'
```

Focused test:

```sh
cargo nextest run --manifest-path rust/Cargo.toml -p <package> <test>
```

## Stress Tests

For session, call, room, network, teardown, or related lifecycle changes:

```sh
cargo nextest run --manifest-path rust/Cargo.toml -p telepathy_core --test core_integration_test --features integration-testing --stress-count 10
```

## System Tests

For system-suite setup, execution, debugging, and validation, read and follow the `system-tests` skill.

Do not duplicate system-test procedures in this skill.

## Handoff

Before handing off substantial Rust work:

1. Run the main suite.
2. Run the stress suite when applicable.
3. Use the `system-tests` skill and run the applicable system suite.
4. Report any skipped, failing, flaky, or unavailable tests and why.
