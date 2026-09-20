# Butler Schedule Ledger 進捗

作成日: 2026-09-21。対象: [saaa-butler-schedule-ledger-plan.md](../../docs/saaa-butler-schedule-ledger-plan.md) と [作業カード](../../docs/saaa-butler-schedule-ledger-work-cards.md)。並行差分は巻き戻さない。

## SL-00 baseline

確認基準は着手時の作業ツリー（ブランチ `main`）。

| 項目 | 値 |
| --- | --- |
| HEAD | `5554e723f60368cb434c67d1ed53550f6ce6c925`（`chore: save concurrent capability command update`） |
| schema | `DATABASE_SCHEMA_VERSION = 29`（コメントは 28=role-routing まで。次の本計画 DDL は **30**。既存 migrate 関数は書き換えない） |
| writer | `persistence/sqlite/writer.rs` の単一 `SqliteWriter`。第2 writer は作らない |
| 共有 slot | `memory/personal_state/scheduler.rs` の `foreground()` / `blocking_generation()` / `interrupt()` |
| Situation Hold | `situation/speech.rs` の `speech_holds_tts` / `holds_speech`。`ShadowDecision.proposed_attention` を読む。独自の会議判定は持たない |
| Goal / Delegation 型 | **専用 Rust enum/struct は無い。** 正本は `steward_goals` / `steward_delegations` / `steward_tasks`（文字列 `id`）。`steward/repository.rs` の `ActiveWork` は `goal_id` / `delegation_id: String` |

### 並行作業（巻き戻さない）

| 作業 | 状態 | 本計画での扱い |
| --- | --- | --- |
| World M3A | 完了記録 `spec/evidence/world-model/m3-results.md`（shadow。通常会話投入は未） | 触らない |
| World M3B | 完了記録 `m3b-results.md`（live 正答は M4） | 触らない |
| Situation TTS hold（Step 3） | 完了記録 `spec/evidence/situation/tts-gate-results.md`。live 実会議は未検証 | Hold 判定は `speech_holds_tts` のみ再利用（SL-07） |
| 最小循環（Step 4） | 完了記録 `spec/evidence/steward-loop/results.md`。ML-00〜09。live Codex は未検証 | 発火先の Task は既存 coding / steward。Butler は未接続のまま |
| 生成検査クローズアウト | `spec/evidence/generation-closeout/`。C14 / Meeting / Role Routing は対象外 | 触らない |
| Role Routing | dirty（`src-tauri/src/role_routing/`、Settings）。本計画非対象 | 触らない |
| 生成・inspect・steward UI・voice idle | dirty（`generated_capabilities/`、`runtime/capability_*`、`StewardPanel`、`idleVoiceCapture` 等） | 触らない。巻き戻さない |

dirty の主な領域（着手時スナップショット）: `generated_capabilities/`、`role_routing/`、`runtime/`、`persistence/`、`features/settings/`、`features/coding/StewardPanel`、`features/voice/`、README / spec docs / size baseline。本計画のファイルはこの表の領域に混ぜない。

### Goal / Delegation が型として無い場合の暫定方針

計画 §3 どおり。時刻の到来は権限にしない。

| 列 | 暫定 | 型が確定したあと |
| --- | --- | --- |
| `scope_ref` | 既存 `runtime/context/scope.rs` の Scope 参照（文字列。`user:` / `project:` / `task:` 等） | 変更しない |
| `delegation_ref` | **文字列参照。NULL 許容。** NULL は委任なし。発火しても Act 不可（Ask / `fire_result=no_delegation`） | 外部キー相当の検査を追加。意味論は変えない |
| `subject_ref` | 文字列（`goal:` / `task:` / `commitment:` / `conversation:`） | steward 行 id との照合を後段で足してよい |

`steward_delegations.id` を入れるときはその文字列をそのまま `delegation_ref` に書く。無いときは NULL。SQLite に steward への FOREIGN KEY はまだ張らない（並行作業の表を本計画の migrate で拘束しない）。

着手条件（委任正本と TTS hold）: Step 3/4 の完了記録があり、SL-A を開始してよい。Calendar 接続は SL-A に含めない。default OFF。

## カード

| ID | 状態 | 証拠 |
| --- | --- | --- |
| SL-00 | 完了 | 本記録。型なしの暫定方針を明記。並行変更を巻き戻していない |
| SL-01 | 実装中 | `schedule/ledger.rs`。試験 `sl_01_*` |
