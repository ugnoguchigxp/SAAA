# Required Context results

Recorded 2026-09-21.

## Automated verification

- `japanese-fixed-cases.md`: **60 fixed cases, 0 malformed rows**. Every case records a
  time-ordered input, expected state, and prohibited action. This validates the evaluation ledger
  only; it is not a configured-model result.
- `cargo test --locked --manifest-path src-tauri/Cargo.toml --lib runtime::context::required --offline`:
  **2 passed**.
- `cargo test --locked --manifest-path src-tauri/Cargo.toml --lib
  runtime::context::generation::tests::required_receipt_is_written_before_dispatch_without_context_text
  --offline`: **1 passed**.
- `cargo test --locked --manifest-path src-tauri/Cargo.toml --lib runtime::context::generation_inputs
  --offline`: **1 passed**.
- `CARGO_TARGET_DIR=/tmp/saaa-required-context-target cargo test --locked --manifest-path
  src-tauri/Cargo.toml --lib runtime::context::broker::tests --offline`: **9 passed**. This includes
  adapter wrapper/tool reservations and the 1/100/512-candidate allocation gate.
- `CARGO_TARGET_DIR=/tmp/saaa-required-context-target cargo test --locked --manifest-path
  src-tauri/Cargo.toml --lib runtime::context::generation::tests --offline`: **12 passed**. This
  includes Scope and erased-assertion changes both before dispatch and before completion.
- `delegation_withdrawal_before_dispatch_rejects_the_generation_cas` and
  `completed_task_before_dispatch_rejects_the_generation_cas`: **2 passed**. Active delegated
  work and coding runs are source-backed Must candidates; withdrawing the delegation or settling
  the run before dispatch invalidates the generation CAS.
- `corrected_assertion_before_dispatch_rejects_the_generation_cas`: **1 passed**. A new
  Personal State transition changes the selected assertion version and prevents dispatch of the
  pre-correction generation.
- `correction_after_provider_response_is_rejected_before_a_tool_starts`: **1 passed**. The
  adapter-facing generation revalidation rejects a correction that arrives after an LLM response
  but before the proposed Tool can begin; both OpenAI-compatible and AgentSession Tool loops call
  that guard and map it to typed recovery codes.
- `CARGO_TARGET_DIR=/tmp/saaa-required-context-target cargo test --locked --manifest-path
  src-tauri/Cargo.toml --lib runtime::turns::required_context_failure_code_tests --offline`:
  **1 passed**.
- `CARGO_TARGET_DIR=/tmp/saaa-required-context-target cargo test --locked --manifest-path
  src-tauri/Cargo.toml --lib runtime::context --offline`: **93 passed, 2 ignored, 0 failed**.
  The ignored cases are opt-in World development performance gates.
- `bun run ipc:check`: **5 passed**; generated runtime-event bindings match the added context
  failure codes. `bun run typecheck` also passed.
- `bun test tests/required-context-recovery.test.ts`: **2 passed**. The three context refusal
  codes stay distinct and suppress automatic retry; an ordinary provider failure remains retryable.
- After removing the generic pre-compose in favor of per-provider admission,
  `runtime::context::broker::tests`: **9 passed** and
  `runtime::turns::required_context_failure_code_tests`: **2 passed**.
- `final_wire_budget_never_demotes_required_context_to_a_normal_size_failure`: **1 passed**.
  A final serialized body over the 64 KiB conservative input allotment is classified as a
  required-context refusal only when Must content is present; an otherwise oversized request
  stays a normal request-size failure.
- `exact_tool_schema_reservation_replaces_the_conservative_default`: **1 passed**. The concrete
  OpenAI-compatible/DynamicLan dispatch now reserves the exact JSON size of the initial offered
  tools after its provider session is acquired, replacing the former fixed 8 KiB estimate.
- `capability_wrappers_have_a_measurable_wire_reservation`: **1 passed**. AgentSession uses the
  same initial-input decoration as its dispatch path to reserve the serialized UI/coding
  capability wrapper delta; its existing local SSE workflows are **3 passed, 1 live ignored**.
- `tool_follow_up_trim_keeps_required_history_current_input_and_tool_suffix` and
  `tool_follow_up_trim_keeps_required_wrapped_history`: **2 passed**. OpenAI-compatible and
  AgentSession retry an oversized Tool follow-up only after removing optional pre-turn history;
  both preserve the current input, Tool continuation data, and selected Must content.
- `tool_follow_up_trim_recovers_a_required_wire_overflow`: **1 passed**. A Tool follow-up over
  the 64KiB conservative input allotment is recomposed below that limit by evicting only old
  optional history; the final wire still contains the selected Must candidate, current input,
  and Tool result.
- `tool_follow_up_trim_recovers_an_agent_session_wire_overflow`: **1 passed**. The equivalent
  AgentSession wrapper is also reduced from over 64KiB to within the conservative allocation
  without losing its Must candidate, current input, or Tool-result frame.
- `scope_change_while_provider_finishes_rejects_the_late_result_with_a_context_code`: **1
  passed**. A local Chat Completions HTTP fixture advances the Scope epoch after dispatch and
  before response completion; the provider result is rejected as `context-scope-changed` rather
  than being accepted as an internal/ordinary provider success.
- `scope_change_before_agent_session_completion_has_a_typed_context_failure`: **1 passed**.
  AgentSession's round adapter returns `ContextScopeChanged`, rather than an internal failure,
  when its dispatched receipt becomes stale before completion.
- `additive_migration_preserves_old_generations_and_adds_digest_receipts`: **1 passed**. An old
  `context_generations` table is upgraded in place, retains its existing generation, receives
  `required_set_digest` and `scope_digest`, and remains safe to migrate twice. Existing provider
  settings migration tests are **9 passed**.
- `cargo test --locked --manifest-path src-tauri/Cargo.toml --lib providers::chat_completions::
  --offline`: **21 passed**. `providers::agent_session::sse::`: **16 passed, 1 live ignored**.
- After the AgentSession completion-path correction, `CARGO_TARGET_DIR=/tmp/saaa-required-context-target
  cargo test --locked --manifest-path src-tauri/Cargo.toml --lib providers::agent_session::sse::
  --offline`: **18 passed, 1 live ignored**. A stale Context generation at SSE completion now
  returns its typed `ContextScopeChanged`/`RequiredContextUnavailable` result instead of `Internal`.
- `CARGO_TARGET_DIR=/tmp/saaa-required-context-target cargo test --locked --manifest-path
  src-tauri/Cargo.toml --lib runtime::context --offline`: **93 passed, 2 ignored, 0 failed**
  after that correction.
- `cargo check --locked --manifest-path src-tauri/Cargo.toml --offline`: passed (warnings are in
  concurrently developed role-routing/adaptive modules).
- A later `bun run typecheck` passed. `bun run check:local` still stops at formatting in the
  unrelated dirty `CodingConnectionFields.tsx` and `RoleRoutingSection.tsx` files.
- A later full `runtime::context` run after the current integration is green: **93 passed,
  2 ignored, 0 failed**. The ignored cases are explicit World development performance gates;
  they are not acceptance evidence for this plan's performance criterion.
- Latest allocation p95 (same host, debug test build): 1 candidate **2.398 ms**, 100 candidates
  **0.887 ms**, 512 candidates **0.959 ms**; all were below the 20 ms target. The test samples
  every required 1/100/512-candidate size without excluding cold values.
- `git diff --check`: passed. `cargo fmt --check` currently reports formatting changes in parallel
  World/voice/context-window files outside this change, so it was not applied to avoid rewriting
  another in-progress change.
- `bun run size:check`: **failed before IPC checks**. The ratchet has unregistered new files and
  over-limit modules across the concurrently integrated World, role-routing, steward, schedule,
  and Required Context work (including `requiredContextRecovery.ts`). Updating the shared baseline
  wholesale would mask those unrelated size increases, so it was deliberately not done here.

## Acceptance status

- Broker-level required classification, optional-history eviction, deterministic overflow
  reason-code tests, final wire validation, and digest-only receipts are implemented. The receipt
  stores `required_set_digest`, resolved `scope_digest`, the existing final request-body digest,
  and normalized source versions without retaining body text.
- Provider attempts now reserve adapter-specific wrapper/tool bytes before candidate selection;
  OpenAI-compatible/DynamicLan initial tool schema reservation is computed from the live offer
  after session acquisition; AgentSession's initial UI/coding schema and context reserve is
  computed from its live input decoration;
  no earlier generic budget decision can reject a request before the concrete provider/fallback is known.
  This is still not full acceptance: per-model capability discovery, correction/forget/
  delegation-revoke race coverage, all provider integration cases, desktop exercise of the
  interactive UI recovery actions, performance measurement, and real-model validation remain unverified or
  unimplemented as itemized in `progress.md`. IPC failure codes distinguish the three backend
  recovery states.
- Desktop smoke on 2026-09-21: the running SAAA/Vite application rendered the conversation
  page and its Settings navigation at `http://localhost:1420`. No provider was selected in that
  runtime, so this proves startup and the normal Chat integration only; it does **not** exercise
  an intentional required-context refusal or a configured-model dispatch.
