# Steward 整合性 — results

確認時点 HEAD: `314625d`。`budget.rs` の `LazyLock` 初期化の折り返し 1 行だけ、そのコミットに対して未コミット。

## 変更ファイル

- `src-tauri/src/steward/outbox.rs`
- `src-tauri/src/steward/reduce.rs`
- `src-tauri/src/steward/report.rs`
- `src-tauri/src/steward/budget.rs`
- `src-tauri/src/steward/dispatch.rs`
- `src-tauri/src/runtime/pi/runner.rs`
- `src-tauri/src/steward/queue.rs`
- `src-tauri/src/steward/invalidation.rs`
- `src-tauri/src/steward/commands.rs`
- `src-tauri/src/steward/tools.rs`
- `src-tauri/src/steward/mod.rs`
- `src-tauri/src/steward/tests/sc.rs`
- `src/features/coding/stewardApi.ts`
- `src/features/coding/StewardPanel.tsx`

`runtime/pi/process.rs` と `request_intent.rs` は編集していない。

## コマンド

| コマンド | 結果 |
| --- | --- |
| `cargo test --manifest-path src-tauri/Cargo.toml sc_ -- --test-threads=8` | 7 passed / 0 failed |
| `cargo test --manifest-path src-tauri/Cargo.toml steward:: -- --test-threads=8` | 88 passed / 0 failed / 2 ignored |
| `bun test tests/steward-panel.test.tsx` | 2 passed / 0 failed |
| `bun run ipc:check` | 7 passed / 0 failed |
| `bun run size:check` | 失敗。上限は上げていない |

`steward::` の ignored 2 件は既存の macOS 音声試験（`steward_speech_delivery_reaches_playback_finished`、`steward_speech_delivery_records_a_real_tts_failure`）。今回の失敗ではない。

## 新規試験

7 件すべて成功。

- `sc_01_flush_rolls_back_message_when_mark_flushed_fails`
- `sc_02_remaining_deadline_uses_elapsed_run_time`
- `sc_02_short_budget_stops_before_fixed_1800_seconds`
- `sc_03_runner_request_contains_goal_summary`
- `sc_04_reorder_changes_next_eligible_task`
- `sc_05_forget_keeps_sibling_goal_report`
- `sc_06_withdraw_one_goal_leaves_the_other_running`

SC-02 の短い停止は、runner の待受が使う `wait_budget` で 80ms 予算を待った結果である。実 Pi プロセスは起動していない。残量が尽きたときの子プロセス停止は、既存の `clear_queue` / `abort` 分岐がこの期限を見る。

## 触った module の size:check

上限は変更していない。確認時点で次が予算を超える、または baseline がない。

| module | 行数 | ratchet | baseline |
| --- | --- | --- | --- |
| `src-tauri/src/steward/budget.rs` | 91 | 41 | 37 |
| `src-tauri/src/steward/queue.rs` | 161 | 140 | 127 |
| `src-tauri/src/steward/dispatch.rs` | 340 | 170 | 154 |
| `src-tauri/src/steward/mod.rs` | 80 | 65 | 59 |
| `src-tauri/src/runtime/pi/runner.rs` | 327 | 248 | 225 |
| `src-tauri/src/steward/tests/sc.rs` | 新規 | なし | なし |

`stewardApi.ts` と `StewardPanel.tsx` はこの失敗一覧に出ていない。リポジトリ全体の size:check は、今回触っていない module も多数超過している。

## 未実施

実モデル、実マイク、署名、公証、sleep/wake、ディスク外読み取りの拒否、自然文の拒否句の追加は実施していない。Tauri の画面操作は開いていない。撤回ボタンの有無は `tests/steward-panel.test.tsx` で確認した。
