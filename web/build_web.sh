export RUSTUP_TOOLCHAIN=nightly
export RUSTFLAGS="-Ctarget-feature=+atomics -Clink-args=--shared-memory -Clink-args=--max-memory=1073741824 -Clink-args=--import-memory -Clink-args=--export=__wasm_init_tls -Clink-args=--export=__tls_size -Clink-args=--export=__tls_align -Clink-args=--export=__tls_base"
wasm-pack build -t no-modules -d /Users/chanchan/IdeaProjects/telepathy/web/pkg \
--no-typescript --out-name telepathy rust/telepathy -- -Z build-std=std,panic_abort