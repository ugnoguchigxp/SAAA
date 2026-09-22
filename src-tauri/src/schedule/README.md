# Schedule

Owner: durable due entries, firing decisions, holds, calendar synchronization, and dispatch/notification outcomes. Steward owns delegated-work authorization and execution.

Preserve:
- Due does not mean authorized. Entry delegation references are checked before action; task dispatch must still pass Steward gates.
- Keep Scheduled/Firing/Fired/Missed/Withdrawn/Superseded transitions explicit. Do not replay a terminal entry as new work.
- Act/Hold/Defer/Ask are different decisions. Situation holds and busy generation slots are not task failures.
- Entry revision, subject/scope, delegation, and external calendar identity must remain correlated through updates and cancellation.
- Calendar synchronization and reminder delivery do not prove a delegated task started or completed.

Locate:
- IPC/types: `commands.rs`, `contracts.rs`; durable states: `ledger.rs`, `schema.sql`.
- Due loop/dispatch: `tick.rs` -> `decide.rs`, `runtime.rs`; work execution: `../steward/dispatch.rs` (`dispatch_scheduled`).
- Holds/notifications: `hold.rs`, `notify.rs`; generation occupancy: `../memory/personal_state/scheduler.rs`.
- Calendar auth/client: `calendar/auth.rs`, `calendar/oauth.rs`, `calendar/client.rs`.
- Calendar reconciliation/projection: `calendar/reconcile.rs`, `calendar/observe.rs`, `calendar/projection.rs`.
- Forget/recovery: `forget.rs`; enabled/runtime handle: `handle.rs`.
- Tests: `tests.rs`, `tests.d/`, calendar tests.

Trace entry ID/revision -> decision -> fire result -> bound task or notification. Check restart, duplicate ticks, withdrawn delegation, situation hold, and calendar edits at the affected boundary.
