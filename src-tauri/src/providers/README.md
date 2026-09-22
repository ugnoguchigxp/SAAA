# Providers

Owner: provider resolution, authentication, allocation/session handling, HTTP/SDK transport, and provider-attempt outcomes. Actor selection remains a Role Routing responsibility.

Preserve:
- Direct OpenAI-compatible, dynamic LAN/Harness, and agent-session paths have different setup contracts. Do not infer one from another's configuration.
- Selection, request preparation, send, headers, response terminal, and server receipt are distinct evidence.
- Preserve timeout/cancellation and retry/fallback policy; never hide a failed provider by silently choosing another.
- SSE success requires the completion contract, not merely EOF. Separate connection failure, interrupted response, protocol failure, and partial output.
- Allocation credentials/leases must remain bound to the actual request. Never log credentials or raw private payloads as routine diagnostics.

Locate:
- Resolve route/policy: `routing.rs`, `routing/`, `route_policy.rs`; actual caller: `../runtime/conversation_inputs_roles.d/01.rs`.
- Direct dispatch/options: `stream/mod.rs` -> `chat_completions/mod.rs`; shared wire options: `../../../crates/larm-session/src/http_api.rs`.
- HTTP/status/retry: `http.rs`; SSE framing/terminal: `chat_completions/sse.rs`, `chat_completions/chunks.rs`.
- Harness claim/probe/release: `dynamic_lan/`; allocation dispatch: `stream/dynamic_lan.rs`; reasoning service: `reasoning_mcp/`.
- Voice session transport: `larm_voice/`; existence of legacy frontdesk code does not authorize its use in normal conversation.
- Agent sessions: `agent_session.rs`, `agent_session/`; capability discovery: `probe.rs`, `openai_compatible/`.
- Failure/transport audit: `stream/attempt.rs`; session/message commit: `session_store.rs`.
- Tests: `chat_completions/tests.rs`, `dynamic_lan/`, `agent_session/`.

Trace provider_session_id/request_id through provider_transport_events and server logs. Probe success does not prove generation success; mock transport tests do not prove live reachability.
