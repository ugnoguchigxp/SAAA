# Role Routing 実装進捗

## 実装監査（2026-09-21、未完了）

このファイルは完了報告ではない。作業カードの合格条件に対しては、R1〜R3すべて未完了である。詳細なカード別の状態、根拠、残作業は[作業カード](../../docs/saaa-role-routing-work-cards.md#実装監査2026-09-21)を正本とする。

| カード | 実装 | 検証 |
| --- | --- | --- |
| RR-01 | 契約・reducer・signalsの断片を追加 | 指定event/Clock/ID注入・境界試験は未実装 |
| RR-02 | SQLiteへpolicy/root/input/decision/step/output/event/feedback/tool-link ledgerを追加 | migrationとcross-root tool-link FKの一部試験のみ。C6全制約は未検証 |
| RR-03 | `routing.roles/default` を無効既定で追加。保存時にimmutable policy snapshotを採番 | `bun test tests/settings-regressions.test.ts tests/settings-review.test.ts`（11 pass） |
| RR-04〜06 | receipt transaction、queue上限拒否、provider route override、pure reducer/ranker断片。active root がある receipt は queued のまま provider 前で待機 | receipt retry/conflict、actor adapter/contextは未実装 |
| RR-07/08/10/12/13 | context projection、follow-up分類、pure tool permit、message transactionでのroot採用、speech queue断片 | 実executor/dispatcher/speechへの接続なし |
| RR-11 | tool operationのroot/step/revision linkを既存tool ownerの前後に記録 | tool-selectionとの縦通し・detach試験は未実施 |
| RR-14 | `get_routing_snapshot` とroot-local `replay_routing_events` のIPC、生成TS binding、chat表示hookを追加。永続cancel後eventと3秒snapshot pollingで更新 | 全eventのlive store、ASR receipt、legacy/duplicate試験は未実装 |
| RR-17 | tool receiptをtransaction内で確認し、`reserved`/`dispatched`/`unknown`の間はrevision再開を拒否 | input update/child drainの実行経路と全順序試験は未実装 |
| RR-18 | 起動時にin-flight rootをinterrupted evidenceとして永続化。queued receipt は provider 前で待機し、turn 終端で最古の開始可能 root を `IMMEDIATE` transaction で claim して実行を解放 | 再接続、restart 後 queued receipt の明示的再開 UX は未実装 |
| RR-27 | 永続root cancel IPC、snapshotを読むchat表示・停止操作を追加。receipt commit後に実行中run/TTSへcancelを通知 | amend/reconsider、live event購読、child drain/restartは未実装 |
| RR-28 | chatに永続snapshot由来のphase/revision/queue先頭を本文非表示で表示 | 実行履歴・選択理由UIは未実装 |
| RR-19 | 固定SDK版のJSONL sidecarとRust JSONL frame validatorを追加。frame/累積/最終結果の上限とcancelを強制 | mock SDK wire試験、実認証SDK呼び出しは未実施 |
| RR-20 | sidecarをBun compiled resourceとして同梱し、Tauri resource pathから起動する Rust ProcessGuard adapter を追加。空の一時cwd、read-only、approval/network/web/MCP無効化、限定環境で SDK を起動するよう固定 | 実 actor dispatcher への接続、live isolation fixture は未実施 |
| RR-22 | root/step deadlineと未知費用・上限超過を拒否する純粋budget判定を追加 | 実dispatcherでの中断・usage集計接続は未実装 |
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
- 隔離test binary: `rr_17_tool_unknown_blocks_restart`、`rr_17_settled_tool_allows_next_revision`: 各1 pass
- 隔離test binary: `rr_18_queue_order_and_restart_are_safe`: 1 pass
- 隔離test binary: `rr_18_claims_fifo_only_after_the_prior_root_is_terminal`、`rr_18_claim_skips_a_conversation_with_active_work`: 各1 pass
- 隔離test binary: `rr_05_commit_before_dispatch`、`rr_16_barrier_is_durable_before_resume`: 2 pass（queued step の `planned → running` 遷移を含む）
- 隔離test binary: `role_routing::`: 53 pass
- 隔離test binary: `rr_33_partial_file_not_ready_and_export_is_deterministic`: 1 pass
- 隔離test binary: `rr_36_invalid_hash_falls_back_to_rules`、`rr_36_new_model_cold_start_and_score_are_safe`: 各1 pass
- 隔離test binary: `rr_36_no_counterfactual_labels`: 1 pass
- 隔離test binary: `rr_37_forget_invalidates_before_dispatch`、`rr_37_forget_feedback_source_invalidates_its_target_dataset`: 各1 pass
- 隔離test binary: `rr_37_learning_snapshot_contains_counts_only`: 1 pass
- 隔離test binary: `role_routing::learning::`: 10 pass
- 隔離test binary: `rr_19_frame_validation_rejects_unknown_ids_and_duplicate_terminals`、`rr_19_frame_validation_rejects_extra_fields_and_empty_results`、`rr_20_process_requires_a_terminal_frame`、`rr_20_process_accepts_only_valid_result_protocol`、`rr_20_cancel_sends_protocol_cancel_then_reaps_child`、`rr_20_config_isolation_clears_parent_environment_and_arguments`: 6 pass
- `bun --check scripts/role-routing/codex-sidecar.ts`: pass。version不一致・非JSON frameがstdoutを出さず拒否されることを確認。
- `printf '{not-json}\\n' | src-tauri/resources/bin/role-routing-codex`: pass（同梱 sidecar が外部認証・実モデル呼び出しなしで起動し、不正 frame を stdout に出さず終了）
- 隔離test binary: `rr_22_unknown_cost_is_not_free_when_policy_has_a_budget`、`rr_22_deadline_rejects_new_dispatch`: 各1 pass
- `bun run typecheck`: pass
- `bun run desktop:smoke -- --report-dir spec/evidence/role-routing/desktop-smoke-20260921-role-routing`: pass（build / bundle / launch / IPC ready）
