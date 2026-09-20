# 最小執事循環 完了記録

作成日: 2026-09-20。状態: Step 4 実装完了（明示 Goal 一件とテスト照会ループ）。live Codex / 実リポジトリのテスト失敗は未検証。計画は `spec/docs/saaa-minimal-loop-plan.md`。カード別は `progress.md`。

## 1. 結論

ユーザーが IPC で一件の Goal を登録し、本文 trim が完全一致 `テストを確認して` のときだけ Task を queued にする。実行は既存 `coding_jobs` / `coding_runs`。終端は assistant 1 通。会議中は `speech_holds_tts` で会話へ出さず、次の `start_turn` で 1 通にまとめて flush する。撤回は新行 + `superseded_by` で、以後 steward 経由の start はしない。再起動後は `coding/recovery.rs` の `reconcile` を写すだけ（`outcome_unknown` → `awaiting_user`、再実行しない）。

default OFF は active Goal 行が無いこと。Reducer は `SAAA_MEMORY_ENABLED=1` のときだけ動く。新しい env、新しい runner、World 五要素、Role Routing、Butler は入れていない。

## 2. ゲート

| ゲート | コマンド | 結果 |
| --- | --- | --- |
| 本 Step 試験 | `cargo test --locked --manifest-path src-tauri/Cargo.toml --lib ml_` | **16 passed** / 0 failed |
| Clippy | `cargo clippy --locked --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings` | 本 Step の unused は解消。失敗は並行 dirty の `runtime/capability_commands.rs`（未使用 public API、本 Step 非対象） |
| size | `bun run size:check` | steward 7 ファイルを baseline 登録。閾値未緩和。失敗は並行 dirty の `generated_capabilities/*` と `capability_commands.rs`（未登録・ratchet 超過） |
| IPC | `bun run ipc:check` | **passed**（TS union 変更なし。`ipc:generate` 不要） |
| spec | `bun run spec:check` | **passed** |
| check:local | `bun run check:local` | 未完走。上記 clippy / size が同じ並行 dirty で止まる |

filter 0件を成功扱いしていない。

## 3. 実装済み / offline合格 / live未検証 / 未着手

| 区分 | 内容 |
| --- | --- |
| 実装済み | schema 26 と 3 表。登録・撤回・一覧 IPC。トリガ Reducer。Coding 遷移は inspect のみ。報告 hold/flush。撤回と再起動写像 |
| offline合格 | 部分一致 0、Memory OFF / Goal なしは no-op、二重 Goal 拒否、同一 dedupe で Task 1、hold 中の会話挿入 0、撤回後の新規 queued 0、再オープン後 job 1 のまま |
| live未検証 | `coding_settings.enabled` での実 `coding_start`、実 Git ワークスペース、実テスト失敗、実 TTS での報告読み上げ |
| 未着手 | 自然文トリガ、自動修正、複数 Goal、Butler Schedule、M3C、M4、Role Routing |

## 4. 接続点

- `execute_turn` は `prepare_runtime_run` のあと `steward::on_user_message` を 1 回だけ呼ぶ
- `start_turn` は検証直後に `flush_held_reports`
- 固定 request は patch/commit を含まない読み取り文面。coding 無効時は queued のまま（offline）
- 前景は `SituationRuntime::foreground_category`。Situation にフィールドは足していない

## 5. 引き渡し

Concept §16 の最初の循環（状態が効く / 会議中は黙る / 一件が続き止めれば止まる）は **offline** で閉じる。次段は別コンセプト作業。
