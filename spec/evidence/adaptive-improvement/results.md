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

2026-09-21 controlled Tool run:
`ai_09_synthetic_approved_artifact_reorders_the_real_tool_search_path` passed. It creates its
catalog, evaluation gate, and active artifact only in an in-memory database; normal Tool search
then selects and invokes `minutes` ahead of the rule-ranked `web`, recording both the adaptive
decision and its successful outcome. This verifies the selection/execution wiring, not
real-user improvement.

2026-09-21 mock-runtime run: the explicit model-free mock configuration parses only when
requested, enables Tool discovery, and seeds exactly three development-fixture catalog entries.
The real search/invoke integration test remains separate and uses the same isolated fixture
backend. No mock configuration was applied to the running app or to the user database.
The opt-in developer launcher is `bun run start:adaptive-fixture`; normal `bun run start` remains
unchanged. A `mode: "mock"` file alone is rejected by the live configuration loader: the launcher
must also provide the explicit fixture flag, a smoke marker, and an absolute isolated data
directory before any fixture data can be seeded. The database-path layer additionally requires an
existing private `saaa-adaptive-fixture.*` directory distinct from normal application data.
The configuration-isolation test, the fixture-directory test, and all four mock Tool tests
passed: the generic opener rejects seeded mock configuration; direct mode has no fixture rows;
mock mode trains from exactly the three synthetic outcomes; and the normal search →
execution-reference → invoke route selects `minutes` and records a successful outcome.

2026-09-21 isolated desktop-runtime run: `bun run start:adaptive-fixture` launched a development
app with its own temporary smoke data directory and reached its frontend-ready marker. Its
isolated SQLite ledger contained exactly 3 `adaptive-development-fixture` catalog rows, 3
corresponding user grants, an enabled fixture source, one active Tool artifact and activation,
and Tool adaptation enabled in its fixture-only Settings document. The artifact is trained from
three synthetic decision/outcome examples rather than from the normal database; the observed
scores were `minutes=1.0`, `web=0.0`, and `archive=0.0`. The process was then stopped; the normal
application database was not opened or changed.

2026-09-21 guarded fixture-runtime run: the launcher created
`saaa-adaptive-fixture.fpqnUq` as its temporary data directory and the live process populated
exactly 3 fixture catalog rows, 3 synthetic examples, one active Tool artifact, and one Tool
activation with the same `minutes=1.0` score. The normal application database still had zero
fixture catalog rows and zero fixture examples. The non-interactive verification stopped the
development process after the database check; it does not claim a second frontend-marker result.
The launcher now removes its temporary fixture data at exit by default; retaining it for
investigation requires `SAAA_ADAPTIVE_FIXTURE_KEEP_DATA=1`. A subsequent SIGINT-stopped fixture
run left no `saaa-adaptive-fixture.*` directory in its controlled `/tmp` data location.

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
