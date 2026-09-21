# Delegated work progress

| Card | Status | Evidence |
| --- | --- | --- |
| DW-00 | complete | `baseline.md` records the pre-existing trigger and lifecycle boundaries. |
| DW-01 | complete | `steward/contracts.rs` validates operations, verifier choices, and bounded proposals. |
| DW-02 | complete | Additive steward columns, origin bindings, reservations table, and 8-goal limit are migrated. |
| DW-03 | complete | `work_propose/status/amend/withdraw` are exposed through both the standard streamed and agent-session SSE coding bridges. The host derives the source message from the current persisted run, validates it, persists a proposal, and queues the fixed read/test recipe. |
| DW-04 | partial | Panel distinguishes goal state, scope/budget/verifier/notification, and supports per-goal withdrawal. Direct registration now requires an explicit confirmation over the user-selected summary, read/test operation, bounded run/time budget, verifier, and notification route; the host revalidates and persists precisely that scope. Existing active Goal amendments are still notification-only, so scope/budget changes require a new confirmed proposal. |
| DW-08 | complete | `steward_event_cursor` consumes committed Coding events in the runner transaction. |
| DW-10 | partial | Test-report versus tests-pass verifiers do not share a completion state. The selected bounded read/test recipe is now persisted per task for audit/restart; multi-step dependency and replan execution remain pending. |
| DW-11 | complete | Reports are durable and deduplicated; a UI refresh delivers them without a user turn and respects Situation hold. |
| DW-07 | blocked-by-runtime | `delegated-read-test-macos-v1` enforces workspace-write and network default deny with temporary/session output allowed. Node 24 required the read-only Seatbelt permission `sysctl-read` during allocator initialization; after adding only that permission, the installed Pi 0.86.1 starts and answers an RPC state request. A real Codex SDK turn still needs its home state DB and in-process app-server writes; granting those would violate the profile, so the SDK combination is deliberately unsupported and not exposed in the UI. A supported authenticated read/test adapter remains required. |
| DW-05 | partial | `coding_origin_bindings` distinguishes user turns from delegated events. Delegated starts use a task-scoped synthetic run key and are authorized/revoked through the active steward task. |
| DW-06 | partial | Dispatch intents and restart-to-unknown recovery are persisted. Both user-triggered and schedule-triggered delegated work use the common Coding service; crash receipt inspection coverage remains pending. |
| DW-09 | partial | Schedule TaskRun verifies durable `task:<id>` and delegation ids through the steward dispatcher, then records Started only after its receipt. Expiry wiring and full restart coverage remain pending. |
| DW-13 | partial | Withdraw/forget cancel derived tasks, release unconsumed reservation, request stop for a running Coding job before source deletion, and ignore late terminal events. The production-adapter fixture confirms a cancellation reaches an `interrupted` process/job terminal state; a direct forget-to-exit integration remains pending. |
| DW-12,14 | pending | Require notification/TTS delivery completion and a real-profile end-to-end acceptance environment. |

No user content, absolute workspace paths, or credentials are recorded here.
