# Steward

Owner: delegated goals, source-bound grants, bounded tasks/plans, dispatch, evidence-based verification, and durable report delivery. Normal chat and coding processes have separate owners.

Goal defines success; delegation grants workspace/operations/budget; task binds plan to coding job/run; outbox tracks report delivery. Accepted != started != verified != delivered.

Preserve:
- Bind model proposals to persisted user input; quoted text/model prose cannot invent or widen grants.
- Public work_propose supports read/test_run; work_amend changes notification policy, not operations/budget.
- Admission commits goal/plan/task/reservation/intent together. Dispatch commits authority/slot/job/binding/receipt before launching.
- Replanning stays within delegation scope/budget. Withdrawal cannot undo already-performed effects.
- Verify host evidence for the adopted run, never model success text. Missing/stale evidence is not success.
- Keep event cursor, terminal dedupe, and report outbox atomic. Speech holds delay delivery, not task execution replay.
- Forget/withdraw invalidates authority across queued work, execution, and reporting.

Locate:
- Model entry: `tools.rs`; IPC: `commands.rs`; receipt: `repository.d/01.rs` (`propose`) -> `intake.rs` -> `authority.rs`, `request_intent.rs`, `admission.rs`.
- Dispatch/gates: `dispatch.rs` (`prepare_candidate`, `finish`, `dispatch_scheduled`), `queue.rs`, `budget.rs`.
- Scheduling/wake: `pump.rs`, `work_queue.rs`, `../schedule/tick.rs`; legacy trigger entry: `reduce.rs` (`on_user_message`).
- Coding events/task/replan: `driver.rs`, `repository.d/02.rs`; evidence: `evidence.rs`, `verifier.rs`, `execution_contracts.rs`.
- Host recipes: `recipes.rs`; plans: `plans.rs`; revoke: `invalidation.rs`, `commands.rs`.
- Reports/speech: `report.d/01.rs`, `outbox.rs`, `repository.d/03.rs`.
- Schema: `schema.rs`, `schema_execution.rs`; tests: `tests.rs`, `tests/dwr.rs`, `tests/rf5.rs`, `tests/migration.rs`.

Trace steward_goals -> delegations -> tasks -> adopted coding run/evidence -> steward_reports. Check actual call sites before treating offline contracts as active paths.
