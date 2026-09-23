# SAAA 自己診断（Self Diagnosis）実装計画

作成日: 2026-09-22。状態: **実装済み**。本書が正本。作業カード DG-00〜DG-16。

## 0. 目的と方針

アプリ起動直後に、SQLite・設定・LLM/TTS/ASR Provider・LARM Harness・embedding・memory・world model・ContextStill の状態をバックグラウンドで一括点検し、結果を `AppState` に保持する。ユーザーは会話画面のボタンメニュー（歯車ポップオーバー）から「自己診断」を押してモーダルで結果を見る。モーダル内の「再診断」で同じ点検を再実行できる。

守ること:

- **起動をブロックしない。** SQLite open/migrate は現行どおり `setup` 内で同期。それ以外の点検は `setup` 完了後に spawn する。診断が失敗・遅延してもアプリは使える。
- **既存 probe を束ねる。** 新しい疎通ロジックは書かない。`providers::probe::test_model_provider`、`service_harness::resolve_with_legacy_llm`、`reachability` snapshot、`world::capabilities::status`、`personal_state::commands::summary` を呼び、共通の `DiagnosisItem` に変換する。
- **必須と任意を分ける。** `severity` を `fatal | degraded | info` で持ち、`fatal` は SQLite と設定文書だけ。外部サービスは `degraded` 以下。
- **単一飛行（single-flight）。** 実行中に再診断を押しても二重実行しない。実行中の結果を待って返す。
- **秘密情報を出さない。** メッセージは `redact::redact_runtime_text` を通す。URL・トークン・provider 応答本文は item に入れない。
- **有料 probe を避ける。** 既存 probe のうち生成を伴うもの（Cloud TTS 等）はそのまま使うが、1 Provider あたり timeout 8 秒、全体 20 秒で打ち切る。起動時に自動で走るのは 1 回のみ。周期実行はしない（既存 `reachability_watcher` が LAN 到達性を担う）。

非目標: 自動修復、診断結果による route 変更（`role_routing/availability.rs` は既存 reachability を引き続き参照）、周期再診断、設定画面への新セクション。

## 1. 既存資産（変更せず呼ぶだけ）

| 用途 | 場所 | シグネチャ |
| --- | --- | --- |
| Provider 疎通 | `src-tauri/src/providers/probe.rs` | `pub(crate) async fn test_model_provider(state: &AppState, input: TestProviderInput) -> Result<ProviderTestResult, String>` |
| Harness 解決 | `src-tauri/src/providers/service_harness.rs` | `pub(crate) async fn resolve_with_legacy_llm(address: &str) -> Result<HarnessResolution, String>`（`services: Vec<HarnessServiceStatus { capability, state, model, message, .. }>`） |
| LAN 到達性 | `src-tauri/src/providers/reachability.rs` | `state.reachability.snapshot() -> ReachabilitySnapshot` |
| Provider 設定読出 | `src-tauri/src/persistence/settings.d/01.rs` | `pub(crate) fn load_model_providers(connection: &Connection) -> Result<ModelProvidersSettings, String>` |
| World model | `src-tauri/src/runtime/context/world/capabilities.rs` | `pub(crate) fn status(c: &Connection, conversation: &str) -> Result<WorldContextStatus, String>` |
| Memory 要約 | `src-tauri/src/memory/personal_state/commands.rs` | `state.sqlite_writer.read_serialized(memory::personal_state::commands::summary)` |
| ContextStill | `src-tauri/src/memory/context_still_recall.d/01.rs`, `context_still_search.d/01.rs` | `is_configured(&self) -> bool` |
| Schema version | `src-tauri/src/persistence/schema.rs` | `DATABASE_SCHEMA_VERSION: i64` |
| 診断 JSON 出力 | `src-tauri/src/diagnostics.rs` | `export_diagnostics(state) -> Result<LocalArtifactResult, String>` |
| TS 型生成 | `src-tauri/src/ipc_contract/bindings.rs` | `typescript_bindings()` に各 domain の `typescript_bindings()` を連結、`bun run ipc:generate` |
| コマンド登録 | `src-tauri/src/runtime/command_registry.rs` | `saaa_invoke_handler!` に 1 行 1 コマンド |
| テスト用 AppState | `src-tauri/src/test_support.rs` | `pub(crate) fn app_state(connection: Connection) -> AppState` |
| FE モーダル focus | `src/components/useDialogFocus.ts` | `useDialogFocus(open, onClose) -> { dialogRef, fallbackRef }`（実装を読んで戻り値を確認） |
| FE ボタンメニュー | `src/features/chat/ConversationBehaviorMenu.tsx` | `<details className="conversation-behavior-popover">` 内パネル |
| FE イベント購読例 | `src/features/chat/useDelegatedReports.ts` | `listen<T>("delegated-report-committed", ...)` |
| i18n | `src/i18n/locales/jaCore.ts`, `enCore.ts` | `chat.*` 配下 |

## 2. 設計

### 2-1. モジュール構成（新規）

```
src-tauri/src/diagnosis/
  mod.rs          pub(crate) mod contract; pub(crate) mod store; pub(crate) mod runner; pub(crate) mod commands; mod checks;
  README.md       所有・不変条件・検索アンカー（短く）
  contract.rs     DiagnosisReport / DiagnosisItem / DiagnosisStatus / DiagnosisSeverity / typescript_bindings()
  store.rs        DiagnosisStore（最新レポート + 実行中フラグ + 完了通知）
  runner.rs       run(state) -> DiagnosisReport。checks を並列実行し集約
  commands.rs     #[tauri::command] get_diagnosis_report / run_diagnosis
  checks/mod.rs   pub(super) trait なし。関数を列挙: sqlite, settings, providers, harness, memory, world, context_still
  checks/sqlite.rs
  checks/settings.rs
  checks/providers.rs
  checks/harness.rs
  checks/memory.rs      memory + world + context_still をまとめる（小さいので 1 ファイル）
src/features/diagnosis/
  api.ts               invoke ラッパ + zod 検証
  DiagnosisModal.tsx   モーダル本体
  DiagnosisModal.css
  useDiagnosisReport.ts  取得・再診断・イベント購読 hook
```

`lib.rs` には `mod diagnosis;` の 1 行のみ追加（800 行予算）。コマンド wrapper は `lib.d/01.rs` に足さず `diagnosis/commands.rs` に置く。

### 2-2. 契約型（`diagnosis/contract.rs`）

```rust
use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum DiagnosisStatus { Ok, Warn, Fail, Skipped, Running }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum DiagnosisSeverity { Fatal, Degraded, Info }

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DiagnosisItem {
    /// 安定 ID。例: "sqlite", "settings.providers", "provider.<provider_id>", "harness.llm", "harness.embedding", "memory.personal_state", "world.status", "context_still.recall"
    pub(crate) id: String,
    /// 表示グループ。"storage" | "settings" | "llm" | "voice" | "harness" | "memory"
    pub(crate) group: String,
    /// 表示名（英語固定。FE は id で i18n し、無ければ label を表示）
    pub(crate) label: String,
    pub(crate) status: DiagnosisStatus,
    pub(crate) severity: DiagnosisSeverity,
    /// redact 済みの短い説明。空文字可
    pub(crate) message: String,
    pub(crate) latency_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DiagnosisReport {
    /// 単調増加。再診断ごとに +1
    pub(crate) revision: u64,
    pub(crate) started_at: String,     // crate::now_iso()
    pub(crate) finished_at: Option<String>,
    pub(crate) running: bool,
    /// Fail かつ Fatal が 1 つでもあれば "fail"。Fail/Warn があれば "warn"。それ以外 "ok"。running 中は "running"
    pub(crate) overall: DiagnosisStatus,
    pub(crate) items: Vec<DiagnosisItem>,
}

pub(crate) fn typescript_bindings() -> String { /* world capabilities と同じ形式で 4 型を export */ }
```

`items` の順序は runner 側で固定（storage → settings → llm → voice → harness → memory）。FE はソートしない。

### 2-3. 状態（`diagnosis/store.rs`）

```rust
pub(crate) struct DiagnosisStore {
    latest: std::sync::Mutex<DiagnosisReport>,
    running: std::sync::atomic::AtomicBool,
    finished: tokio::sync::Notify,
}
impl DiagnosisStore {
    pub(crate) fn new() -> Self            // revision 0, running false, overall Skipped, items 空
    pub(crate) fn snapshot(&self) -> DiagnosisReport
    pub(crate) fn try_begin(&self) -> Option<u64>   // running を CAS で true にし、次 revision を返す。既に実行中なら None
    pub(crate) fn publish(&self, report: DiagnosisReport)  // latest 置換、running false、finished.notify_waiters()
    pub(crate) async fn wait_finished(&self)  // finished.notified().await
}
```

`AppState` に `pub(crate) diagnosis: Arc<diagnosis::store::DiagnosisStore>` を追加。`lib.d/02.rs` の `app.manage(AppState{..})` と `test_support::app_state` の両方で初期化する。

### 2-4. 実行フロー

1. `lib.d/02.rs`: `providers::reachability_watcher::spawn(...)` の直後に `diagnosis::runner::spawn_startup(app.handle().clone())` を追加。
2. `runner::spawn_startup(app)`: `tauri::async_runtime::spawn` で `run_and_publish(&app).await`。
3. `run_and_publish(app)`: `store.try_begin()` が `None` なら `wait_finished().await` して `snapshot()` を返す。`Some(revision)` なら `running: true` の暫定レポートを publish せず（`snapshot()` は前回値を返し続ける。FE は `running` を IPC 戻り値の `running` フラグで見る）、各 check を `tokio::join!` で並列実行、`tokio::time::timeout(Duration::from_secs(20), ...)` で全体を打ち切り、`DiagnosisReport` を組み立て `store.publish()`、`app.emit("diagnosis-updated", &report)`。
4. IPC `run_diagnosis` は `run_and_publish` を呼んで完了レポートを返す。`get_diagnosis_report` は `snapshot()` を返す。

各 check の共通形: `pub(super) async fn check(state: &AppState) -> Vec<DiagnosisItem>`（同期のものは `fn`）。失敗しても panic せず Fail item を返す。1 Provider probe は `tokio::time::timeout(Duration::from_secs(8), ...)`、timeout は `status: Fail, message: "timed out after 8s"`。

### 2-5. 各 check の判定表

| id | group | severity | Ok 条件 | Warn | Fail | Skipped |
| --- | --- | --- | --- | --- | --- | --- |
| `sqlite` | storage | fatal | `sqlite_readers.read` で `SELECT 1` 成功かつ `PRAGMA user_version == DATABASE_SCHEMA_VERSION` | version 不一致（起動 migrate 後は起きないはず。起きたら Warn） | read エラー | — |
| `settings.providers` | settings | fatal | `load_model_providers` 成功 | provider が 0 件 | parse エラー | — |
| `provider.<id>` | llm / voice（kind で振分: `CloudAsr`/`CloudTts`/`SystemTts` → voice、他 → llm） | degraded | `test_model_provider(...).ok` | — | `!ok` または timeout | `enabled == false` の provider は item を作らない |
| `harness.reachability` | harness | degraded | `reachability.snapshot()` が到達可 | 未確定（snapshot 取得前） | 到達不可 | DynamicLan provider が無い |
| `harness.<capability>` | harness | degraded（`embedding` は info） | `resolve_with_legacy_llm(address)` の `HarnessServiceStatus.state == "ready"` | `state == "degraded"` | `state == "unavailable"`、または resolve 自体が Err（その場合 id `harness.resolve` 1 件で Fail） | `harness.address` が空 |
| `memory.personal_state` | memory | degraded | `summary` 読出成功 | — | Err | — |
| `world.status` | memory | degraded | `capabilities::status(c, PRIMARY_CONVERSATION_ID)` 成功 | — | Err | — |
| `context_still.recall` / `context_still.search` | memory | info | `is_configured()` true | — | — | false（message "not configured"） |

`harness.embedding` は Harness の `services` に `capability == "embedding"` があればその state を映す。無ければ Skipped。`embed_query` の実呼出しは行わない（session lease が必要で、起動時コストに見合わない）。

### 2-6. FE

- 入口: `ConversationBehaviorMenu` のパネル末尾に `<button type="button" onClick={onOpenDiagnosis}>{t("chat.diagnosis.open")}</button>`。props に `onOpenDiagnosis: () => void` を追加。
- 状態: `ChatPage` に `const [diagnosisOpen, setDiagnosisOpen] = useState(false)`。`ChatPage` 末尾で `{diagnosisOpen && <DiagnosisModal onClose={() => setDiagnosisOpen(false)} />}`。`App.tsx` は触らない（450 行予算）。
- `useDiagnosisReport()`: mount 時 `getDiagnosisReport()`、`listen("diagnosis-updated")` で置換、`rerun()` は `running` を true にして `runDiagnosis()` を await。unmount で unlisten。
- `DiagnosisModal`: `role="dialog" aria-modal="true"`、`useDialogFocus`、背景クリック/Escape で閉じる。ヘッダに overall バッジと `finishedAt`、本文は `group` ごとに `<section>`、各 item は status アイコン + label + message + latency。フッタに「再診断」（`running` 中は disabled、ラベル `chat.diagnosis.rerunning`）と「閉じる」。
- label の i18n: `t(\`chat.diagnosis.items.${id}\`, { defaultValue: item.label })`。`provider.<id>` は `defaultValue` に落ちる（provider label を message に含める）。

## 3. 作業カード

各カードは実装 1〜3 ファイル＋試験 1 ファイルを標準とする。前提は直前カードの完了。試験名は `dg_NN_具体条件`。カードごとに `cargo test --manifest-path src-tauri/Cargo.toml diagnosis` または `bun test tests/diagnosis-*.test.tsx` を通す。commit はカード単位を推奨するが必須ではない。

| ID | 対象 | 実装すること | 合格条件 |
| --- | --- | --- | --- |
| DG-00 | `spec/evidence/self-diagnosis/progress.md`（新規） | HEAD、dirty 差分（`persistence/schema.rs`、`role_routing/schema.rs` が変更中）を記録。schema.rs へは触らない方針を明記 | ファイル存在。以降のカードで完了時刻を追記 |
| DG-01 | `diagnosis/mod.rs`, `diagnosis/contract.rs`, `lib.rs`, `ipc_contract/bindings.rs`, `examples/export_ipc_bindings.rs` | §2-2 の 4 型と `typescript_bindings()`。`bindings.rs::typescript_bindings()` の末尾に連結。`export_ipc_bindings.rs` に `src/lib/generated/diagnosis.ts` 書出しを追加し `bun run ipc:generate` | `bun run ipc:check` 通過。`src/lib/generated/diagnosis.ts` に 4 型が export される。`dg_01_status_serializes_kebab_case`（`Skipped` → `"skipped"`） |
| DG-02 | `diagnosis/store.rs`, `app_state.rs`, `lib.d/02.rs`, `test_support.rs` | §2-3 の `DiagnosisStore`。`AppState.diagnosis` 追加。`app.manage` と `test_support::app_state` で `Arc::new(DiagnosisStore::new())` | `dg_02_try_begin_is_single_flight`（2 回目は None）、`dg_02_publish_clears_running_and_bumps_revision`。`cargo build` 通過 |
| DG-03 | `diagnosis/checks/mod.rs`, `diagnosis/checks/sqlite.rs` | `pub(super) fn sqlite(state: &AppState) -> DiagnosisItem`。`SELECT 1` と `PRAGMA user_version` を `sqlite_readers.read` で実行 | `dg_03_sqlite_ok_on_initialized_database`（in-memory + `initialize_database` で Ok）。`dg_03_sqlite_warns_on_version_mismatch`（`PRAGMA user_version = 1` に書き換え後 Warn） |
| DG-04 | `diagnosis/checks/settings.rs` | `pub(super) fn settings(state) -> DiagnosisItem`（`settings.providers`）。`load_model_providers` を呼ぶ | `dg_04_settings_ok_with_defaults`、`dg_04_settings_warn_when_no_providers`（providers を空配列で保存後 Warn） |
| DG-05 | `diagnosis/checks/providers.rs` | `pub(super) async fn providers(state) -> Vec<DiagnosisItem>`。enabled provider ごとに `test_model_provider` を 8 秒 timeout で呼ぶ。`ProviderTestResult` → item（message は `redact_runtime_text` 済のものをそのまま）。group を kind で振分 | `dg_05_system_tts_reports_ok`（SystemTts のみ有効 → 1 件 Ok, group voice）。`dg_05_disabled_provider_is_omitted`。`dg_05_unreachable_provider_fails_within_timeout`（`https://example.invalid` で Fail、10 秒以内） |
| DG-06 | `diagnosis/checks/harness.rs` | `pub(super) async fn harness(state) -> Vec<DiagnosisItem>`。`harness.reachability`（snapshot）と `harness.<capability>`（`resolve_with_legacy_llm` 10 秒 timeout）。`address` 空なら両方 Skipped | `dg_06_harness_skipped_when_address_empty`。`dg_06_harness_resolve_error_yields_single_fail`（`http://127.0.0.1:9` で `harness.resolve` Fail）。embedding capability の有無で `harness.embedding` が Ok/Skipped になる単体（`HarnessResolution` を手組みして変換関数だけ試験） |
| DG-07 | `diagnosis/checks/memory.rs` | `pub(super) fn memory(state) -> Vec<DiagnosisItem>`：`memory.personal_state`、`world.status`、`context_still.recall`、`context_still.search` | `dg_07_memory_and_world_ok_on_fresh_database`。`dg_07_context_still_skipped_when_not_configured`（`ContextStillRecallClient::disabled()` を持つ state） |
| DG-08 | `diagnosis/runner.rs` | `pub(crate) async fn run_and_publish(app: &tauri::AppHandle) -> DiagnosisReport`、`pub(crate) fn spawn_startup(app: tauri::AppHandle)`、内部 `async fn collect(state: &AppState) -> Vec<DiagnosisItem>`（`tokio::join!` で sqlite/settings/providers/harness/memory を並列、順序固定）と `fn overall(items) -> DiagnosisStatus`。全体 20 秒 timeout。`AppHandle` に依存しない `collect` を試験対象にする | `dg_08_overall_fail_only_when_fatal_fails`、`dg_08_overall_warn_when_degraded_fails`、`dg_08_collect_orders_groups`（storage → settings → llm → voice → harness → memory）。`dg_08_collect_completes_on_fresh_state`（外部無しで 3 秒以内） |
| DG-09 | `lib.d/02.rs` | `providers::reachability_watcher::spawn(...)` の直後に `diagnosis::runner::spawn_startup(app.handle().clone());` | `cargo build`。手動: `bun run tauri dev` 起動後 stderr に `self diagnosis published revision=1 overall=...` が 1 回出る（runner に `eprintln!` を 1 行入れる） |
| DG-10 | `diagnosis/commands.rs`, `runtime/command_registry.rs` | `#[tauri::command] async fn run_diagnosis(app: tauri::AppHandle) -> Result<DiagnosisReport, String>`、`#[tauri::command] fn get_diagnosis_report(state: State<AppState>) -> Result<DiagnosisReport, String>`。registry に 2 行追加。`run_and_publish` 内で `app.emit("diagnosis-updated", &report)` | `cargo clippy --all-targets -- -D warnings` 通過。`dg_10_get_report_returns_store_snapshot` |
| DG-11 | `diagnostics.rs` | `export_diagnostics` の payload に `"selfDiagnosis": state.diagnosis.snapshot()` を追加 | 既存 2 試験が通り、`dg_11_export_includes_self_diagnosis`（`payload["selfDiagnosis"]["revision"]` が数値） |
| DG-12 | `src/features/diagnosis/api.ts` | `getDiagnosisReport(): Promise<DiagnosisReport>`、`runDiagnosis(): Promise<DiagnosisReport>`。zod で `DiagnosisReport` を検証（`z.object` を api.ts 内に定義。`ipcValidation.ts` は触らない）。型は `src/lib/generated/diagnosis.ts` から import | `tests/diagnosis-api.test.ts`: 正常 payload 通過、`status: "bogus"` は reject |
| DG-13 | `src/features/diagnosis/useDiagnosisReport.ts` | §2-6 の hook。`listen("diagnosis-updated")` で置換、`rerun()`、`error` 状態 | `tests/diagnosis-hook.test.tsx`: mount で 1 回 invoke、`rerun` 中 `running === true`、イベント受信で report 置換 |
| DG-14 | `src/features/diagnosis/DiagnosisModal.tsx`, `DiagnosisModal.css`, `src/i18n/locales/jaCore.ts`, `enCore.ts` | §2-6 のモーダル。i18n キー: `chat.diagnosis.{open,title,rerun,rerunning,close,overall.{ok,warn,fail,running,skipped},status.{ok,warn,fail,skipped,running},group.{storage,settings,llm,voice,harness,memory},items.*}`。`items.*` は §2-5 の固定 id 分のみ | `tests/diagnosis-modal.test.tsx`: `role="dialog"` が存在、Escape で `onClose`、`running` 中は再診断ボタン disabled、item が group 見出し下に描画 |
| DG-15 | `ConversationBehaviorMenu.tsx`, `ChatPage.tsx` | `onOpenDiagnosis` prop 追加とボタン、`ChatPage` に open 状態とモーダル描画 | `tests/conversation-behavior-menu.test.tsx`（既存があれば追記）: ボタン押下で `onOpenDiagnosis` が呼ばれる。`bun run size:check` 通過（超えたら `ChatPage` からモーダル制御を `useDiagnosisModal.ts` へ分離） |
| DG-16 | `diagnosis/README.md`, `src-tauri/src/README.md`, `spec/evidence/self-diagnosis/progress.md` | README に所有・不変条件（single-flight、redact、起動非ブロック）・検索アンカーを 10 行以内。`src/README.md` の一覧に `diagnosis` 1 行追加。evidence に `bun run check:local` の結果と手動確認（起動→モーダル→再診断）の記録 | `bun run check:local` 全通過。evidence に実行日時とコマンド出力の要点 |

## 4. カード別の補足（詰まりやすい点）

### DG-01

- `ts_rs` の enum は `#[serde(rename_all = "kebab-case")]` と `#[ts(...)]` の両方が必要な版がある。`ipc_contract.rs` の `RuntimeFailureCode` を真似る。生成された `diagnosis.ts` の中身が `"ok" | "warn" | ...` になっていることを目で確認する。
- `export_ipc_bindings.rs` は複数ファイルを `fs::write` している。同じ形式で 1 本追加する。`typescript_bindings()` に連結する場合は 1 つの文字列に混ざるため、別ファイルにしたいなら `diagnosis::typescript_bindings()` を独立して書き出す（推奨: 独立ファイル `diagnosis.ts`、先頭コメントは他ファイルと同文）。
- `tests/ipc_contract_bindings.rs` は生成物と現在の出力の一致を見る。生成後にコミットへ含める。

### DG-02

- `AppState` を構築している場所は `lib.d/02.rs` と `test_support.rs` の 2 箇所のみ（`rg -n "AppState \{" src-tauri/src` で確認）。両方に `diagnosis` を足さないとコンパイルが落ちる。
- `Notify::notify_waiters()` は待っていない側には届かない。`wait_finished` は `try_begin` が None を返した直後に呼ぶ前提で、`running` を再確認してから待つ（`if !running.load() { return snapshot }`）。

### DG-05

- `test_model_provider` は `probe_state::record_if_current` で probe 記録を更新する副作用がある。これは設定画面の接続テストと同じ効果で、問題ない。
- provider ID を item id に使うため `validate_identifier` 済の値である。`format!("provider.{}", provider.id())`。
- `TestProviderInput { provider: provider.clone() }`。`ModelProvidersSettings.providers` を `iter().filter(|p| p.enabled())` で回す（`ModelProviderSettings::enabled(&self) -> bool` と `id(&self) -> &str` は `models/provider_settings.rs` に既存）。

### DG-06

- `harness.address` は `ModelProvidersSettings.harness.address`。空判定は `trim().is_empty()`。
- `HarnessServiceStatus.state` は `"ready" | "degraded" | "unavailable"` の 3 値（`service_harness.rs` 154/307/330 行）。それ以外が来たら Warn 扱い。
- 変換関数 `fn items_from_resolution(resolution: &HarnessResolution) -> Vec<DiagnosisItem>` を pure に切り出し、試験はこれだけを対象にする（ネットワーク不要）。

### DG-08

- `tokio::join!` の各 future は `&AppState` を借用する。`run_and_publish` の中で `let state = app.state::<AppState>();` を 1 回取り、`collect(&state).await` に渡す。
- 全体 timeout 発火時は、集まっている分だけでレポートを作らず、`id: "diagnosis.timeout", status: Fail, severity: Degraded` 1 件＋既に完了した item を返す実装は複雑になる。**単純化: timeout 時は `overall: Warn`、`items` は `[timeout item]` のみ。** 実運用で見たい場合は次の再診断で取り直せる。
- `overall`: `items.iter().any(|i| i.status == Fail && i.severity == Fatal)` → Fail; `any(status == Fail || status == Warn)` → Warn; それ以外 Ok。

### DG-10

- `tauri::AppHandle` を引数に取るコマンドは `app: tauri::AppHandle` と書く（`State` と混在可）。`app.emit` は `use tauri::Emitter;` が必要（他所の `app.emit` 呼出しの use を真似る）。
- 二重実行時（`try_begin` None）は待って前回結果ではなく **完了した最新結果** を返す。

### DG-12〜14

- `invoke` は `@tauri-apps/api/core`、`listen` は `@tauri-apps/api/event`。既存 `src/lib/runtime.ts` と `useDelegatedReports.ts` の import を真似る。
- テストは `bun test`。DOM は既存 `tests/setup-checklist.test.tsx` の書き方（render 方法、`invoke` の mock 方法）をそのまま流用する。新しい test util を作らない。
- `useDialogFocus` の戻り値（ref の名前）は実装を読んで合わせる。`AuditLogPage.tsx` 332 行付近が使用例。
- CSS はスコープ用に `.diagnosis-modal` 接頭辞をつける。既存 `.audit-drawer` の backdrop/位置指定を参考にするが共有しない。

## 5. 検証コマンド

```
cargo test --manifest-path src-tauri/Cargo.toml diagnosis
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
bun run ipc:generate && bun run ipc:check
bun test tests/diagnosis-api.test.ts tests/diagnosis-hook.test.tsx tests/diagnosis-modal.test.tsx
bun run typecheck && bun run lint && bun run size:check
bun run check:local            # DG-16 で 1 回
```

手動確認（DG-16）: `bun run tauri dev` → 起動 → 会話画面の歯車 → 「自己診断」 → モーダルに revision 1 の結果 → 「再診断」 → ボタンが disabled になり、完了後 revision 2 に更新される → Escape で閉じる。

## 6. リスクと対処

| リスク | 対処 |
| --- | --- |
| Cloud TTS probe が課金される | 起動時 1 回のみ。周期実行なし。将来は `kind == cloud-tts` を起動時 Skipped にするフラグを `checks/providers.rs` に 1 定数で持てる構造にしておく |
| 起動時 probe が Provider の rate limit を消費 | 1 Provider 1 回、8 秒 timeout。既存の接続テストと同頻度 |
| `probe_state::record_if_current` の副作用で route 状態が変わる | 既存の接続テストと同じ経路。挙動差は生まない |
| `AppState` 構築箇所の漏れ | DG-02 で `rg -n "AppState \{"` を実行し 2 箇所を確認 |
| モジュールサイズ予算超過（tsx 550、`App.tsx` 450、`lib.rs` 800） | `App.tsx`・`lib.rs`・`lib.d/01.rs` に手を入れない構成。`ChatPage` 超過時は hook へ分離 |
| 20 秒 timeout で結果が空になる | 再診断で取り直せる。timeout item の message に「再診断してください」を入れる |
