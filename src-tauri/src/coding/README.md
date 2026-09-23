# Coding

Owner: registered workspaces, coding job/run receipts, command dispatch, cancellation, and recovery. The saved implementation method selects Runtime/pi or the direct Codex runner; Steward owns delegated goals and grants.

Preserve:
- Workspace registration is not authorization for arbitrary work. Bind requests to the persisted input and validated settings/profile.
- Distinguish job identity, individual run identity, adopted current_run_id, delivery receipt, and actual outcome.
- Commit job/run/source bindings before process launch; do not hold transactions across process or network waits.
- Keep duplicate-call handling and cancellation checks. Recovery must not resend ambiguous deliveries or signal unverified PIDs.
- A start receipt is not implementation completion. Old run output cannot settle a newer adopted run.

Locate:
- Model tools/contracts: `tools.rs`, `contracts.rs`; IPC: `commands.rs`.
- Validate/dispatch: `service.rs` (`execute`, `execute_delegated`, `commit_delegated_job`, `spawn_run`).
- Atomic job operations: `service_transactions.rs`; queries: `service_queries.rs`; storage/settings: `repository.rs`, `settings.rs`.
- Workspace checks: `workspace.rs`; recovery: `recovery.rs`.
- Actual process/session: `../runtime/pi/runner.rs`, `../runtime/pi/process.rs`, `../runtime/pi/session_reader.rs`.
- Direct Codex execution: `codex_runner.rs` and `../runtime/codex_process.rs`. Existing Pi Codex SDK extension profiles remain Pi jobs.
- Delegation/profile/recipes: `../steward/dispatch.rs`, `../runtime/pi/delegated_profile.rs`, `../runtime/pi/recipe_runner.rs`.
- World status projection: `world_snapshot.rs`.
- Tests: `tests.rs`, `integration/tests.rs`, `e2e/`, `../runtime/pi/tests.rs`.

Trace input/source -> job -> adopted run -> process identity/events -> terminal evidence -> report. Verify actual execution and recovery, not only tool receipt JSON.
