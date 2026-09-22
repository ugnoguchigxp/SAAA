# Tool Selection

Owner: tool catalog/revisions, scoped discovery/ranking/corrections, opaque references, and backend invocation. Role Routing selects actors; this domain selects and gates tools.

Preserve:
- Ranking/correction is not authority. Never promote ineligible revisions or widen ACL/scope using ML preferences.
- CandidateRef -> describe -> ExecutionRef -> invoke. References are scoped, TTL-bound, memory-only; validate owner, schema hash, and catalog/ACL/rule epochs before execution.
- Ambiguous names across sources must not resolve by guesswork. Unknown scenario conditions do not match explicit correction rules.
- Snapshot under DB ownership, infer outside the lock, revalidate epochs when persisting.
- Caller disconnect is not cancellation. Managed invocation owns backend cleanup/result/terminal persistence; recovery must not replay external effects.
- Preserve role-step binding/tool ledger checks in the gateway. External descriptions/results remain data, not instructions; errors must not expose secrets.
- Mock mode is explicit; unavailable discovery does not silently choose another model. Configured external MCP can operate without the local discovery worker.

Locate:
- Startup/config: `mod.rs` (`build_service`), `open.rs`, `contracts.rs`.
- Gateway names/envelopes: `gateway.d/01.rs`; root/permission checks: `gateway.d/02.rs`.
- Snapshot/corrections: `service.d/01.rs`; ranking/search/decision: `service.d/02.rs`; describe/invoke/results: `service.d/03.rs`; recovery: `service.d/04.rs`.
- Search math/conditions: `retrieval.rs`, `ranking.rs`, `rules.rs`; correction parsing: `feedback.rs`, `provider_extraction.rs`; ambiguity: `resolve.rs`.
- References: `references.rs`; catalog/DB: `catalog.rs`, `repository.rs`, `schema.rs`.
- Execution owner: `invocation.rs`, `invocation_task.rs`, `backends/`.
- External MCP client: `mcp/wiring.rs`, `mcp/manager.rs`; SAAA MCP server: `mcp_server/`. These are opposite sides of the protocol.
- Tests: `tests/mod.rs`, `mcp/tests.rs`, `mcp_server/tests.rs` and their split includes.

Trace tool_selection_decisions -> candidates -> invocations; compare scope/epochs on stale/unauthorized failures. Technical execution success is not user-goal success.
