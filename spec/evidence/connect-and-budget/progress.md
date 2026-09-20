# 接続とモジュール予算 進捗

作成日: 2026-09-20。対象外: Meeting 削除、Git 運用、C14 live。

## RW-00 baseline

| 項目 | 値 |
| --- | --- |
| `lib.rs` | 677 行。`cancel_coding_job` と同じ行に steward 3 コマンド |
| `runtime/turns.rs` | 1,110 行。`execute_conversation_turn` が 467 行以降 |
| `tool_selection/service.rs` | 1,366 行（hard 1,600） |
| `useAmbientVoiceSession.ts` | 683 / 700 |
| `SettingsPage.tsx` | 515 / 550 |
| schema | `DATABASE_SCHEMA_VERSION = 27`（v26→v27 は Meeting 表 DROP。steward + 生成/検査表は IF NOT EXISTS） |
| C07 | `publication_sync` 未実装 |
| C10 | `generation/service.rs` 未実装。`recovery::reconcile` は試験のみ |
| C11 | parse/display のみ。`execute_turn` 未接続 |
| steward UI | Frontend 呼び出し 0 |

0 件実行を合格にしない。

## カード結果

| ID | 状態 | 証拠 |
| --- | --- | --- |
| RW-00 | 実装済み | 本ファイルの baseline 表 |
| RW-01 | 実装済み | `runtime/command_registry.rs`。1 行 1 コマンド |
| RW-02 | 実装済み | schema 27 を現行和として固定。次 DDL は 28。数値繰り上げなし |
| RW-03 | 実装済み | `conversation_turn.rs` へ会話本体。`execute_turn` は分岐 |
| RW-04 | offline合格 | `rw_04_catalog_failure_rolls_back_activation` / `rw_04_publish_registers_one_catalog_revision` |
| RW-05 | offline合格 | `rw_05_initialize_database_interrupts_running_jobs` / `rw_05_reconcile_does_not_regenerate`。起動時 `GenerationService::reconcile` |
| RW-06 | offline合格 | `rw_06_*` と `rw_13_generate_a_invoke_update_b_and_keep_prior_call`（fake + fixture）。C14 は未着手 |
| RW-07 | 実装済み | `StewardPanel` + `stewardApi`。`tests/coding-steward.test.ts`。新 IPC なし |
| RW-08 | 実装済み | Settings タブ抽出、`idleVoiceCapture.ts`。Meeting ファイル未変更 |
| RW-09 | 一部 | `bun run size:check` 合格。`cargo test --lib rw_` 7 件。Clippy `-D warnings` は既存 dead_code（ASR 等）で未達。`check:local` は本カードでは未完走 |

`cargo test --locked --manifest-path src-tauri/Cargo.toml --lib rw_` → 7 passed。
