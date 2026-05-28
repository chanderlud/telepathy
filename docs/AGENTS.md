### Agent Skills
- Search .cursor/skills for relevant skills.

### Formatting and Lint Rules
- Run `cargo fmt --manifest-path .\rust\Cargo.toml --all` in the project root directory after editing Rust files.
- After editing Rust files in a specific package, run `cargo clippy --manifest-path .\rust\Cargo.toml -p <package_name>` from the project root directory.
- For example, after editing files in telepathy-core, you should run `cargo clippy --manifest-path .\rust\Cargo.toml -p telepathy_core`.
- After editing Dart files, run `flutter analyze` in the project root directory.

### Flutter Rust Bridge Rules
- After editing pub members of telepathy-core, you must run `flutter_rust_bridge_codegen generate` to regenerate the bindings.
- If the codegen command is unavailable, try running `cargo install flutter_rust_bridge_codegen`.