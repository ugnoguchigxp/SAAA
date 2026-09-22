# Memory

Owner: bounded history projection, conversation recall, external typed recall/search, source-backed Personal State, World projections, and forget propagation.

Raw conversation_messages, transient context windows, retrieved evidence, and derived assertions are different layers. World indexes are projections of Personal State, not a second canonical history.

Preserve:
- Current user input is instruction; historical/projection/tool data has no instruction authority. Never derive grants, scope, or adopted goals from quoted memory.
- Preserve source/version/digest/dependencies; candidate, disputed, pending, and active states are not interchangeable.
- Keep context budgets, required-source validation, omission/health reporting, recall call limits, time ranges, and cursor scope. No-hit does not prove absence.
- Revalidate source/scope/epoch at commit, dispatch, and output. No inference await inside DB transactions.
- Background extraction uses scheduler slots/cancellation. Unverified product/delivery contracts cannot fall back to synthetic adapters or another model.
- Forget invalidates dependent state/generations/artifacts, active run/TTS, learning, and Steward authority. Track remote cleanup separately; never claim guaranteed physical erasure.

Locate:
- Context: `context_window.d/01.rs` (`load`, `compose`), `context_window.d/02.rs` -> `../runtime/context/` for final wire/generation validation.
- Enable/continuity: `control_plane/mod.d/01.rs` (`memory_enabled`: SAAA_MEMORY_ENABLED=1); raw history remains independently stored.
- Recall: `contracts.rs`, `recall/mod.d/01.rs`, `recall/search.rs`; dispatch: `../providers/stream/agent_dispatch.rs`, `../providers/stream/recall_dispatch.rs`.
- External contracts/transport: `typed_recall.rs`, `context_still_recall.rs`, `context_still_search.rs` and their `.d/` files.
- Extract/store: `personal_state/worker.rs`, `personal_state/worker_run.rs`, `personal_state/jobs.rs`, `personal_state/sources.rs`, `personal_state/store.rs`.
- State semantics: `../../../crates/personal-state-core/src/`; projection: `personal_state/projection.rs`; World: `personal_state/world/mod.rs`.
- Generation/output: `personal_state/generation.rs`, `personal_state/output.rs`; product binding: `personal_state/managed.rs`, `personal_state/product_binding.rs`.
- Forget/recovery: `personal_state/commands.rs`, `personal_state/journal.rs`, `personal_state/product_cleanup.rs`, `personal_state/schema.sql`.
- Tests: module split tests, `personal_state/tests.rs`, `personal_state/product_tests.rs`, core crate tests.

Trace source -> job -> assertion/dependencies -> generation inputs -> output permission. personal_generations and runtime context_generations are separate ledgers; inspect actual request/cleanup receipts, not only UI counts.
