---
name: frb-bindings
description: Regenerate Flutter Rust Bridge bindings correctly — version match, codegen + cargo fmt pipeline, commit all generated output. Use after changing any public telepathy-core member or bumping flutter_rust_bridge.
---

# FRB Bindings Generation

Regenerate the Flutter Rust Bridge bindings after changing public
`telepathy-core` members or bumping `flutter_rust_bridge`. CI regenerates
the bindings and fails on any diff, so stale output blocks merge.

## Rules

- The codegen version MUST equal the pinned `flutter_rust_bridge` version
  (see `pubspec.lock` and `rust/telepathy-core/Cargo.toml`). Mismatched
  codegen produces different output (e.g. identifier mangling) and fails CI.
  Check with `flutter_rust_bridge_codegen --version`.
- Generated output is never hand-edited: `lib/core/rust/*` and
  `rust/telepathy-core/src/frb_generated.rs`.

## Procedure

Run from the repository root:

```sh
flutter_rust_bridge_codegen generate
cargo fmt --manifest-path rust/Cargo.toml --all
```

`cargo fmt` is required: raw codegen output is not rustfmt-clean (import
ordering), and the repo standard is the formatted form.

Then verify and commit every generated change:

```sh
git status --porcelain -- lib/core/rust rust/telepathy-core/src/frb_generated.rs
```

## If the codegen is missing or the wrong version

Do not `cargo install` (slow compile). Download the prebuilt binary for the
pinned version, e.g. for version X.Y.Z on Linux x86_64:

```sh
curl -sSfLO "https://github.com/fzyzcjy/flutter_rust_bridge/releases/download/vX.Y.Z/flutter_rust_bridge_codegen-x86_64-unknown-linux-gnu-vX.Y.Z.tgz"
tar -xzf flutter_rust_bridge_codegen-x86_64-unknown-linux-gnu-vX.Y.Z.tgz -C ~/.cargo/bin
```
