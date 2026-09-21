# World delivery remaining-work baseline

Recorded: 2026-09-21. This is a read-only baseline for the Terra cards; it
does not certify the later cards as complete.

| Source | Current canonical fields and ownership | Scope/revalidation boundary | Status |
| --- | --- | --- | --- |
| Coding | `coding_jobs.id,state,revision` and the current run; the existing adapter joins its task scope through `context_scope_links` | resolved Scope epoch, policy revision, owner digest and the source version are checked at dispatch/acceptance | connected by the legacy runtime view; v2 source projection pending |
| Delegated work | `steward_tasks` → `steward_delegations.goal_id` → `steward_goals`; task has `loop_state,revision`, delegation/goal have status and revision where present | join must be through an explicitly linked, allowed Scope; `conversation_id` is insufficient | not connected to WorldFrame |
| Deadline | `schedule_entries.id,scope_ref,status,due_at,revision` | all resolved allowed scopes; collection digest must cover additions/removals as well as individual revisions | only the existing bounded deadline metadata is connected |
| Situation | `SituationRuntime` owns `RuntimeInner`; snapshots expose scene, decision, signal health, monitoring state and signal sequence | snapshot without a DB lock; freshness is 3 × `sample_interval_ms` (`sample_interval_ms` defaults to 2,000 ms) | not connected to WorldFrame |
| Graph | validated `WorldSliceV2` projection and provenance | Project only, after existing World validation | connected to legacy frame |

## Existing routes and extraction entry points

| Route | Current caller | Baseline handling |
| --- | --- | --- |
| OpenAI-compatible / DynamicLan / shared LARM | `runtime/context/world/turn.rs` and the common chat-completions adapter | render plus per-request freshness check |
| AgentSession | `providers/agent_session/sse.rs` | fresh initial remote turn; tool continuation does not resend the old body |
| reasoning MCP | `providers/reasoning_mcp` | typed evidence v2 after handshake |
| Codex | `runtime/codex_context.rs`, `codex_process.rs` | bounded Scope/Task/deadline metadata; full graph is not yet injected |
| Natural-language extraction | `memory/personal_state/product_extract.rs` and the shared scheduler | no `world-extraction` purpose or strict candidate validation exists |

## Gates observed before this card

- The working tree was already dirty; all pre-existing changes are preserved.
- The project has existing global `clippy` and size-gate failures outside this
  card. They are not treated as a pass.
- Local providers remain an acceptance dependency only. Offline implementation
  and tests must not fabricate a live provider result.
