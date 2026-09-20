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
