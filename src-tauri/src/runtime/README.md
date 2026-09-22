# Runtime

Owner: conversation/coding turn execution, cancellation, event delivery, and result adoption. Role Routing owns actor authorization; providers own transport.

Preserve:
- Bind execution to the persisted input/run and Role Routing permit. Do not reselect an actor downstream.
- Commit state before effects; reject cancelled, stale, or already-finalized results. Keep message persistence and adoption atomic.
- UI delivery, provider completion, message commit, and speech completion are distinct stages.
- Voice and text use the ordinary turn path. Do not restore LFM classification/delegation as a prerequisite for voice reasoning.
- Keep required-context, scope, generation, and output-permission checks at dispatch and adoption boundaries.

Locate:
- IPC/start/cancel: `start_turn.rs`, `turns.rs` and `turns.d/`; search `prepare_runtime_run` / `execute_turn`.
- Conversation orchestration: `conversation_turn.rs`; provider execution: `conversation_provider_route.rs` and its `.d/`; SDK: `conversation_codex_dispatch.rs`.
- Role permit: `conversation_inputs_roles.d/01.rs`; multi-step results: `conversation_role_steps.rs`, `role_step_sink.rs`, `../role_routing/repository_turns/`.
- Context: `conversation_inputs.rs`, `conversation_context.rs`, `context/`; search `generation`, `scope`, `world` only as needed.
- Final persistence: `../providers/session_store.rs`; delivery: `event_hub.rs`, `voice_response.rs`.
- Tools: `agent_tools.rs`, `../providers/stream/agent_dispatch.rs`; web tools: `web_fetch/`.
- Process lifecycle: `supervisor.rs`, `codex_supervise.rs`, `pi/`; recovery: `conversation_recovery.rs`.
- Tests: `conversation_inputs_roles.d/02.rs`, adjacent tests, `pi/tests.rs`.

Trace: runId -> input message -> role root/step -> provider session -> context generation -> accepted message -> speech delivery. A terminal UI event alone proves none of the preceding I/O.
