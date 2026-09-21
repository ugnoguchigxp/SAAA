# Role Routing 受入対応表

このファイルは [完了ロードマップ](../../docs/saaa-role-routing-completion-roadmap.md) の E00 成果物。
RR-00〜39、A01〜A42、P1〜P5 の各要求を、担当 E/L 単位・試験名・lane・実行結果へ 1 対 1 で対応させる。

- 記録日: 2026-09-22
- 基準 HEAD: `7471ecadc1d64a5b3c1d4e6a898669be80a2cb8a` + 作業ツリー
- lane: `offline` は通常 suite で完結、`live` は明示許可が必要、`未` は未実装。
- 実行結果: `pass` は本表作成時に当該 command を実行して確認、`未実装` は対応 test が未追加、
  `未検証` は test はあるが今回再実行していない、`live未許可` は認証済みモデル起動が必要。
- 過去ログの件数を今回の合格として再掲しない。結果は `results.md` / `progress.md` に追記する。

## 1. RR-00〜RR-39 → E/L 対応

| RR | テーマ | 担当 E | 依存 | 状態 |
| --- | --- | --- | --- | --- |
| RR-00 | 基準・接続能力の記録 | E00 | - | 部分 |
| RR-01 | 型と純粋イベント | E01 | E00 | 部分 |
| RR-02 | 永続 schema と migration | E01/E03 | E00 | 部分 |
| RR-03 | 設定の検証と版管理 | E01/E06/E07 | E00 | 部分 |
| RR-04 | repository と受付 transaction | E05 | E04 | 部分 |
| RR-05 | reducer と actor の骨格 | E03/E08 | E02 | 部分 |
| RR-06 | 候補生成とルール ranker | E07/E11 | E06 | 部分 |
| RR-07 | context projection の分離 | E09 | E08 | 部分 |
| RR-08 | role 別 prompt と構造化分類 | E19/E23 | E05 | 部分 |
| RR-09 | Provider adapter と内部 Sink | E04/E10/E11 | E03 | 部分 |
| RR-10 | role 別 tool offer と permit | E13 | E11 | 部分 |
| RR-11 | ツール実行の紐付け | E12/E14 | E03 | 部分 |
| RR-12 | 結果の原子的採用 | E02/E04/E10 | E01 | 部分 |
| RR-13 | 発話 owner | E24 | E04 | 部分 |
| RR-14 | IPC・起動・UI 接続 | E10/E24/E25 | E08 | 部分 |
| RR-15 | R1 設定 UI と縦通し gate | E23/E27 | E22 | 部分 |
| RR-16 | 実行中入力の分類 barrier | E02/E05/E23 | E04 | 部分 |
| RR-17 | 更新・cancel・drain | E02/E14/E24 | E05 | 部分 |
| RR-18 | queue・再接続・再起動 | E06/E25 | E03 | 部分 |
| RR-19 | SDK wire 契約と sidecar | E15/E16 | E12 | 部分 |
| RR-20 | SDK 隔離・同梱・能力 gate | E15/L01 | E12 | 部分 |
| RR-21 | Sol 推論と host tool loop | E12/E15/E16 | E13 | 部分 |
| RR-22 | 委任と時間・費用上限 | E04/E06/E07/E11/E13 | E03 | 部分 |
| RR-23 | 回答への反応の紐付け | E19 | E05 | 部分 |
| RR-24 | 独立評価の実行 | E17/E18 | E16 | 部分 |
| RR-25 | 評価後の修正 | E18 | E17 | 部分 |
| RR-26 | Astra 提案と承諾 | E20/E21/L02 | E18 | 部分 |
| RR-27 | 会話 UI の継続操作 | E21/E25 | E20 | 部分 |
| RR-28 | 実行履歴・理由の表示 | E21/E34 | E25 | 部分 |
| RR-29 | R2 競合・実機 gate | E26/L01/L02/L03 | E25 | 部分 |
| RR-30 | 学習 schema・特徴 snapshot | E28 | E26 | 部分 |
| RR-31 | 増分抽出と checkpoint | E29 | E28 | 部分 |
| RR-32 | ラベル生成 | E30 | E29 | 部分 |
| RR-33 | dataset 組立と export | E31 | E30 | 部分 |
| RR-34 | 夜間 scheduler と手動 tick | E32/L04 | E31 | 部分 |
| RR-35 | 集計・shadow ranker | E33 | E31 | 部分 |
| RR-36 | artifact loader・offline 評価 | E33 | E30 | 部分 |
| RR-37 | 削除・失効・学習状況 UI | E34 | E31 | 部分 |
| RR-38 | tool-specialist 差し替え試験 | E22 | E14/E16 | 部分 |
| RR-39 | 全体 gate・性能・報告 | E35/E36/E37 | E34 | 部分 |

欠落 0、重複 0（40 行）。

## 2. A01〜A42 → E 単位・試験

lane が `offline` の行は認証済みモデル・実機を起動しない。`live` 行は L 単位で別途許可を得る。

| A | 担当 E | 目標試験名 | lane | 本表作成時の結果 |
| --- | --- | --- | --- | --- |
| A01 | E10 | `rr_05_normal_turn_two_steps` | offline | pass（`rr_05_normal_turn_two_steps_commits_only_the_final_answer`。通常 `execute_turn` から2 provider stepを順次実行） |
| A02 | E01/E07 | `rr_03_policy_cas_conflict`, `rr_03_policy_validation` | offline | 部分（CAS は `rr_03_policy_cas_conflict` pass。製品 validator は `validate_settings`。E07 の recipe 検査待ち） |
| A03 | E05 | `rr_04_receipt_retry`, `rr_04_same_id_changed_payload` | offline | pass（`rr_04_receipt_retry_and_conflict` が retry/conflict を検証。`rr_04_queue_full_leaves_no_input_message` も pass） |
| A04 | E10/E23 | `rr_05_frontend_ack_then_reasoner` | offline | pass（frontend draftは非公開・非採用、reasoner回答だけを1件commit） |
| A05 | E23 | `rr_08_mixed_greeting` | offline | pass（`rr_08_mixed_greeting_is_not_a_valid_action`） |
| A06 | E23 | `rr_08_timeout_unclear` | offline | pass |
| A07 | E11 | `rr_09_shared_resource_group` | offline | 未実装 |
| A08 | E13/E22 | `rr_10_reviewer_resolved_mutation_denied` | offline | pass（`rr_10_*`, `rr_24_reviewer_cannot_call_a_mutating_tool`） |
| A09 | E14 | `rr_11_duplicate_operation_once` | offline | pass（`rr_11_duplicate_operation_keeps_the_original_reservation`） |
| A10 | E02 | `rr_12_db_failure_no_speech` | offline | pass（E02 実装） |
| A11 | E24 | `rr_13_late_synthesis_not_played` | offline | pass（`rr_13_*`） |
| A12 | E27 | `rr_15_asr_tool_tts_reconnect_e2e` | offline | 未実装 |
| A13 | E05 | `rr_16_multiple_pending_inputs` | offline | pass（E05 実装） |
| A14 | E23 | `rr_16_status_does_not_cancel` | offline | pass |
| A15 | E23/E26 | `rr_16_clarification_keeps_barrier` | offline | 未実装 |
| A16 | E02 | `rr_12_old_revision_result_rejected` | offline | pass（E02 実装） |
| A17 | E13/E14 | `rr_10_update_between_reserve_and_invoke` | offline | 未実装 |
| A18 | E14 | `rr_11_detached_owner_settles` | offline | 未実装 |
| A19 | E25 | `rr_18_restart_no_replay` | offline | 未検証（`rr_18_*` pass） |
| A20 | E25 | `rr_14_subscribe_replay_race` | offline | 未実装 |
| A21 | E19 | `rr_23_challenge_starts_new_root` | offline | 未実装 |
| A22 | E18 | `rr_25_normal_turn_author_review_revise` | offline | 未実装 |
| A23 | E19 | `rr_23_wrong_conversation`, `rr_23_quoted_negative` | offline | pass（`rr_08_quote_not_feedback`, `rr_08_bad_target_is_rejected`, `rr_23_*`） |
| A24 | E17 | `rr_24_false_evidence` | offline | pass |
| A25 | E11/E18 | `rr_22_loop_budget` | offline | 未実装 |
| A26 | E20 | `rr_26_premium_no_implicit_execution` | offline | pass |
| A27 | E20 | `rr_26_approval_consumed_once` | offline | 未実装 |
| A28 | E11/E20 | `rr_22_cloud_revoked_before_dispatch` | offline | pass（receipt 後に SDK availability を剥奪し、provider session 0件を確認） |
| A29 | E16 | `rr_21_changed_revision_new_thread` | offline | 未実装 |
| A30 | E16/L01 | `rr_21_sol_tool_roundtrip` | offline/live | 未実装 |
| A31 | E28 | `rr_30_feature_snapshot_immutable` | offline | 未実装 |
| A32 | E29 | `rr_31_crash_before_checkpoint` | offline | 未実装 |
| A33 | E30 | `rr_31_explicit_feedback_next_day` | offline | 未実装 |
| A34 | E30 | `rr_32_silence_not_success` | offline | 未実装 |
| A35 | E31 | `rr_33_group_split` | offline | 未実装 |
| A36 | E31 | `rr_33_partial_file_not_ready` | offline | pass（`rr_33_partial_file_not_ready_and_export_is_deterministic`） |
| A37 | E32 | `rr_34_missed_night` | offline | 未実装（`rr_32_scheduler_handles_overnight_window_and_idle_gate` は pass） |
| A38 | E33 | `rr_35_small_sample_rules` | offline | 未実装 |
| A39 | E33 | `rr_35_shadow_no_second_call` | offline | 未実装 |
| A40 | E34 | `rr_37_forget_invalidates_before_dispatch` | offline | pass |
| A41 | E22 | `rr_38_normal_turn_specialist_returns_to_parent` | offline | 未実装 |
| A42 | E33 | `rr_36_no_counterfactual_labels` | offline | pass |

欠落 0、重複 0（42 行）。`pass` は本表作成時の既存 test 名であり、E 単位の完了を意味しない。
各 E の合格条件に必要な縦通し・故障・競合が欠ける場合は部分のままとする。

## 3. P1〜P5 → E/L 単位

| P | 指標 | 担当 E/L | 測定 command / fixture | lane | 状態 |
| --- | --- | --- | --- | --- | --- |
| P1 | host routing/DB 受付 p95<=50ms | E36 | `rr_39_*` warm5+100 mock | offline | 未実装 |
| P2 | 受付 TTFA p95<=1500ms | L03 | 実音声 30 件 | live | live未許可 |
| P3 | 最終回答追加遅延 p95 | L03 | baseline 交互 30 件 | live | live未許可 |
| P4 | 回答品質・継続性 | L03 | 固定日本語 100 件 blind | live | live未許可 |
| P5 | 夜間負荷 | L04 | page100 + 実機 30 | live | live未許可 |

## 4. 最初に着手可能な offline 作業

E00 完了時点で、明示許可を待たずに進められる offline 単位:

1. E01 契約整合（本表の A02 の一部）。
2. E02 stale 採用拒否（A10/A16）。
3. E03 step lifecycle と DB 制約。
4. E04 中間 output と最終採用の分離（A10）。
5. E05 入力 receipt と pending barrier（A03/A13）。
6. E06 policy と queue 期限。

live lane（L01〜L04）は認証済みモデル ID・回数・費用上限の提示と明示許可まで実行しない。
