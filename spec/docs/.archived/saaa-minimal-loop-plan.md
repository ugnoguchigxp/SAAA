# SAAA 最小執事循環 実装計画 — 本コンセプト縦糸の締め

作成日: 2026-09-20。状態: Step 4 の実装計画。コードは本計画の作成では変更しない。

前提は Step 3 完了記録（`spec/evidence/situation/tts-gate-results.md` 引き渡し）。上位は [執事循環フェーズ](saaa-steward-loop-phase-plan.md) §8 と [Personal AI Concept](saaa-personal-ai-concept.md) §9・§16。カードは本書 §8（10枚、ML-00〜09）。

この Step は、観測→黙る／話す→任された一件が続く、という縦糸の最後の実体である。M3C・Butler・M4 は計画があるが、前提が揃うまで **見送り中**（禁止ではない）。Role Routing は本フェーズ着手禁止。詳細は [フェーズ計画](saaa-steward-loop-phase-plan.md) §8。

## 1. 次に完成させるもの

ユーザーが明示登録した **一件** の Goal に対し、指定 Git ワークスペースのテスト失敗を許可範囲で調べ、結果を **一メッセージ** で報告する。別話題へ移っても仕事は続き、委任を撤回すれば止まり、再起動後は未完了を一覧して状態照会する。

含めない: 自動修正、複数 Goal、定期ポーリング、LLM による Goal 推定、新しい実行デーモン、カレンダー／期限 tick（Butler 計画は設計のみ）。

この段階の利用者は開発環境。`SAAA_MEMORY_ENABLED=1` かつ active な Goal 行があるときだけ Reducer が start/inspect する。行が無いときは default OFF。live Codex / 実テスト失敗は完了条件にしない。

## 2. Step 3 から固定する判断

カード内で再議論しない。

| # | 判断 |
| --- | --- |
| 1 会議中の報告 | 通知保留は Step 3 の `speech_holds_tts`（`MEETING` ∧ IGNORE/OBSERVE）を再利用する。`meeting.blocks_tts()` で置換しない。分類器は触らない。 |
| 2 前景 Coding | `snapshot()` は使わない。メモリ上の `signals.foreground.category` だけ読む。遷移の記憶は steward 側（前回 category）。Situation にフィールドを足さない。 |
| 3 実行正本 | 実行は既存 `coding_jobs` / `coding_runs`。PID 判定は `coding/recovery.rs` の `reconcile` だけ。steward は loop_state を写像する。 |
| 4 権限 | Situation・World・モデル本文から委任を作らない。start は明示トリガと active Delegation の両方。Coding 遷移だけでは start しない（inspect のみ）。ユーザーメッセージを偽造して `source_id` を作らない。 |
| 5 忘却 | Goal/Delegation/Task 行の削除は直接 DELETE しない。忘却は既存 forget journal。status 変更は新行 + `superseded_by`。 |
| 6 Writer | 既存 `SqliteWriter` 一つ。`initialize_database` の既存 migrate 関数は書き換えず、version を 25→**26** に加算して CREATE TABLE を足す。 |
| 7 会話経路 | `execute_turn` から薄い Reducer 呼び出しだけ。新しい会話 Runtime は作らない。Broker に命令 Candidate を足さない。 |
| 8 World | 報告文に WorldFrame を必須としない。M3B 配線を steward 判定に使わない。 |

相談して止まる条件（それ以外は進める）: Scope／忘却／single writer と矛盾する、外部送信や自動修正が必要になる、既存 coding ジョブで受入シナリオが満たせない。代替に新しい runner を出さない。既定は「inspect のみ自動、start は明示トリガ」。

## 3. 実装調査で分かった接続点

- `DATABASE_SCHEMA_VERSION` は **25**（`persistence/schema.rs`）。version 25 の backfill は tool_selection。26 は steward 表の追加のみ。
- `coding_jobs` は `conversation_id` と **UNIQUE な `source_id`**（ユーザーメッセージ）を持つ。`coding_start` は workspace が会話に登録済みであること、`coding_settings.enabled`、同時 active run 1 を要求する。
- ツール名は `coding_start` / `coding_inspect` / `coding_continue` / `coding_cancel`。撤回は既存 `cancel_coding_job` / `coding_cancel`。
- 再起動時 `initialize_database` が既に `coding::recovery::reconcile` を呼ぶ。starting/running は `outcome_unknown` または `interrupted`。不明なら再実行しない。
- 報告の本文挿入は既存 `conversation_messages`（assistant）。TTS は `speech_holds_tts` が true なら開始しない（Step 3）。保留中は表の `held_reason` に載せ、会話へは出さない。解放は次の `start_turn` 先頭（hold でないとき）で flush。Situation tick へは繋がない。
- Memory 有効化は `memory/control_plane` の `SAAA_MEMORY_ENABLED=1`。新しい env は作らない。
- 固定トリガはユーザー本文 trim が **完全一致** `テストを確認して`。部分一致も LLM 抽出もしない。
- `turns.rs` は既に大きい。Reducer は `steward::on_user_message` をユーザーメッセージ INSERT のあと 1 回だけ呼ぶ。

## 4. 契約（B0–B7）

**B0 正本。** 表は `steward_goals` / `steward_delegations` / `steward_tasks`。World Goal ノードへ転記しない。実行状態の正本は coding 側。

**B1 Goal。** `origin` は `user_explicit` のみ。status は `active | paused | withdrawn`。変更は INSERT 新行 + 旧行 `superseded_by`。同時 active Goal は 1。2 件目の登録は拒否。

**B2 Delegation。** `ops` は CHECK で `read` / `test_run` のみ（必要なら両方可の固定値 `read_test` 1 種）。`budget_runs` / `budget_ms`。撤回は即時 `withdrawn`。running なら既存 cancel。撤回後の `coding_start` は steward 経由 0。withdrawn を active に戻さない。

**B3 Task。** `loop_state` は `queued | running | awaiting_user | done | failed | cancelled`。`dedupe_key` = `workspace_id` + 失敗シグネチャ（offline は `fixture:test-failure`）。同一 key で queued/running/awaiting_user があるとき INSERT しない。`coding_job_id` は start 後にだけ埋める。`report_json` にモデル confidence を書かない。`complete: true` のモデル申告は無視し、coding run の state だけ写す。

**B4 Reducer。** Memory ON かつ active Goal+Delegation があるときだけ動く。(1) トリガ完全一致 → workspace 登録済みなら queued、予算内なら既存 `coding_start`（request は読み取り・テスト照会に固定した文面。patch/commit を含む文字列は start 前拒否）。(2) 前景が Coding へ変わった → 既存 Task の `coding_inspect` のみ。source メッセージ偽造 0。

**B5 報告。** done / failed / cancelled で 1 メッセージ。hold 中は `held_reason=meeting`、会話挿入 0、TTS 0。hold 解除後の flush は 保留分を **1 通** にまとめる。World 引用は任意で、無くても合格。

**B6 再起動。** 同じ DB を開き直す。`reconcile` 後、steward は coding 状態を写像するだけ。outcome_unknown は inspect。再実行 0。「続きを」（トリガと同じ固定文でよい）は withdrawn なら復活せず撤回を報告し、それ以外は inspect。

**B7 規模と IPC。** モジュールは `src-tauri/src/steward/`。公開 IPC は登録・撤回・一覧の 3 コマンドまで。`bun run ipc:generate`。size 閾値未緩和。`classifier.rs` / `recovery.rs` の方針変更 0。

## 5. ファイル分担

| ファイル | 役割 |
| --- | --- |
| `persistence/schema.rs` | version 26。既存分岐は追加のみ |
| `steward/schema.rs` | CREATE TABLE と CHECK |
| `steward/repository.rs` | 登録・一覧・撤回・写像。Writer 経由 |
| `steward/reduce.rs` | トリガと Coding 遷移 |
| `steward/report.rs` | 1 メッセージ、hold flush |
| `steward/commands.rs` | IPC 3 つ |
| `runtime/turns.rs` / `start_turn.rs` | 1 呼び出しずつ。本体を肥やさない |

5 実装ファイルを超えるカードは枝番へ。`coding/` の cancel/inspect/start は呼ぶが、recovery の PID ロジックは複製しない。

## 6. ゲート

| ゲート | コマンド | 合格 |
| --- | --- | --- |
| 本 Step | `cargo test --locked --manifest-path src-tauri/Cargo.toml --lib ml_` | 件数 > 0、failed 0 |
| Clippy | `cargo clippy --locked --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings` | 警告 0 |
| size | `bun run size:check` | 閾値未緩和 |
| IPC | 型変更時 `bun run ipc:generate` と ipc 試験 | 生成物一致 |
| spec | `bun run spec:check` | 警告をエラー |
| 全体 | `bun run check:local` | 機能カード通過だけで完了としない |

filter 0 件を合格にしない。並行 dirty（Role Routing / D5）は触らない。p95 を新設しない。既存 coding / Broker 予算を緩めない。

## 7. 完了報告

`spec/evidence/steward-loop/progress.md` にカード証拠、`results.md` にまとめ。Spec HTML から `../evidence/` へはリンクしない。

報告は「実装済み / offline合格 / live未検証 / 未着手」を分ける。live Codex と実リポジトリのテスト失敗は live未検証。

この Step の完了をもって、Concept §16 の最初の循環（状態が効く / 会議中は黙る / 一件が続き止めれば止まる）は offline で閉じる。Butler / M3C / M4 は準備が整っていないので見送り中。Role Routing は本フェーズでは着手しない。

## 8. カード（ML-00〜09）

| ID | 対象 | 契約 | 実装すること | 合格条件 |
| --- | --- | --- | --- | --- |
| ML-00 | steward-loop/progress.md | §6 | HEAD、schema 25、coding 語彙、recovery、Step 3 読取口、Memory flag を記録 | 既存 migrate を書き換えない方針を明記。0 件実行を合格にしない |
| ML-01 | schema.rs + steward/schema.rs | B0/B1/B2/B3 | version 26。3 表。CHECK。UNIQUE(dedupe) は active 相当に限定 | 旧 DB が開く。Goal 0 件。上書き UPDATE で status を書き換えない |
| ML-02 | steward/repository.rs + commands.rs | B0/B7 | 登録・一覧・撤回。IPC 3。forget journal。第 2 writer なし | 直接 DELETE 0。2 件目の active Goal 拒否。不正 ops 挿入 0 |
| ML-03 | reduce.rs + turns.rs | B4 | 完全一致トリガ → queued。Memory OFF または Goal なしは no-op | 命令位置 1。別 Runtime 0。部分一致 0 |
| ML-04 | reduce.rs + coding 接続 | B3/B4 | start は read/test 固定 request。budget 超過は start しない。Coding 遷移は inspect のみ | 書込みツール 0。遷移のみで start 0。source 偽造 0。同一 dedupe で Task 1 |
| ML-05 | report.rs + start_turn.rs | B5 | 終端 1 メッセージ。`speech_holds_tts` なら保留。解除後 1 通 | hold 中 TTS 0、会話挿入 0。confidence 0 |
| ML-06 | repository + coding_cancel | B2/B6 | 撤回即時、以後 steward start 0。「続きを」で復活 0 | withdrawn のまま。遅延結果は新操作に使わない |
| ML-07 | 再起動写像 | B6 | 同一実 DB 再オープン。outcome_unknown は inspect。再実行 0 | recovery 既存。独自 PID 0 |
| ML-08 | 実DB受入 | §1 シナリオ | Goal登録→別話題→完了通知→撤回→再起動→続き | Scope 漏洩 0、重複 Task 0、撤回後新規実行 0。`ml_` 件数 > 0 |
| ML-09 | size / ipc / check:local / results.md | §6/7 | baseline 登録。live未検証を分離 | 閾値緩和 0。生成品質を完了扱いしない |

ML-03 の固定トリガ以外の自然文理解は実装漏れとして残す。M3C（自然文からの候補抽出）に回し、本 Step では見送り中。

指示例: 「ML-04 だけを実装してください。Coding 前景だけで `coding_start` しないでください。`recovery.rs` に PID 判定を足さないでください。ユーザーメッセージを INSERT して source を作らないでください。」
