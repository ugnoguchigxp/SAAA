# SAAA 接続とモジュール予算 実装計画

作成日: 2026-09-20。状態: 接続作業は完了。残る生成通し試験は [生成検査クローズアウト](saaa-generation-closeout-plan.md)。

評価で残った「実装に効く」穴を、既存の生成計画を壊さず閉じる。カードは本書 §8（RW-00〜09）。

## 1. 次に完成させるもの

会話ターンから制限付き capability コマンドが動き、執事 Goal を Coding 画面から登録でき、`lib.rs` と `turns.rs` を増やさずに検査が通る状態。

生成・検査の機能契約（DDL、fake generator、独立 acceptance、64KiB 表示、MCP 非公開）の正本は [実行版検査・動的生成 統合実装計画](saaa-llang-generation-inspection-plan.md) の 12・13 章である。本書は再掲しない。本書が足すのは接続順、行数予算、未配線の復旧、steward の画面だけである。

検証報告の未達表（C07 / C10 / C11 接続 / C13）を、未達のまま成功扱いにしない。

## 2. 対象外（カード内で再議論しない）

| 対象外 | 扱い |
| --- | --- |
| Meeting 機能 | 削除済み。本計画は会議製品の復元をしない |
| Git / dirty / レーン分割 / commit | 作業ツリーの整理はしない。検査はコードの成否だけで見る |
| C14 実モデル live | 既存生成計画の live lane。本計画の完了条件にしない |
| 署名・公証、2時間運転、脆弱性窓口 | 製品配布。本計画に含めない |
| Role Routing、Butler、自動修正、自然文トリガ、M3C | 既存凍結のまま |
| 新しい SqliteWriter、新しい会話 Runtime、新しい env | 禁止 |

相談して止まる条件: Scope / 忘却 / single writer と矛盾する、外部送信が必要になる、size 閾値を緩める、`#![allow(dead_code)]` で clippy を通す。

## 3. 実装調査で分かった接続点

- `DATABASE_SCHEMA_VERSION` は **26**。steward 3 表と `generated_capability_generation_jobs` / `call_owners` / `inspections` が同じ `initialize_database` から `CREATE TABLE IF NOT EXISTS` されている。ML-01 の「26 は steward のみ」と生成報告の「25→26 で生成 3 表」は同じ番号を使っている。現行 26 をその和として固定し、次の意味ある DDL は **27**。
- `runtime/capability_commands.rs` は parse と inspection 表示まで。`#![allow(dead_code)]`。`execute_turn` からは呼ばない。
- `generation/recovery.rs::reconcile` は試験だけ。`initialize_database` は呼んでいない。
- `GenerationService`（C10）は未実装。`generated_capabilities/service.rs` は M1 の import/verify/invoke であり、ここに生成状態機械を足さない。
- C07 の `publication_sync` は未実装。`tool_selection/service.rs` は本番 1,366 行（hard 1,600）。catalog 同期の追記先にしない。
- `execute_turn` は `prepare_runtime_run` のあと `steward::on_user_message`、そのあと coding 分岐、そのあと tool_selection。C11 の挿し口は「入力メッセージが永続化したあと、tool_selection と最初の provider より前」。
- `lib.rs` 本番は hard 800。steward の 3 コマンドが `cancel_coding_job` と同じ行に圧縮されている。生成コマンドを同じパターンで足さない。
- steward IPC 3 つ（`register_steward_goal` / `withdraw_steward_delegation` / `list_steward_tasks`）は Rust のみ。`src/features/coding/` に呼び出しが無い。Chat は既に `CodingJobs` を載せる。
- Frontend 上限寸前: `useAmbientVoiceSession.ts` 683 / 700、`SettingsPage.tsx` 510 / 550。Meeting 用フックは触らない。

## 4. 契約（B0–B7）

**B0 正本。** 生成の型・DDL・CLI・試験シナリオは生成計画 12・13 章。本計画は行数と挿し口と画面。衝突したら生成計画の機能契約を優先し、本計画の「turns を増やさない」を次に優先する。

**B1 schema。** 26 の意味は「現行 initialize が作る steward 表 + 生成/検査表」。既存 migrate 関数の書き換え禁止。新しい列・表は version を 27 に加算してから。26 をもう一度「誰か専用」と書き直さない。

**B2 lib.rs。** 本番 ≤ 800。コマンド列挙は別モジュール。1 行へ複数コマンドを詰めない。`rustfmt` 後の行数で判定する。

**B3 turns。** `execute_turn` は分岐だけにする。会話 provider 本体は既存 `conversation_controller` か新ファイルへ移す。C11 は `runtime/capability_commands` の 1 関数を 1 回呼ぶ。`turns.rs` の本番行は RW-03 完了時を上限とし、C11 で実質増やすなら先に移す。

**B4 dead_code。** 接続したモジュールから `#![allow(dead_code)]` と関数単位の allow を外す。未接続の公開 API を残して allow で隠さない。必要になるまで実装しない。

**B5 生成接続。** C07 → C10 → C11 → C12 → C13 の順。C14 は外。fake generator で C13 まで閉じる。通常会話モデルや tool 出力から `/capability` を起動しない。

**B6 steward UI。** 既存 3 IPC だけ。新しいコマンドを足さない。Memory OFF や Goal なしの表示は no-op の説明で足りる。トリガ文面 `テストを確認して` は画面に出す。自動修正 UI は作らない。

**B7 規模。** size 閾値未緩和。hard を超える追記は分割が先。試験ファイルの巨大さは本計画の対象外（機能を増やさない分割だけなら可、必須ではない）。

## 5. ファイル分担

| ファイル | 役割 |
| --- | --- |
| `src-tauri/src/runtime/command_registry.rs`（新規） | `generate_handler!` の列挙。`lib.rs` は登録呼び出しだけ |
| `src-tauri/src/runtime/turns.rs` | dispatcher。steward 1 呼出し、capability 1 呼出し、coding / conversation へ委譲 |
| `src-tauri/src/runtime/conversation_turn.rs` または既存 controller | 会話 provider 本体の移動先 |
| `src-tauri/src/generated_capabilities/generation/service.rs`（新規） | C10 状態機械。M1 の `service.rs` には足さない |
| `src-tauri/src/generated_capabilities/publication.rs` または新規 `publication_sync.rs` | C07 の 1 transaction |
| `src-tauri/src/persistence/schema.rs` | reconcile 呼び出しと、次 DDL 用の 27。既存分岐は追加のみ |
| `src/features/coding/api.ts` / `CodingJobs.tsx` | steward 3 IPC の最小 UI |
| `src/features/voice/useAmbientVoiceSession.ts` | 純関数・送信へ抽出 |
| `src/features/settings/SettingsPage.tsx` | タブ本体を既存 section へ寄せて薄くする |

5 実装ファイルを超えるカードは枝番へ。

## 6. ゲート

| ゲート | コマンド | 合格 |
| --- | --- | --- |
| 本計画の接続試験 | `cargo test --locked --manifest-path src-tauri/Cargo.toml --lib rw_` および既存 `ml_` / `runtime::capability_commands` / `generated_capabilities` | 件数 > 0、failed 0。0 件実行を合格にしない |
| C13 | 生成計画の通し試験 | fake 経路。credential なし |
| Clippy | `cargo clippy --locked --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings` | 新規 allow 0 |
| size | `bun run size:check` | 閾値未緩和。`lib.rs` 本番 ≤ 800 |
| IPC | steward の TS 呼び出しを足したら frontend 試験。Rust 公開 IPC 型を変えたら `ipc:generate` | 生成物一致 |
| spec | `bun run spec:check` | 警告をエラー |
| 全体 | `bun run check:local` | カード単体の緑だけで完了としない |

## 7. 完了報告

`spec/evidence/connect-and-budget/progress.md` にカード証拠、`results.md` にまとめ。Spec HTML から `../evidence/` へはリンクしない。

報告は「実装済み / offline合格 / live未検証 / 未着手」を分ける。C14 と Meeting 削除は未着手のまま残してよい。

## 8. カード（RW-00〜09）

一枚を完了してから次へ。並行着手しない。

| ID | 対象 | 契約 | 実装すること | 合格条件 |
| --- | --- | --- | --- | --- |
| RW-00 | `spec/evidence/connect-and-budget/progress.md` | §3 | `lib.rs` / `turns.rs` / `tool_selection/service.rs` / Frontend 2 ファイルの本番行、C07/C10/C11 未接続、`reconcile` 未配線、steward UI 0 を記録 | 対象外（Meeting/Git/C14）を本文に書く。0 件実行を合格にしない |
| RW-01 | `runtime/command_registry.rs` + `lib.rs` | B2 | handler 列挙を移す。圧縮行を通常の列挙に戻す | `lib.rs` 本番 ≤ 800。fmt 後も 1 行 1 コマンド。挙動変更 0 |
| RW-02 | `persistence/schema.rs` コメントと steward/生成の version 試験 | B1 | 26 を現行和として固定。次 DDL は 27 と書く。ML-01 と矛盾するコメントを直す | 既存 DB が開く。表の DROP 0。version 数値の勝手な繰り上げ 0（DDL が無いなら 26 のまま） |
| RW-03 | `turns.rs` 分割 | B3 | 会話本体を移す。`execute_turn` は prepare → steward →（空の capability 挿し口）→ coding/conversation | 既存 `ml_` と会話試験が残る。`turns.rs` 本番が RW-00 より減る |
| RW-04 | C07（生成計画 13 章） | B0/B5/B7 | 両台帳・grant・job・epoch を 1 transaction。`tool_selection/service.rs` へ大きな追記をしない | G04/G08 相当の rollback 試験。service.rs が 1,600 を超えない |
| RW-05 | C10 + `generation/recovery.rs` | B4/B5 | `generation/service.rs` 新設。状態遷移・予算・取消。`initialize_database` から `reconcile` | 再起動で running→interrupted。awaiting_activation 保持。自動再生成 0。recovery の allow 削除 |
| RW-06 | C11–C13 | B3/B4/B5 | 永続化後・provider 前に parse。NotACommand は通常会話。Malformed はモデルへ送らない。allow 削除 | `rw_` または計画 12.8 の通し。`turns.rs` 本番 ≤ RW-03 完了時 +20 行。clippy -D |
| RW-07 | `CodingJobs` + `coding/api.ts` | B6 | 登録・撤回・一覧。workspace 必須。トリガ文面を表示 | 二重 Goal 拒否が画面から再現できる。新 IPC 0。Frontend 試験 > 0 |
| RW-08 | `useAmbientVoiceSession.ts`、`SettingsPage.tsx` | B7 | 純関数・既存 section へ抽出。Meeting ファイルは触らない | 両ファイルが hard 未満。音声の既存試験が残る |
| RW-09 | size / clippy / ipc / spec / check:local / results.md | §6/7 | 追加ファイルを baseline 登録。未達を分離 | 閾値緩和 0。C14 を完了扱いしない |

RW-04〜06 の機能詳細・禁止事項は生成計画の C07/C10/C11–C13 をそのまま使う。指示が二重になったら生成計画を正本とする。

指示例: 「RW-01 だけを実装してください。`lib.rs` の本番行を 800 以下にしてください。コマンドを 1 行に詰めないでください。挙動を変えないでください。」
