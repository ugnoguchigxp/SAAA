# Required Context results

Recorded 2026-09-21.

## Automated verification

- `cargo test --locked --manifest-path src-tauri/Cargo.toml --lib runtime::context` was invoked.
  It could not complete because pre-existing dirty Schedule work fails compilation:
  `calendar::reconcile::kind_task_run` and `ignore_foreign` are missing, and `tick.rs` references
  removed `decide::fire_result` and `Decision::Drop` APIs.
- The same compilation exposed and then verified the absence of type errors in the required
  context change after correction. `rustfmt` was run on all changed Rust files and `git diff
  --check` passed.

## Acceptance status

- Broker-level required classification, optional-history eviction, and deterministic overflow
  reason-code tests are implemented. Final wire validation and digest-only required-set receipts
  are also implemented for the common OpenAI-compatible/DynamicLan/voice and AgentSession paths.
- End-to-end provider, fallback, voice, AgentSession/Codex, UI recovery guidance, real-model, and
  performance acceptance remain blocked from execution by the unrelated crate compilation errors
  above. They are not reported as passing.
