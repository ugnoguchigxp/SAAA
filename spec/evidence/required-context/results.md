# Required Context results

Recorded 2026-09-21.

## Automated verification

- `cargo test --locked --manifest-path src-tauri/Cargo.toml --lib runtime::context::required --offline`:
  **2 passed**.
- `cargo test --locked --manifest-path src-tauri/Cargo.toml --lib
  runtime::context::generation::tests::required_receipt_is_written_before_dispatch_without_context_text
  --offline`: **1 passed**.
- `cargo test --locked --manifest-path src-tauri/Cargo.toml --lib runtime::context::generation_inputs
  --offline`: **1 passed**.
- `CARGO_TARGET_DIR=/tmp/saaa-required-context-target cargo test --locked --manifest-path
  src-tauri/Cargo.toml --lib runtime::context::broker::tests --offline`: **8 passed**. This includes
  adapter wrapper/tool reservations and the 1/100/512-candidate allocation gate.
- `CARGO_TARGET_DIR=/tmp/saaa-required-context-target cargo test --locked --manifest-path
  src-tauri/Cargo.toml --lib runtime::context::generation::tests --offline`: **12 passed**. This
  includes Scope and erased-assertion changes both before dispatch and before completion.
- `CARGO_TARGET_DIR=/tmp/saaa-required-context-target cargo test --locked --manifest-path
  src-tauri/Cargo.toml --lib runtime::turns::required_context_failure_code_tests --offline`:
  **1 passed**.
- `CARGO_TARGET_DIR=/tmp/saaa-required-context-target cargo test --locked --manifest-path
  src-tauri/Cargo.toml --lib runtime::context --offline`: **83 passed, 1 ignored, 0 failed**.
  The ignored case is an unrelated opt-in World development performance gate.
- `bun run ipc:check`: **5 passed**; generated runtime-event bindings match the added context
  failure codes. `bun run typecheck` also passed.
- Allocation p95 (same host, debug test build): 1 candidate **1.612 ms**, 100 candidates
  **0.549 ms**, 512 candidates **0.837 ms**; all were below the 20 ms target. The single-candidate
  value includes the observed cold outlier and is retained rather than normalized away.
- `git diff --check`: passed. `cargo fmt --check` currently reports formatting changes in parallel
  World/voice/context-window files outside this change, so it was not applied to avoid rewriting
  another in-progress change.

## Acceptance status

- Broker-level required classification, optional-history eviction, deterministic overflow
  reason-code tests, final wire validation, and digest-only receipts are implemented. The receipt
  stores `required_set_digest`, resolved `scope_digest`, the existing final request-body digest,
  and normalized source versions without retaining body text.
- Provider attempts now reserve adapter-specific wrapper/tool bytes before candidate selection.
  This is still not full acceptance: exact dynamic-tool-schema recomposition, correction/forget/
  delegation-revoke race coverage, all provider integration cases, interactive UI recovery
  actions, performance measurement, and real-model validation remain unverified or
  unimplemented as itemized in `progress.md`. IPC failure codes distinguish the three backend
  recovery states.
