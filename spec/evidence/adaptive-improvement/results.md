# Adaptive improvement results

The deterministic unit cases cover scope isolation, ineligible override fallback, rejection of
un-evaluated activation, active artifact dispatch selection, and silent-outcome materialization.

2026-09-21 deterministic run: 8 adaptive-improvement Rust tests passed in the isolated Cargo
target after compiling the app containing the background materializer and settings gate. The
frontend typecheck remains blocked by unrelated voice and IPC contract errors.
2026-09-21 delegated-work regression run: 25 Steward Rust tests passed after wiring finite Plan
recipe selection, terminal outcome recording, and Goal-scoped notification aggregation.
The outbox deadline invariant is separately covered by
`aggregate_report_is_not_flushed_before_its_delivery_deadline`.

Repository checks: IPC contract tests and `bun run spec:check` passed. `bun run size:check`
cannot pass in the shared dirty worktree because many unrelated modules are above their stored
ratchets or have no baseline. `bun run check:local` stops at pre-existing formatting issues in
`scripts/role-routing/codex-sidecar.ts`, `src/features/chat/useConversationTurn.ts`, and
`src/lib/roleRoutingApi.ts`; none are adaptive-improvement files.
The broader Role Routing receipt test target was also attempted, but the current worktree fails
first on an unrelated shadowed `world_free_history` name in `runtime/conversation_turn.rs`.
Full evaluation claims are not made here: no real-user data, paired evaluation pool, or
live-provider comparison was used.
