# Delegated work results

## Targeted checks

- `bun run typecheck`: pass.
- `bun test tests/coding-steward.test.ts`: 2 passed, 0 failed.
- `bun test tests/steward-panel.test.tsx`: 1 passed, 0 failed.
- `cargo test dw_01_direct_registration_allows_eight_goals_then_enforces_the_limit --lib`: pass.
- `cargo test dw_14_multiple_goals_keep_the_sibling_through_topic_switch_withdrawal_and_hold --lib`: pass.
- `cargo test ml_08_acceptance_register_divert_complete_withdraw --lib`: pass.
- `cargo test coding_service_runs_through_a_delegated_event_origin --lib`: pass.
- `cargo test dw_06_restart_marks_unreceived_dispatch_unknown_without_reclaiming --lib`: pass.
- `cargo test dw_10_settled_read_step_durably_enqueues_one_dependent_test_step --lib`: pass.
- `cargo test dw_10_migration_backfills_a_plan_for_an_existing_goal --lib`: pass.
- `cargo test dw_10_failure_creates_at_most_two_durable_replans --lib`: pass.
- `delegated_sdk_profile_completes_a_read_only_prompt` (authenticated local
  acceptance): pass.
- `bun test ./tests/pi-codex-sdk.test.ts`: 3 passed, 0 failed.
- `cargo test speech_callbacks_have_durable_terminal_mappings --lib`: pass.
- `cargo test steward::tests --lib`: 30 passed, 0 failed.
- `cargo test schedule::tests --lib`: 18 passed, 0 failed.
- Packaged desktop smoke: pass (build 27.95s, bundle, launch, IPC ready,
  cleanup; 31.21s total on the current worktree).
- Rust formatting check for changed steward and runner sources: pass.
- Targeted steward test binary: 27 passed, 0 failed; the agent-session
  delegated-work bridge regression test also passes. It initially exposed
  duplicate task creation from repeated triggers; the dedupe key was corrected
  to delegation scope and the complete targeted rerun passed.
- Isolated-target `cargo check --lib`: pass (warnings only). Full application
  gates remain pending.
- `bun run spec:check`: pass.
- `bun run typecheck`: pass after regenerating the IPC bindings and aligning
  the meeting-blocked voice-policy union.
- `bun run ipc:check`: pass on the current worktree (4 general bindings and 1
  voice-ASR binding). A transient unrelated `role_codex_prompt` arity mismatch
  in concurrent work was resolved without changing any delegated-work source.
- `bun run size:check`: fails on repository-wide pre-existing ratchet excesses
  and missing baselines (including files outside delegated work); no ratchet
  baseline was relaxed for this implementation.
- `bun run check:local`: currently stops at TypeScript formatting in shared
  files outside this work (`CodingConnectionFields.tsx` and
  `RoleRoutingSection.tsx`). The touched `StewardPanel.tsx` was formatted;
  `bun run typecheck` passes.

## Behavioural result

- A settled Coding job can produce a durable steward report through the event
  cursor without waiting for another conversation turn.
- A `tests_pass` verifier remains `awaiting_user` after a merely settled job;
  only `test_report_obtained` may become `done` from that event.
- Terminal event replay is harmless because the cursor and report uniqueness
  constraint prevent repeated delivery.
- A streamed `work_propose` is bound to the persisted source message for its
  current user turn; model-provided arguments cannot select a different source.
- The `delegated-read-test-macos-v1` process profile was exercised directly:
  workspace writes were denied while temporary-output writes were allowed.
- Steward source formatting and its latest targeted binary run pass.
- Schedule `TaskRun` records `Started` only after the durable steward
  dispatcher receives a Coding receipt; an absent task/delegation remains
  `NoDelegation`.
- A terminal-report replay for the same task revision keeps one outbox row and
  records the durable conversation-message identifier after delivery.
- The fixed-trigger path persists exactly one bounded task-plan recipe.
- Forgetting a source requests `stopping` for its running Coding job before the
  source is deleted, while late terminal events remain unable to revive work.
- The installed Pi 0.86.1 runtime was invoked beneath the macOS
  deny-by-default profile for a real canary. Node 24 requires only
  `sysctl-read` during allocator initialization; with that read-only rule,
  Pi starts and answers `get_state`. Workspace write and network permissions
  remain absent. A real Codex SDK turn additionally requires home state-DB and
  app-server writes, so that combination is rejected rather than weakening the
  profile; an authenticated restricted read/test turn has not completed.
- The pinned Pi 0.86.1 interface canary passed against its isolated scripted
  provider: session resume, result collection, model-error handling, abort,
  and orderly shutdown all completed. No live model was used for that canary.
- The macOS delegated-profile boundary test and profile-contract test pass.
- The authenticated SDK startup probe passed under
  `delegated-codex-sdk-macos-v1`: Pi loaded the SDK provider and completed
  its RPC state handshake while `CODEX_HOME` pointed at isolated writable
  state. No model prompt was sent by that probe.
- The production Pi-adapter fixture cancellation test passed and observes the
  Coding job terminal state as `interrupted`, not merely `cancel_requested`.
- The production Pi-adapter forget integration passed: deleting an accepted
  source causes the runner to abort the fixture process, and both the current
  run and its job become `interrupted`. The authorization-aware inspection API
  then correctly rejects the forgotten lineage.
- The Steward panel now exposes the already host-validated, per-Goal
  notification-only amendment. Broader operation or budget changes remain a
  new confirmed registration, preserving the explicit authority boundary.
- The frontend Steward API test confirms a notification amendment sends only
  the selected Goal id and route to `work_amend`.
- Direct UI registration and model proposals now share the same eight-active-Goal
  limit. The ninth direct registration is rejected as `active_goal_limit`; the
  panel explains that limit without telling the user that a single active Goal
  is required.
- A browser-independent panel acceptance renders two active Goals together and
  verifies that withdrawing B calls `work_withdraw` with B's Goal id only.
- A combined steward acceptance keeps B queued after A is withdrawn, does not
  start work for an unrelated topic, holds the resulting report during a
  meeting, and flushes one report when the hold clears.
- A steward restart-boundary test passes: an unreceived `dispatching` intent
  becomes `outcome_unknown` during startup migration, cannot be reclaimed,
  and creates no Coding job by replay.
- All 18 targeted Schedule tests pass, including expiry before TaskRun dispatch
  and crash recovery that closes `firing` work without rerunning it.
- The production Pi fixture now verifies the shared Coding service's delegated
  origin path end-to-end: it settles normally, persists `delegated_event` with
  the steward task id, and does not fabricate a user turn during dispatch.
- The packaged desktop smoke passed on the current worktree: build, bundle,
  launch, IPC-ready observation, and cleanup all completed.
- Speech-delivery claims are durable and restart-safe: a claimed report becomes
  `delivery_unknown` on startup and cannot be replayed automatically. Speech
  starts only after the conversation report is committed and only when the
  existing Situation and global auto-speak policies permit it.
- The current macOS host completed a system-speech invocation and an `afplay`
  invocation against an existing system sound. This establishes that the OS
  synthesis and player processes used by the selected System TTS route can run;
  it does not replace an app-driven audible-delivery acceptance.
- The task-list IPC now returns the latest durable report-delivery and speech
  state; the Steward panel renders those values after each refresh/reconnect.
  Its notification amendment button also sends the user-selected route.
- A settled read step for a `read_test` delegation atomically creates one
  queued test step with its own dispatch intent. Replaying the same terminal
  event leaves the task count unchanged.
- New and migrated Goals persist a bounded Goal plan and dependency rows;
  the existing `TaskPlan` validator rejects invalid or cyclic plans before
  those rows are written.
- The persistent plan executor selects only dependency-ready steps. A failed
  step creates at most two new plan revisions; replaying the same terminal
  event cannot create another revision.
- A terminal task records its Coding job as a durable artifact reference. The
  task-list IPC returns those references and the Steward panel displays them.
- The live delegated SDK profile now separates the trusted Pi adapter from the
  SDK's model-tool sandbox. An authenticated turn read an isolated Git workspace
  README and recovered `read-only fixture` from the durable session. The same
  profile rejected a model-generated workspace write and public-network request.
  With `sandbox_permissions: []`, it also rejected a marker outside the workspace
  before tool execution. Mutable SDK state remains below the Git-ignored `.saaa`
  state area; authentication is linked rather than copied.
