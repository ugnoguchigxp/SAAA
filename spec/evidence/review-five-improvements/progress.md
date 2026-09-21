# 再レビュー改善5点 進捗

作成日: 2026-09-21。正本: `spec/docs/saaa-review-five-improvements-terra-plan.md`。

## F-00 現状と所有境界

| 経路 | producer | 台帳 | consumer |
| --- | --- | --- | --- |
| 委任完了 | `runtime/pi/runner.rs` が `coding_events` を commit | `coding_events` / `steward_event_cursor` | `steward/driver.rs` → verifier → outbox |
| 復旧完了 | `coding/recovery.rs` | 同上 | 起動時 `pump::drain` |
| 状況変化 | `situation/tick.rs` の decision/transition | Situation ledger | `Wake::signal` → drain |
| 通知設定 | `work_amend` | `steward_delegations.notify` | drain が最新 notify を読む |
| 4領域choose | provider/tool/plan/notification の既存 `adaptive_improvement::choose` | `ai_artifacts` / `ai_activations` | 通常選択。override → adaptive → rules |
| World入力 | `question_input::read`（保存済み user/transcript） | runtime_runs.input_message_id | 定型parser + 自然文理解 → GraphRequest |

着手時の dirty 差分（role_routing / turns 等）は維持。対象外の一括書換えはしていない。

再現していた不足: verifier が job ID だけで Pass、intake が抜粋+引用記号、Wake が 45s tick 依存、World が4定型文、学習昇格が通常IPC未接続。

## カード状態

| ID | 状態 | 検証 |
| --- | --- | --- |
| F-00 | 完了 | 本表 |
| V-01 | 完了 | `rf5_v_01_*` |
| V-02 | 完了 | runner が host session から証拠を persist。失敗時は成功証拠なし |
| V-03 | 完了 | `rf5_v_03_*` settled+欠損で後続0 |
| V-04 | 完了 | Missing を成功ラベルにしない。Pass のみ Goal done |
| V-05 | 完了 | `legacy_unverified` は Unknown。UI に根拠/不足表示。schema 32 加算 |
| V-06 | 実装済み・受入待ち | 単体/統合の境界は通した。実 profile の委任実行はローカル agent 未接続 |
| N-01〜06 | 完了 | `rf5_n_*` と既存 `dw_r03_*` |
| W-01〜05 | 実装済み・受入待ち | drain 合流・commit 後 signal・起動 drain・due timer。実アプリ 1s/2s 計測は未実施 |
| G-01〜06 | 実装済み・受入待ち | 4intent×日英言い換えの host 理解。実モデル1経路の精度測定は未実施 |
| A-01〜08 | 実装済み・受入待ち | 評価record/IPC/設定UI/choose 接続。gate 合格の実測 bundle は未投入のため改善効果は未実証 |
| F-01 | 外部条件待ち | 通常UIの実委任+会議hold+実モデル質問は実行環境不足 |

既存 `dw_` 回帰: 49 passed / 0 failed（`cargo test --lib dw_`）。
