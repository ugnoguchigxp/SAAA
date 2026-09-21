# Adaptive improvement results

The deterministic unit cases cover scope isolation, ineligible override fallback, rejection of
un-evaluated activation, active artifact dispatch selection, silent-outcome materialization,
aggregate training without invented negative labels, and a return from an active artifact to
fixed rules.

2026-09-21 deterministic run: 11 adaptive-improvement Rust tests passed after compiling the
app test harness. The companion Provider integration test confirmed its dispatch decision records
a successful terminal outcome. `bun run typecheck` passed. The generated IPC contract tests also
passed (4 runtime bindings and 1 voice-ASR binding), including the adaptive artifact status and
rollback contract. The Settings projection for a pending artifact separately passed its Rust unit
test.
2026-09-21 delegated-work regression run: 25 Steward Rust tests passed after wiring finite Plan
recipe selection, terminal outcome recording, and Goal-scoped notification aggregation.
The outbox deadline invariant is separately covered by
`aggregate_report_is_not_flushed_before_its_delivery_deadline`.

Repository checks: IPC contract tests and `bun run spec:check` passed. `bun run size:check`
cannot pass in the shared dirty worktree because many unrelated modules are above their stored
ratchets or have no baseline. `bun run check:local` stops at pre-existing formatting issues in
`scripts/role-routing/codex-sidecar.ts`, `src/features/chat/useConversationTurn.ts`, and
`src/lib/roleRoutingApi.ts`; none are adaptive-improvement files.
The full Rust test target remains sensitive to concurrent worktree changes; this work was
independently checked with `cargo check --lib` after the Provider, Tool, and notification outcome
writes were added.
Full evaluation claims are not made here: no real-user data, paired evaluation pool, or
live-provider comparison was used.
