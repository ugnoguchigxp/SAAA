# Steward loop 進捗

作成日: 2026-09-20。並行差分は巻き戻さない。完了記録は `results.md`。

## SL-00 baseline

| 項目 | 値 |
| --- | --- |
| HEAD | `84ead9752a509cec4d5a81810816cd17cefc507f` |
| dirty（本フェーズ） | World M3A（`runtime/context/world/`、generation_inputs、size baseline、m3-progress/results） |
| dirty（凍結・触らない） | `spec/docs/verification/tool-selection-d5.md`。Role Routing 文書。`src-tauri/src/role_routing/` は作らない。`generated_capabilities/generation` と `runtime/capability_commands.rs` も本 Step では触らない |
| M2 | [m2-results.md](../world-model/m2-results.md)。WorldFrame は Broker 未接続だった |
| 凍結 | [deferred.md](deferred.md) |

## Step 1（SL-01）

既存 M3-00〜21。証拠は [m3-progress.md](../world-model/m3-progress.md) と [m3-results.md](../world-model/m3-results.md)。本表へ再掲しない。

## Step 2（SL-02 / SL-03）

計画書: [saaa-personal-world-model-m3b-plan.md](../../docs/saaa-personal-world-model-m3b-plan.md)。実装完了: [m3b-results.md](../world-model/m3b-results.md)。live 回答は未検証。

## Step 3（SL-04）

計画書: [saaa-situation-tts-gate-plan.md](../../docs/saaa-situation-tts-gate-plan.md)。実装完了: [tts-gate-results.md](../situation/tts-gate-results.md)。live 実会議は未検証。

## Step 4（SL-05）baseline — ML-00

計画正本: [saaa-minimal-loop-plan.md](../../docs/saaa-minimal-loop-plan.md)。

| 項目 | 値 |
| --- | --- |
| schema | 着手時 **25**。既存 migrate 関数は書き換えない。26 は steward 表の CREATE 追加のみ。version 25 の tool_selection backfill 条件は `< 25` に固定し、25→26 で再実行しない |
| coding 語彙 | `coding_start` / `coding_inspect` / `coding_continue` / `coding_cancel`。`source_id` は実ユーザーメッセージ。settings.enabled 既定 false |
| recovery | `coding/recovery.rs` の `reconcile` のみ。独自 PID 判定は足さない |
| Step 3 読取口 | `speech_holds_tts`。`meeting.blocks_tts()` で置換しない |
| Memory | `SAAA_MEMORY_ENABLED=1`。新しい env は作らない |
| 試験 | `cargo test --locked --manifest-path src-tauri/Cargo.toml --lib ml_` → **16 passed** |

## カード

| ID | 状態 | 証拠 |
| --- | --- | --- |
| ML-00 | 完了 | 本記録 |
| ML-01 | 完了 | `ml_01_schema_version_and_empty_goals` / `ml_01_status_changes_insert_not_overwrite` / `ml_01_invalid_ops_rejected` |
| ML-02 | 完了 | `ml_02_second_active_goal_refused_and_no_delete` / `ml_02_register_requires_workspace`。IPC 3（`register_steward_goal` / `withdraw_steward_delegation` / `list_steward_tasks`） |
| ML-03 | 完了 | `ml_03_exact_trigger_queues_and_partial_does_not` / `ml_03_memory_off_or_no_goal_is_noop`。`turns.rs` は `on_user_message` 1 回 |
| ML-04 | 完了 | `ml_04_start_request_is_read_only_and_coding_shift_does_not_start` / `ml_04_duplicate_dedupe_keeps_one_task` / `ml_04_unrelated_turn_does_not_start` |
| ML-05 | 完了 | `ml_05_hold_skips_insert_then_flush_one`。`start_turn` 先頭で `flush_held_reports` |
| ML-06 | 完了 | `ml_06_withdraw_blocks_start_and_continue` / `ml_06_withdraw_does_not_rewrite_done` |
| ML-07 | 完了 | `ml_07_reopen_maps_outcome_unknown_without_rerun` |
| ML-08 | 完了 | `ml_08_acceptance_register_divert_complete_withdraw` |
| ML-09 | 完了 | `results.md`。size は steward 7 ファイルを登録。閾値未緩和 |
