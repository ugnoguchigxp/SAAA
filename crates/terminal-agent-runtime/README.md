# SAAA terminal runtime

Small Rust helper linked into SAAA, dispatched before GUI initialization. It never opens the host database. `run` owns the native CLI group; `mcp` and `hook` durably append bounded events; `view` renders a read-only progress stream. All modes require a private run receipt.

Tests run without credentials:

```sh
cargo test --manifest-path crates/terminal-agent-runtime/Cargo.toml
cargo clippy --manifest-path crates/terminal-agent-runtime/Cargo.toml --all-targets -- -D warnings
```

Host authorization, question decisions, verification, cancellation/recovery and report delivery live in `src-tauri/src/coding/terminal/`. No global CLI settings or provider settings are changed.
