# Delegated work progress

| Card | Status | Evidence |
| --- | --- | --- |
| DW-00 | complete | `baseline.md` records the pre-existing trigger and lifecycle boundaries. |
| DW-01 | complete | `steward/contracts.rs` validates operations, verifier choices, and bounded proposals. |
| DW-02 | complete | Additive steward columns, origin bindings, reservations table, and 8-goal limit are migrated. |
| DW-03 | complete | `work_propose/status/amend/withdraw` are exposed through both the standard streamed and agent-session SSE coding bridges. The host derives the source message from the current persisted run, validates it, persists a proposal, and queues the fixed read/test recipe. |
| DW-04 | partial | Panel distinguishes goal state, scope/budget/verifier/notification, and supports per-goal withdrawal. Direct registration now requires an explicit read/test-only confirmation; editing operation/budget or richer confirmation flows remain pending. |
| DW-08 | complete | `steward_event_cursor` consumes committed Coding events in the runner transaction. |
| DW-10 | partial | Test-report versus tests-pass verifiers do not share a completion state. The selected bounded read/test recipe is now persisted per task for audit/restart; multi-step dependency and replan execution remain pending. |
| DW-11 | complete | Reports are durable and deduplicated; a UI refresh delivers them without a user turn and respects Situation hold. |
| DW-07 | blocked-by-runtime | `delegated-read-test-macos-v1` enforces workspace-write and network default deny with temporary/session output allowed. A real Pi `--version` canary currently terminates in the installed Pi/Node runtime's `LowLevelAlloc arithmetic overflow` under Seatbelt. The attempted minimal IPC/HOME workaround did not fix it and was not retained; a supported read/test adapter or Pi runtime correction is required. |
| DW-05 | partial | `coding_origin_bindings` distinguishes user turns from delegated events. Delegated starts use a task-scoped synthetic run key and are authorized/revoked through the active steward task. |
| DW-06 | partial | Dispatch intents and restart-to-unknown recovery are persisted. Both user-triggered and schedule-triggered delegated work use the common Coding service; crash receipt inspection coverage remains pending. |
| DW-09 | partial | Schedule TaskRun verifies durable `task:<id>` and delegation ids through the steward dispatcher, then records Started only after its receipt. Expiry wiring and full restart coverage remain pending. |
| DW-13 | partial | Withdraw/forget cancel derived tasks, release unconsumed reservation, request stop for a running Coding job before source deletion, and ignore late terminal events. Process-exit confirmation remains pending. |
| DW-12,14 | pending | Require notification/TTS delivery completion and a real-profile end-to-end acceptance environment. |

No user content, absolute workspace paths, or credentials are recorded here.
