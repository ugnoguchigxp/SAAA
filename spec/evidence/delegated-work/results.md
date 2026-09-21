# Delegated work results

## Targeted checks

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
- `bun run ipc:check`: pass (runtime/UI/Coding/Schedule bindings and voice ASR).
- `bun run size:check`: fails on repository-wide pre-existing ratchet excesses
  and missing baselines (including files outside delegated work); no ratchet
  baseline was relaxed for this implementation.

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
- The macOS delegated-profile boundary test and profile-contract test pass;
  the current IPC contract suite also passes (4 general bindings and 1 voice
  ASR binding).
- The production Pi-adapter fixture cancellation test passed and observes the
  Coding job terminal state as `interrupted`, not merely `cancel_requested`.
