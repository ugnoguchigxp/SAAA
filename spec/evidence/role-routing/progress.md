# Role Routing 実装進捗

## E00〜E06 完了作業（2026-09-22）

基準 HEAD: 作業開始時 `7471ecadc1d64a5b3c1d4e6a898669be80a2cb8a`。作業中に別プロセスが
`e8c79608b138324e51839ef15bff5914f4146771`（`feat: complete delegated work and world delivery updates`）を
commit し、その commit には当時の E00〜E02 成果物が含まれた。E03 以降の変更は現時点で未 commit。
本作業は git の書き込み操作を行っていない。別プロセスの dirty 変更（`adapters/codex.rs` 等）は
上書き・取り込み・revert していない。
E00〜E06（P0 基準・契約と P1 台帳・採用境界）を実装し、下表の test を通した。
lane はすべて offline。live lane（L01〜L04）は未実行で、認証済みモデルを起動していない。

| 単位 | 状態 | 今回変更ファイル | test 名 / command | 結果 |
| --- | --- | --- | --- | --- |
| E00 | 完了 | `spec/evidence/role-routing/acceptance-matrix.md`（新規）、`baseline.md`、`progress.md` | 文書整合（RR40/A42/P5 欠落0・重複0） | pass |
| E01 | 完了 | `role_routing/contracts.rs` | `rr_01_unknown_field`, `rr_01_utf8_limit`, `rr_01_invalid_id` | 3 pass |
| E02 | 完了 | `role_routing/repository_turns.rs` | `rr_12_old_revision_result_rejected`, `rr_16_pending_input_blocks_real_acceptance`, `rr_12_db_failure_no_speech` | 3 pass |
| E03 | 完了 | `role_routing/schema.rs`, `role_routing/steps.rs`（新規）、`coordinator.rs`, `mod.rs` | `rr_02_start_claims_one_planned_step`, `rr_02_one_active_reasoning_step`, `rr_02_migrate_existing_partial_state`, `rr_05_duplicate_completion_once` | 4 pass |
| E04 | 完了 | `role_routing/steps.rs`, `repository_turns.rs` | `rr_12_intermediate_output_not_final`, `rr_12_finalize_once`, `rr_22_usage_is_saved_with_the_active_step` | 3 pass |
| E05 | 完了 | `role_routing/schema.rs`, `repository_turns.rs`, `coordinator.rs` | `rr_04_receipt_retry_and_conflict`, `rr_16_multiple_pending_inputs` | 2 pass |
| E06 | 完了 | `role_routing/repository_policy.rs`, `repository.rs`, `coordinator.rs`, `repository_turns.rs`, `recovery.rs` | `rr_03_policy_cas_conflict`, `rr_18_queued_policy_immutable`, `rr_22_deadline_starts_at_claim` | 3 pass |
| E07 | 部分（offline compiler 完了、executor 未接続） | `role_routing/recipe.rs`（新規）、`mod.rs` | `rr_06_recipe_invalid_dependency`, `rr_22_recipe_all_branches_bounded`, `rr_06_self_review_alias_rejected` | 3 pass |
| E08 | 部分（driver/registry 完了、AppState 接続待ち） | `role_routing/driver.rs`（新規）、`mod.rs` | `rr_05_one_actor_per_conversation`, `rr_05_io_does_not_block_input`, `rr_05_two_steps_run_in_order`（+ 既存 `rr_05_commit_before_dispatch`） | 4 pass |
| E09 | 部分（projection 検証完了、通常 turn 未接続） | `role_routing/context.rs`（新規）、`mod.rs` | `rr_07_amendment_present_once`, `rr_07_scope_no_widening`, `rr_07_revoked_source` | 3 pass |
| E11 | 部分（pure budget/resourceGroup 完了、dispatch 接続待ち） | `role_routing/limits.rs` | `rr_22_loop_budget`, `rr_09_shared_resource_group`（+ 既存 cost/deadline） | 4 pass |
| E10 | 未着手 | - | `rr_05_normal_turn_two_steps` | 未実装 |
| E12〜E37 | 未着手 | - | - | 未実装 |

command: `cargo test --locked --manifest-path src-tauri/Cargo.toml --lib role_routing::` = 111 passed / 0 failed（E09 追加後）。
command: `cargo test --locked --manifest-path src-tauri/Cargo.toml --lib runtime::` = 193 passed / 0 failed / 6 ignored。
full lib: 1322 passed / 1 failed / 23 ignored。唯一の failure は role-routing 外の既存 dirty 変更
`larm_voice::world_tests::wr_t22_http_route_transition_matrix`（`world_wire_fixture.rs` の
`Bearer token-llm` vs `Bearer token-v1`）で、本作業では触れていない。

E02 の採用境界: 採用は root phase=`responding`・cancel=0・result 側 step の revision 一致時のみ。
`draining`（入力 barrier / 停止要求）と terminal は拒否、DB 失敗は assistant message と同時に rollback。
E03/E04: `idx_rr_steps_one_active_reasoning` で root ごと active reasoning step を1つに制限し、
Start は `steps::claim_next_planned_step` で最小 ordinal の planned 1件だけ running にする。
完了は `steps::complete_step`（同一 terminal は no-op、別 terminal は conflict）、採用は
`steps::finalize_root`（1回だけ）に分離。usage/output は ordinal 0 固定を廃止し active step に保存。
E05: `rr_inputs.generation` を追加し、同 inputId 同 digest は duplicate、別 digest は conflict、
active root 入力は barrier と同一 transaction で保存。
E06: policy 取得を compare-and-swap 化。queued root の deadline は claim 時に開始。

## E07〜E25 追加作業（offline、継続セッション）

本セッションは live lane（L01〜L04）を一切起動していない。事前の dirty 変更（別プロセスが編集中の
`steward/*`、`role_routing/adapters/codex.rs`、`runtime/conversation_turn.rs`）は上書き・revert・取り込み
していない。`src/generative_ui/revisions.rs` にあった既存の borrow error（本作業外）は crate の build を
塞いでいたため最小修正した。

| 単位 | 状態 | 今回変更ファイル | test 名 | 結果 |
| --- | --- | --- | --- | --- |
| E07 | 部分（offline compiler 完了、実行接続は executor 経由） | `role_routing/recipe.rs`（新規）、`repository_turns.rs` | `rr_06_recipe_invalid_dependency`, `rr_22_recipe_all_branches_bounded`, `rr_06_self_review_alias_rejected` | pass |
| E08 | 部分（registry/driver 完了、AppState 常駐接続待ち） | `role_routing/driver.rs`（新規） | `rr_05_one_actor_per_conversation`, `rr_05_io_does_not_block_input`, `rr_05_two_steps_run_in_order` | pass |
| E09 | 部分（context projection 完了、通常 turn 未接続） | `role_routing/context.rs`（新規） | `rr_07_amendment_present_once`, `rr_07_scope_no_widening`, `rr_07_revoked_source` | pass |
| E10 | 部分（executor/permit/budget 完了、実 turn 未接続） | `role_routing/executor.rs`（新規） | `rr_22_loop_budget_at_the_executor`, `rr_22_deadline_at_the_executor`, `rr_05_queued_root_is_not_dispatchable_yet`, `rr_07_compile_reuses_the_recipe_plan` | pass |
| E11 | 部分（step budget を receipt へ接続） | `role_routing/limits.rs`, `runtime/conversation_inputs_roles.rs` | `rr_22_role_route_enforces_the_step_budget`, `rr_22_loop_budget`, `rr_09_shared_resource_group` | pass |
| E17 | 部分（host 検証を model verdict から分離） | `role_routing/review.rs`, `revision.rs`, `repository.rs` | `rr_24_model_verified_does_not_authorize_revision`, `rr_24_issue_shape_and_count_are_bounded`, `rr_25_unsupported_critique_is_preserved_but_cannot_revise` | pass |
| E20 | 部分（一度だけの消費と step 同時 commit） | `role_routing/proposals.rs`, `schema.rs` | `rr_26_receipt_requires_named_candidate_and_rechecks_revision`（consume 二重拒否を含む） | pass |
| E23 | 部分（generation/revision 束縛を追加） | `role_routing/classifier.rs` | `rr_08_late_classification_is_dropped`, `rr_08_mixed_greeting_is_not_a_valid_action`, `rr_08_timeout_unclear` | pass |
| E24 | 部分（話者・revision 束縛を追加、永続 repository 未接続） | `role_routing/speech_queue.rs` | `rr_13_speech_is_bound_to_speaker_and_revision`, `rr_13_final_before_ack_speaks_final_only` | pass |
| E25 | 部分（sourceId 重複受付拒否を追加） | `role_routing/repository_turns.rs` | `rr_14_asr_duplicate_receipt`, `rr_18_queue_order_and_restart_are_safe` | pass |

command: `cargo test --locked --manifest-path src-tauri/Cargo.toml --lib role_routing:: -- --test-threads=1` = **125 passed / 0 failed**。

E17 の要点: model の `verdict="verified"` は host 検証を経ない限り revision を許可しない。
`host_verify` は evidence ref が scope 内かつ未失効のときだけ `Verified` を返し、
`revision_allowed` は host-verified 件数のみを見る。issue は最大 8 件・claim 2000 bytes に制限。
E20 の要点: `consume_approval` は approved かつ未消費の proposal のみを対象に、snapshot と
step 作成を同一 ambient transaction で行う。二重消費・stale・期限切れ・cloud 剥奪は起動0。
E23/E24/E25 の要点: 分類結果は generation と revision の両一致時のみ適用。speech は話者必須・
final は root の新しい revision のみ。ASR の同 sourceId 再送は inputId が違っても duplicate を返す。

## 実装監査（2026-09-21、未完了）

このファイルは完了報告ではない。作業カードの合格条件に対しては、R1〜R3すべて未完了である。詳細なカード別の状態、根拠、残作業は[作業カード](../../docs/saaa-role-routing-work-cards.md#実装監査2026-09-21)を正本とする。

| カード | 実装 | 検証 |
| --- | --- | --- |
| RR-01 | 契約・reducer・signalsの断片を追加 | 指定event/Clock/ID注入・境界試験は未実装 |
| RR-02 | SQLiteへpolicy/root/input/decision/step/output/event/feedback/tool-link ledgerを追加 | migrationとcross-root tool-link FKの一部試験のみ。C6全制約は未検証 |
| RR-03 | `routing.roles/default` を無効既定で追加。保存時にimmutable policy snapshotを採番。queued rootはreceipt policyを優先してdispatchし、stepにpolicy/recipe/actor由来fingerprintを保存 | `bun test tests/settings-regressions.test.ts tests/settings-review.test.ts`（11 pass）。隔離test binary: `rr_03_queued_root_uses_its_immutable_policy_receipt`: 1 pass |
| RR-04〜06 | receipt transaction、queue上限拒否、provider route override、pure reducer/ranker断片。active root がある receipt は queued のまま provider 前で待機 | receipt retry/conflict、actor adapter/contextは未実装 |
| RR-07/08/09/10/12/13 | role dispatch時のcontext projection、follow-up分類、Provider開始の本文なしactivity Sink、pure tool permit、message transactionでのroot採用、speech queue断片 | delta activity・executor/dispatcher/speechへの全接続なし |
| RR-11 | tool operationのroot/step/revision linkを既存tool ownerの前後に記録 | tool-selectionとの縦通し・detach試験は未実施 |
| RR-14 | `get_routing_snapshot` とroot-local `replay_routing_events` のIPC、生成TS binding、chat表示hookを追加。永続cancel後eventと3秒snapshot pollingで更新 | 全eventのlive store、ASR receipt、legacy/duplicate試験は未実装 |
| RR-17 | tool receiptをtransaction内で確認し、`reserved`/`dispatched`/`unknown`の間はrevision再開を拒否 | input update/child drainの実行経路と全順序試験は未実装 |
| RR-18 | 起動時にin-flight rootをinterrupted evidenceとして永続化。queued receipt は provider 前で待機し、turn 終端で最古の開始可能 root を `IMMEDIATE` transaction で claim して実行を解放 | 再接続、restart 後 queued receipt の明示的再開 UX は未実装 |
| RR-27 | 永続root cancel IPC、snapshotを読むchat表示・停止操作を追加。receipt commit後に実行中run/TTSへcancelを通知 | amend/reconsider、live event購読、child drain/restartは未実装 |
| RR-28 | chatに永続snapshot由来のphase/revision/queue先頭を本文非表示で表示 | 実行履歴・選択理由UIは未実装 |
| RR-19 | 固定SDK版のJSONL sidecarとRust JSONL frame validatorを追加。frame/累積/最終結果の上限とcancelを強制。`codex_sdk` actor はconversation経路からsidecarへdispatchし、結果をroot採用transactionへ保存。actor input上限を超えるcurrent requestはdispatch前に拒否 | mock SDK wire試験、実認証SDK呼び出しは未実施 |
| RR-20 | sidecarをBun compiled resourceとして同梱し、Tauri resource pathから起動する Rust ProcessGuard adapter を追加。空の一時cwd、read-only、approval/network/web/MCP無効化、Codex loginに必要な最小環境だけを許可 | live isolation fixture は未実施 |
| RR-22 | root deadlineをreceiptへ永続化し、role provider route の総timeout/attempt timeoutへ反映。queued 待機中の期限切れはfailedとして次rootを解放。未知費用・上限超過を拒否する純粋budget判定も追加 | active dispatchのusage集計、切替上限の実行接続は未実装 |
| RR-30〜35 | learning schema、dirty queue、DB materialize、本文なしJSONL export、window判定、shadow artifact断片。60秒writer tickからrole-routing materializeを起動 | group split、job状態、foreground/shutdown pauseは未実装 |
| RR-36 | hash・feature版・candidate fingerprintを検証するlinear-v1 artifact loaderと、観測labelだけを使うoffline評価を追加 | selectionへのshadow接続、昇格証跡は未実装 |
| RR-37 | forget sourceからinput root/feedback receiptを逆引きし、example削除・dataset/artifact失効を同一writer transactionで実行。設定画面で本文なし件数表示と手動materialize | filesystem journalは未実装 |

検証実績:

- `bun test ./tests/settings-regressions.test.ts ./tests/settings-review.test.ts`: 11 pass
- `cargo check --manifest-path src-tauri/Cargo.toml --lib`: pass（role routingの未接続warningあり）
- 隔離targetで `cargo check --lib`: pass（警告のみ）
- 隔離test binary: `rr_02_tool_link_cannot_reference_a_step_from_another_root`、`rr_11_duplicate_operation_keeps_the_original_reservation`、`rr_11_unknown_no_retry`、`rr_13_final_before_ack_speaks_final_only`、`rr_13_ack_stop_timeout_leaves_one_active_speech`: 各1 pass
- 隔離test binary: `rr_14_snapshot_and_replay_preserve_queue_order`: 1 pass
- `bun run typecheck`: pass（cancel update eventのchat subscriptionを含む）
- 隔離test binary: `rr_27_cancel_is_durable_and_replayable`: 1 pass
- 隔離test binary: `rr_04_queue_full_leaves_no_input_message`: 1 pass
- 隔離test binary: `rr_04_receipt_rows_rollback_with_the_runtime_transaction`: 1 pass（queued root / planned step / root deadline のreceipt不変条件を含む）
- 隔離test binary: `rr_17_tool_unknown_blocks_restart`、`rr_17_settled_tool_allows_next_revision`: 各1 pass
- 隔離test binary: `rr_18_queue_order_and_restart_are_safe`: 1 pass
- 隔離test binary: `rr_18_claims_fifo_only_after_the_prior_root_is_terminal`、`rr_18_claim_skips_a_conversation_with_active_work`: 各1 pass
- 隔離test binary: `rr_05_commit_before_dispatch`、`rr_16_barrier_is_durable_before_resume`: 2 pass（queued step の `planned → running` 遷移を含む）
- 隔離test binary: `role_routing::`: 53 pass
- 隔離test binary: `rr_`: 67 pass（2026-09-21、RR-20 fake sidecarのtimeoutを10秒へ安定化後）
- 隔離test binary: `rr_33_partial_file_not_ready_and_export_is_deterministic`: 1 pass
- 隔離test binary: `rr_36_invalid_hash_falls_back_to_rules`、`rr_36_new_model_cold_start_and_score_are_safe`: 各1 pass
- 隔離test binary: `rr_36_no_counterfactual_labels`: 1 pass
- 隔離test binary: `rr_37_forget_invalidates_before_dispatch`、`rr_37_forget_feedback_source_invalidates_its_target_dataset`: 各1 pass
- 隔離test binary: `rr_37_learning_snapshot_contains_counts_only`: 1 pass
- 隔離test binary: `role_routing::learning::`: 10 pass
- 隔離test binary: `rr_19_frame_validation_rejects_unknown_ids_and_duplicate_terminals`、`rr_19_frame_validation_rejects_extra_fields_and_empty_results`、`rr_20_process_requires_a_terminal_frame`、`rr_20_process_accepts_only_valid_result_protocol`、`rr_20_cancel_sends_protocol_cancel_then_reaps_child`、`rr_20_config_isolation_clears_parent_environment_and_arguments`: 6 pass
- 隔離test binary: `codex_actor_selects_the_isolated_dispatch_without_rewriting_provider_settings`、`rr_19_codex_prompt_keeps_current_request_and_bounds_history`、`rr_19_codex_prompt_rejects_an_unfit_current_request`: 各1 pass
- `bun --check scripts/role-routing/codex-sidecar.ts`: pass。version不一致・非JSON frameがstdoutを出さず拒否されることを確認。
- `printf '{not-json}\\n' | src-tauri/resources/bin/role-routing-codex`: pass（同梱 sidecar が外部認証・実モデル呼び出しなしで起動し、不正 frame を stdout に出さず終了）
- 隔離test binary: `rr_22_unknown_cost_is_not_free_when_policy_has_a_budget`、`rr_22_deadline_rejects_new_dispatch`: 各1 pass
- 隔離test binary: `enabled_direct_recipe_overrides_the_legacy_conversation_route`: 1 pass（role root/step timeout が provider route に反映されることを含む）
- `bun run typecheck`: pass
- `bun run desktop:smoke -- --report-dir spec/evidence/role-routing/desktop-smoke-20260921-role-routing`: pass（build / bundle / launch / IPC ready）
- `bun run desktop:smoke -- --report-dir spec/evidence/role-routing/desktop-smoke-20260921-codex-actor`: pass（Codex actor dispatch 経路を含む build / bundle / launch / IPC ready）
