# Role Routing

Owner: durable actor selection, finite step plans, execution permits, budgets, cancellation/revision, and final-answer adoption. Provider transport and LAN reachability observation remain downstream.

Role maps to actor; actor binds transport/provider/model. Recipe compiles action/roles into steps. Root owns policy snapshot, revision, deadline, and result. Permit authorizes the persisted active step, not a new selection. Home and away are two `respond` recipes chosen once, when the turn is accepted.

Preserve:
- Reachability is a selection input (`unreachable_actor_ids`), not a second selector. `Unknown` stays eligible. Dispatch revalidates the chosen LAN actor and fails closed if it is now unreachable; it does not pick another recipe.
- Eligible recipes tie-break by recipe id string order. A home/away pair must sort home first (`10-respond-home` before `20-respond-away`).
- Use the root's immutable policy/decision; match permit actor/revision to actual dispatch. Revalidate current provider/cloud availability without reselecting. `location_fallback` records that an unreachable actor excluded another recipe. It is not legacy `fallback_provider_ids` and not a mid-turn revision change.
- Never bypass queued/draining/terminal, stale-revision, deadline, budget, or approval gates to make a provider run.
- Commit coordinator state/events before effects; no I/O across DB transactions. Preserve one active root per conversation and active reasoning-step constraints.
- Claim/complete/finalize through the ledger; reject cancelled, superseded, duplicate, or already-adopted results.
- Tool/review/premium transitions share root authority and budgets. Specialist cannot publish the final answer; premium approval binds candidate/revision.
- Role-bound provider dispatch clears legacy fallback. Normal voice uses the ordinary turn path, without LFM classification or input rewriting.
- Disable/restart reconciles state; it does not implicitly replay actor/tool effects.

Locate:
- Types/validation: `contracts.rs`; DB constraints: `schema.rs`; settings: `../persistence/settings.rs`.
- Reachability input: `availability.rs` (`selection_input_for`). Observation: `../providers/reachability.rs`, `../providers/reachability_watcher.rs`; claim-free probe: `../providers/dynamic_lan/probe.rs` (`reachable`).
- Receipt/selection: `repository_turns/start.rs`, `selection.rs` (`actor_unreachable`); acceptance caller: `../runtime/turns.d/02.rs`. Finite compiler: `recipe.rs`.
- Actual dispatch gate: `../runtime/conversation_inputs_roles.d/01.rs` (`validate_actor_host`) -> `executor.rs` (`permit_next_step`). `driver.rs` is scaffolding, not the normal conversation entry.
- Reconnect projection: `ipc.d/01.rs` (`selected_recipe_id`, `decision_reason_codes`).
- State/revision: `reducer.rs`, `coordinator.rs`, `steps.rs`, `signals.rs`, `revision.rs`, `recovery.rs`.
- Advance/review/adopt: `repository_turns/advance.rs`, `repository_turns/lifecycle.rs`, `review.rs`, `proposals.rs`.
- Tools/context: `tools.rs`, `tool_ledger.rs`, `tool_specialist.rs`, `context.rs`.
- Speech/replay/learning: `speech_repository.rs`, `ipc.rs`, `learning/`, `ranker.rs`.
- Boundary tests: `../runtime/conversation_inputs_roles.d/02.rs`; module tests and `repository_turns/tests.rs`.

Trace runtime_run_id -> rr_roots -> rr_decisions -> rr_steps -> provider session -> result_message_id -> rr_speech. Selection/permit is not server receipt or answer delivery.
