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

## 実装・コード監査追補（2026-09-21、offline）

結論は引き続き未完了。特に E10 の実 multi-step turn 接続、E18、E21/E22、E24 の永続 speech、
E26/E27 の E2E、E35/E37 の受入・報告、および live lane L01〜L04 は完了していない。
live model / 認証済み SDK は費用と外部副作用を伴うため、この監査では起動していない。

今回のコード監査では、既存実装の次の不変条件を修正・回帰試験化した。

- dispatch は receipt に保存済みの `selected_id` を使い、実行直前に同じ actor/revision/deadline/budget を再検証する。
- 実行中 step を二重計上しない。未開始 cancel step は予算を消費せず、費用上限下の cloud unknown cost は拒否する。
- `draining` 中は provider I/O を開始しない。
- step/tool/proposal/root の条件付き更新が0件なら成功扱いせず、同一 terminal の真の再送だけを冪等とする。
- provider finish は step と root を同一 transaction で確定し、step不在・revision不一致・message不在を拒否する。
- operation key は元の step/revision を越えて再利用できない。
- tool/review/feedback/revision-decision の再送は、元receiptと完全に整合する場合だけ冪等とし、異なるpayloadを拒否する。
- learning materialize は dirty page ごとに一意な dataset/job を作り、0件更新や `batch_size=0` を拒否する。
- role routing 有効時の `/capability` 早期応答も routing root を同一 transaction で完了させる。

検証実績:

- `cargo test --locked --manifest-path src-tauri/Cargo.toml --lib role_routing:: -- --test-threads=1`: **130 pass / 0 fail**
- `cargo test --locked --manifest-path src-tauri/Cargo.toml --lib runtime:: -- --test-threads=1`: **197 pass / 0 fail / 6 ignored**
- `bun test ./tests/role-routing-codex.test.ts`: **2 pass / 0 fail**
- `bun run ipc:check`: **6 pass / 0 fail**
- `bun run typecheck`: pass
- `bun run lint`: pass
- 今回変更した role-routing Rust ファイルの `rustfmt --check`: pass。全crateの `cargo fmt --check` は並行変更中の `steward/dispatch.rs` と `steward/intake.rs` のみで fail（本作業では未変更）。
- `git diff --check`: pass
- `cargo clippy --manifest-path src-tauri/Cargo.toml --lib`: pass（未接続 planned module を含む既存 warning あり）
- `bun run size:check`: fail。role routing 内外の多数の既存未登録/超過ファイルがあり、baseline 緩和や自動登録は行っていない。

## E10 完了追補（2026-09-21、offline）

E10 の通常 turn / Provider / 内部 Sink 接続を完了した。`frontend → reasoner` の保存済み plan を
通常の `execute_turn` 入口から順次 dispatch し、中間 provider の delta は process 内 Sink にのみ保持する。
中間 step は本文ではなく SHA-256 と byte 数だけを ledger に記録し、最終 step の採用 transaction が
成功した後に限り assistant message と完了通知を公開する。partial stream / timeout / disconnect は旧経路や
後続 actor へ自動再実行せず、active step と未開始 step を同一 revision 上で終端化する。

追加検証:

- `rr_05_normal_turn_two_steps_commits_only_the_final_answer`: pass。frontend/reasoner 各1回、assistant 1件、accepted output 1件、frontend delta 0件。
- `rr_09_partial_role_step_leaks_no_draft_and_cancels_remaining_work`: pass。assistant 0件、delta 0件、後続 provider 起動0、step は `failed/interrupted`。
- `rr_09_child_delta_is_buffered_and_activity_is_bodyless`: pass。
- `rr_09_child_cannot_emit_completion_or_speech`: pass。
- `rr_05_frontend_ack_then_reasoner_compiles_as_two_bounded_steps`: pass。
- role-routing suite（E10 完了時点）: **132 pass / 0 fail**。

E11 は継続中。dispatch 直前の provider/SDK enabled・health・model・location 再検査と、provider actor の
`maxInputBytes` を実送信前に強制する実装を追加した。`rr_22_cloud_revoked_before_dispatch` は pass（receipt
後の SDK 剥奪で provider session 0件）。E10 を含む `rr_0` 抽出 47 test も 0 fail。

## E12 完了追補（2026-09-21、offline）

restricted MCP session は初期化時に host DB から `rootId / stepId / revision / startedAt /
configFingerprint` を取得して immutable に保持する。各 tool call の authorize と operation reserve は
この完全な束縛を同じ DB 条件で再検査し、現在 step の root-only 推定を routing session から除去した。
step/revision/attempt/fingerprint が変わった旧 session は、新 step の tool link を作成できない。

- `rr_21_old_session_cannot_use_new_step`: pass（旧束縛から link 0件）。
- `rr_21_missing_step_denied`: pass。
- `tool_selection::mcp_server::`: **41 pass / 0 fail**（generic 非routing session の回帰を含む）。
- `cargo check --locked --manifest-path src-tauri/Cargo.toml --lib`: pass。

## E13 完了追補（2026-09-21、offline）

Provider / SDK MCP / specialist が通る共通 gateway に、step束縛・root取消・revision・role・解決後 effect・
root累積 tool budget の二段検査を接続した。`tools_invoke` は署名済み execution ref を host service で解決し、
catalog の trusted effect を reviewer permit に使う。tool budget と operation link は同一 transaction で検査・
予約され、上限/取消/古いstepの場合は owner invocation を開始しない。

- `rr_10_reviewer_resolved_mutation_denied_at_gateway`: pass。
- `rr_21_tool_budget`: pass（予算0で link 0件）。
- `rr_10_update_between_reserve_and_invoke`: pass（取消後 link 0件）。
- gateway E12/E13 抽出: **5 pass / 0 fail**。

## E14 完了追補（2026-09-21、offline）

tool operation key を canonical JSON digest に変更し、同じ意味のpayloadでキー順だけが違う再送を同一操作として
扱う。link ID は root を含み、別 root の同payloadは独立する。owner 終了後の routing settle failure は
`routing-settle-failed` として呼出元へ返し、`dispatched` receipt を成功扱いして continuation しない。
loopback MCP の client timeout fixture で、HTTP handler detach後も management taskが owner と routing linkを
最後まで settleすることを確認した。

- `rr_11_same_payload_different_roots`: pass。
- `rr_11_duplicate_operation_once_uses_canonical_payload`: pass。
- `rr_11_detached_owner_settles`: pass。
- `rr_11_settle_failure_blocks_continuation`: pass。

## E15 完了追補（2026-09-21、offline）

Codex sidecar の bridge token は、gateway が明示された場合だけ SDK 子プロセス環境へ渡し、JSONL、
SDK config、診断本文には含めない。sidecar 入力は用途別の完全な key 集合、byte 上限、timeout、loopback
gateway を検査し、出力は host 側でも step/id/terminal/固定 error code と JSON Schema を再検証する。
任意の SDK 例外文は `sdk_error` に閉じ、schema や protocol 違反時に本文を保存しない。

- `rr_21_sdk_child_receives_token_without_recording`: pass。
- `rr_19_wrong_step_terminal`: pass。
- `rr_19_host_validates_the_declared_output_schema`: pass。
- `rr_19_failure_codes_are_closed_and_do_not_accept_exception_text`: pass。
- Codex adapter/protocol 抽出: **12 pass / 0 fail**。
- `bun test ./tests/role-routing-codex.test.ts`: **2 pass / 0 fail**。
- `bun run typecheck`、`cargo check --locked --manifest-path src-tauri/Cargo.toml --lib`、`git diff --check`: pass。

実認証 SDK の隔離確認は費用・外部呼出しを伴うため、計画どおり L01 に残す。

## E16〜E18 追補（2026-09-21、offline）

E16 は SDK adapter から最終 message の保存責務を外し、host が固定した
`stepId / revision / configFingerprint / purpose` と完全一致する dispatch だけを sidecar へ渡す。
SDK は候補と usage だけを返し、採用・中間保存・最終公開は通常 Provider と同じ host transaction が行う。
revision または model fingerprint が変わる dispatch は別 sidecar process/thread になり、古い束縛を再利用しない。
mock SDK→実 gateway→tool roundtrip（A30）はまだ未実装のため、E16 は部分完了のままとする。

- `rr_21_changed_revision_new_thread`: pass。
- `rr_21_changed_model_new_thread`: pass。
- bound Codex dispatch / output schema / terminal protocol 抽出: pass。

E17 は review target、author step、evidence scope と host verifier を分離した。実在するだけの ref や
model の `verdict="verified"` では revision を許可せず、payload に
`hostVerification="verified"` と非空の `verifierVersion` を持つ evidence だけを verified issue とする。
review JSON は未知 field、8件超過、過大 claim、別 revision・失効・scope 外 ref を拒否する。

E18 は通常 `respond` recipe の `author → independent reviewer → author` を
`respond → review → revise` step として実行する。draft と review は process-local 候補として次 step にだけ渡し、
review decision の消費と revise claim を同一 transaction にした。verified issue が0件なら review 済み draft を
1件だけ採用し、verified issue があれば host が整形した decision のみを reviser へ渡す。review/revise の中間本文・
completion・speech は公開しない。

- `cargo test --locked --manifest-path src-tauri/Cargo.toml --lib rr_24_ -- --test-threads=1`: **10 pass / 0 fail**。
- `cargo test --locked --manifest-path src-tauri/Cargo.toml --lib rr_25_ -- --test-threads=1`: **8 pass / 0 fail**。
- `rr_25_normal_turn_author_review_revise`: author 2回、reviewer 1回、最終 assistant 1件、3 step succeeded。
- `rr_25_decision_consumed_once_and_claims_revise_atomically`: pass。
- `rr_25_review_round_limit_requires_verified_issue`: pass。

以上により E17/E18 の offline 完了条件は満たした。live model の品質確認は L01/L03 に残す。

## E19・E28〜E34 追補（2026-09-21、offline）

E19 は回答への反応を同会話の最新 role-routing assistant message にだけ束縛した。challenge は旧rootを
再開せず新rootとして処理し、feedback receipt の `target_root_id` だけが旧回答を参照する。別conversationの
answer ID、引用内の否定、曖昧な対象はfeedbackにしない。明示positive/negativeとchallengeは別kindで保存する。

- `cargo test --locked --manifest-path src-tauri/Cargo.toml --lib rr_23_ -- --test-threads=1`: **6 pass / 0 fail**。
- `rr_23_challenge_starts_new_root_without_reopening_completed_root`: pass。
- `rr_23_wrong_conversation_target_is_rejected`: pass（不正receipt 0件）。

E28〜E31 では、decision時点のfeaturesをimmutable datasetへコピーし、後日のfeedbackは新datasetのlabelだけへ
反映するようにした。明示positive/negativeだけを二値labelとし、沈黙、challenge、cancel、矛盾feedbackは
unknown/excludedとして理由を残す。dirty page、dataset、job、cursorは同一transactionで確定し、途中失敗時は
すべてrollbackする。exportはconversation lineage由来の決定的group splitを含み、本文fieldを拒否し、JSONL後に
manifestをatomic publishする。

- `rr_30_feature_snapshot_immutable_and_rr_31_explicit_feedback_next_day`: pass。
- `rr_31_crash_before_checkpoint_keeps_the_page_retryable`: pass。
- `rr_32_silence_and_challenge_are_not_success_or_failure`: pass。
- `rr_32_cancel_not_failure_and_conflict_is_excluded`: pass。
- `rr_33_group_split_keeps_one_conversation_in_one_partition`: pass。
- `rr_33_partial_file_not_ready_and_export_is_deterministic`: pass。

E32 は local day単位の一意なrun receiptを追加し、通常window、missed window、clock rollback、foreground中断を
判定する。複数pageは各page transaction後に`paused`へ戻し、次tickでforeground/idle gateを再検査してから
再開する。E33 はeligible 20件未満のartifact生成を拒否し、hash/feature/candidate fingerprint不一致をrulesへ
fail closedする。shadowは通常turnのreceipt transactionで観測だけを保存し、dispatch候補を変更せず、追加の
adapter呼出しも行わない。E34 はsource失効と同時にdataset/artifactを利用不可にし、
本文・pathを持たないcleanup journalを作成、workerで固定export名の削除を再試行する。設定UIは失効datasetと
cleanup待ち件数を表示する。詳細な実行理由UIは未完了のためE34は部分。

- `rr_34_missed_night_runs_once_and_clock_rollback_does_not_repeat`: pass。
- `rr_34_foreground_preempts_before_the_next_page`: pass。
- `rr_35_small_sample_rules`: pass。
- `rr_35_shadow_no_second_call_on_the_real_turn_path`: pass（Provider 1回、shadow receipt 1件）。
- `rr_37_cleanup_retry_keeps_artifact_invalidated`: pass。
- `cargo check --locked --manifest-path src-tauri/Cargo.toml --lib`: pass（既存未接続warningのみ）。

## E16・E21・E24・E34 追加追補（2026-09-21、offline）

E16/A30 のoffline側は、認証付きloopback gatewayへrole-scoped sessionを張り、catalog解決、permit、
tool owner、routing link settle、SDK候補を模したhost採用までを一つのfixtureで通した。認証済みSol本体は
外部呼出しとなるためL01に残す。

E21 はproposalをrouting snapshotへ追加し、生成binding経由のIPCとchatの明示的な候補承認/辞退UIを接続した。
proposal IDとcandidate IDを必須にし、単独の肯定入力は承認にしない。reload後もapproved/consumed/費用不明を
DBから復元する。proposal生成と承認後executor wakeは未接続のため、E20/E21は引き続き部分。

E24 はaccepted final output、speaker、root revisionへ束縛した永続speech intent/epochを既存TurnEventHubへ接続した。
新しいfinal intentは同会話の旧queued/playing intentを取消し、再生直前に最新epochと全体の単一playing制約を
再検査する。重複completionは画面イベントを保持する一方、final音声を再投入しない。SpeechStarted/Ended/
Failed/CancelをDB lifecycleへ反映し、音声ledger失敗時も画面回答は保持する。

E34 のcleanupは本文やpathをDBに保存せず固定名だけを削除する。コードレビューでSQLite writer lock中に
filesystem I/Oをしていた点を検出し、本番workerはpending取得→lock外削除→結果receiptの二相へ修正した。

- `cargo test --lib role_routing:: -- --test-threads=1`: **157 pass / 0 fail**。
- `rr_21_sol_tool_roundtrip`: pass（未settle link 0）。
- `rr_29_old_speech_end_new_owner`: pass（旧SpeechEnded後も新epochだけがplaying）。
- `rr_26_proposal_snapshot_survives_reload_without_losing_consumption`: pass。
- `bun run ipc:generate`、`bun run typecheck`: pass。

E23/E26 の実入力経路では、従来のactive root存在時の一律`busy`拒否を廃止した。status/明示feedback以外の
追加入力は、旧rootの`input_barrier`と新rootのFIFO receiptを同一transactionで確定する。旧provider候補は
barrier後に採用できず、新rootのtaskは旧root終端後のdurable claimを待ってからdispatchする。明示cancelは
DB cancelを先にcommitし、該当するprocess-local run/TTSも停止する。新しい入力は同会話の旧speech intentを
取消してからTTS runtimeを停止するため、completed rootの遅い音声もbarge-in後に再生されない。

- `rr_29_provider_before_classifier_raises_durable_barrier_and_queues_follow_up`: pass。
- `cargo test --lib`（この時点）: **1476 pass / 6 fail / 27 ignored**。6 failはrole-routing外
  （`runtime::agent_tools` 1、並行変更中の`steward` 4、固定Bun版不一致`wasm_host_poc` 1）。最新結果は後続追補を参照。
- `bun run size:check`: fail。role-routing着手前からの未登録/ratchet超過が多数あり、baselineは緩和していない。

## E20・E21・E25・E36 追加追補（2026-09-22、offline）

E20/E21 は、独立reviewの未解決issueが残り、premium actorが設定され、費用上限下で価格不明にならない場合に限り、
通常review transactionからproposalを生成するよう接続した。通常turnはproposalの明示承認/辞退/期限切れを待ち、
承認時にpolicy/revision/candidate/期限と現在のCodexまたはprovider availabilityを再検査する。承認の消費、premium
step作成、claimは一つのtransactionで行い、元response recipe外のstepは消費済みproposalと一致する場合だけ
bound executorへ渡す。辞退、期限切れ、能力失効、二重承認ではpremium起動0でreview済みdraftを維持する。

E25 はlive購読を初回snapshotより先に確立し、active/queued rootとlive eventのroot hintをroot-local replayへ
渡す。同期処理を直列化し、`(rootId,eventSeq)`で重複排除するため、snapshotから直前に消えたterminal rootと
replay/liveの重複を両方扱える。startup reconcileはin-flight stepだけでなく、未消費proposalと未完了speechも
expired/cancelledへ移し、provider/tool/speechを自動再実行しない。またclassifierが応答しない場合はimmutableな
classification timeout後に旧barrierをfailし、FIFOの次rootをclaimして旧provider/TTSを停止するため、会話が
永久に`draining + queued`で停止しない。

E36/P1 はwarm-up 5回後のin-memory SQLite routing receipt 100回を測定し、p95 403µs（閾値50,000µs）だった。

- `rr_26_unresolved_review_proposes_and_consumes_premium_once`: pass。
- `rr_26_consumed_premium_step_reaches_the_bound_executor`: pass。
- `tests/role-routing-proposal.test.tsx`: 2 pass。
- `tests/role-routing-replay.test.ts`: 2 pass。
- `rr_18_queue_order_and_restart_are_safe`: pass（proposal/speech restart reconcileを含む）。
- `rr_29_provider_before_classifier_raises_durable_barrier_and_queues_follow_up`: pass（timeout後の旧failed、新respondingまで確認）。
- `rr_39_host_receipt_p95_is_under_fifty_milliseconds`: pass、p95 403µs。
- `bun run typecheck`、`bun run lint`、`git diff --check`: pass。

この追補で、以前記載した「proposal生成と承認後executor wakeは未接続」は解消した。E22/E27/実行理由UIは
次の追補で接続した。live lane L01〜L04は引き続き明示許可待ち。

## E22・E27・E34 追加追補（2026-09-22、offline）

E22 は `author → tool_specialist → author` を有限recipeとしてcompileし、通常provider/Codex stepのspecialist出力を
厳密な `{toolName,arguments}` としてだけ受理する。hostは同じrole-root binding、permit、tool budget、operation ledgerで
実行し、そのenvelopeだけを親へ返す。specialist出力はassistant回答へ採用されず、actor差替えでもACLは変わらない。
同一operationの再実行はsettled/unknownを問わずledgerで拒否する。

E27 は通常 `execute_turn` 境界で voice source receipt、specialist host tool、親の最終回答、永続speech intent、UI terminal
event、DB replayを一つのoffline fixtureで通した。この過程で初回receiptが`origin='text'`を固定しASR `sourceId`を
捨てていた不具合を修正し、origin/sourceId/presentationMode/payload digestを受付transactionへ保存した。mock TTS failure
を注入してもrootと画面回答はcompletedのまま、speechだけfailedへ遷移する。

E23 の通常active入力は保守的なhost分類結果をstrict JSON契約へ通し、target/evidence/confidenceを検証してから
coordinator eventへ変換する。挨拶と実質依頼が混在するなど契約不成立時は`unclear`としてbarrierを維持し、immutable
classification timeout後のfailoverで永久停止を避ける。classifierにはcancel/premium/toolの実行権限を与えない。

E34 はsnapshotが直近8 root IDを返し、reload後もterminal rootをDB replayできる。replay済みeventの直近12件を
会話UIの「Routing 実行履歴」に表示する。表示する詳細はhost生成のkindとallowlist済み
reasonCode/reason/candidateId/phaseだけで、draftなど任意payloadは表示しない。frontend保持量は8 root/256 eventに
制限し、allowlist値も制御文字を除去して160文字までに制限する。

- `cargo test --lib 'tests::rr_' -- --test-threads=1`: 186 pass / 0 fail。
- `cargo test --lib role_routing:: -- --test-threads=1`: 162 pass / 0 fail。
- `rr_38_`: 8 pass / 0 fail。
- `rr_15_asr_tool_tts_reconnect_e2e_and_rr_38_normal_provider_turn_specialist_returns_to_parent`: pass。
- `rr_08_normal_active_input_uses_the_strict_structured_contract`: pass。
- frontend replay/proposal/history: 6 pass / 0 fail。
- `bun run typecheck`、`bun run lint`、`cargo fmt --check`、`git diff --check`: pass。
- repo全体 `cargo test --lib -- --test-threads=1`: 1486 pass / 6 fail / 27 ignored。6件は
  `runtime::agent_tools::tests::typed_memory_tool_calls_are_projected_without_generic_fallback`、
  `steward::tests::{ml_01_schema_version_and_empty_goals,ml_05_hold_skips_insert_then_flush_one,ml_06_withdraw_does_not_rewrite_done,ml_08_acceptance_register_divert_complete_withdraw}`、
  `wasm_host_poc::tests::real_kit_inspect_verify_and_vectors_run_without_credentials`（installed Bun 1.4.2 / expected 1.3.14）で、
  role-routing integration testの失敗は0件。

offlineの計画項目とA01〜A42は完了した。残件は明示許可が必要なlive lane L01〜L04と、repo全体gateに元から残る
role-routing外の失敗だけである。live未実施のためR1/R2/R3の「実機完了」は主張しない。
