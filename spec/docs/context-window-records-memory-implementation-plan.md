# ContextWindow・入力記録・メモリー統合 実装計画

作成日: 2026-09-22。状態: **計画（未実装）**。設計の正本は [設計書](context-window-records-memory-design.md)。本書は設計書を現行コードに落とすための作業手順で、設計書と矛盾する場合は本書の §0-2/§0-3 を優先する（設計書には同内容を追記済み）。作業カード CW-00〜CW-55。**初めて作業する担当 AI は先に [引き継ぎ文書](context-window-records-memory-handoff.md) を読む。**

## 0. 目的・範囲・確定した判断

### 0-1. 目的

1. Provider usage（input/cache_read/output）と費用を generation ごとに保存し、cache 効果を測れるようにする。
2. アプリが受領した Tool 結果・Web 取得本文を SQLite に immutable な record として保存し、モデルには ref/outline を渡す。
3. 保存した record を `read_record` / `recall_activity` で読めるようにし、Memory OFF でも「さっきの検索」「2 番目」「当時の本文」「何を根拠に」に答えられるようにする。
4. chat_completions 経路の prompt を `F（固定）→ L（追記）→ D（動態）→ U（依頼）` に再編し、Segment 単位で F+L の bytes が変わらないようにする。
5. 忘却（forget）を records・rendering・Segment まで波及させる。

### 0-2. 非目標（本計画では実装しない。設計書の該当節は「将来」として残す）

| 設計書の項目 | 扱い |
| --- | --- |
| `recall_memory` / `query_world` の新規論理操作 | 既存 `recall_experience/rule/skill`・`search_knowledge/episodes`・World 注入で代替。カタログ登録も行わない |
| LLM 抽出 job（D 章 65,536 超の抽出、課金上限） | 実装しない。65,536 超も outline + 範囲読取のみ |
| `history_bound` 経路（AgentSession）の Segment 化 | 実装しない。AgentSession は現行の履歴構築を維持し、adapter 契約で `history_binding = "session"` と宣言するだけ |
| zstd 圧縮 | 実装しない。`blobs.codec` は常に `identity`。列だけ用意する |
| 引き継ぎ（carry）の LLM 要約 | 実装しない。carry は Runtime が決定的に作れる項目のみ |
| embedding による record 検索 | 実装しない。trigram FTS のみ |
| FTS の非同期索引化 | 実装しない。readable_text の索引は保存 transaction 内で同期実行（上限 1 MiB） |
| Provider 側 cache 削除 API 呼出し | 実装しない |
| 容量上限（10 GiB / 80% / 95%）の enforcement | 通知だけ実装（CW-44）。取得停止は実装しない |
| 外部 ContextStill 結果の record 化 | 実装しない |

### 0-3. 確定した判断（設計書で未決だった 5 点）

| # | 項目 | 判断 |
| --- | --- | --- |
| 1 | 既存 `ProviderInputBudget::apply` の履歴削減と F+L 不変条件の衝突 | Segment 有効時は `apply` を呼ばない。代わりに `SegmentBuilder::build` が L の entry 選択で予算を満たし、満たせなければ再構成（新 Segment）を 1 回だけ試みる。それでも溢れるなら `required_context_overflow` で失敗させる（既存 reason code を流用）。Segment 無効時（AgentSession、または flag OFF）は現行どおり `apply` を使う |
| 2 | history_bound 経路の契約 | 本計画の範囲外（§0-2）。`AdapterContract` 型（CW-41）に `history_binding: "none" \| "session"` を持たせ、`"session"` のとき Segment 経路を選ばない |
| 3 | L に追記する内容の生成主体 | **LLM 派生要約を L に入れない。** L の entry は次の 3 種のみ。(a) `conversation_messages` の user/assistant 本文（既存 `MAX_LOADED_HISTORICAL_CHARS = 4_000` の両端切り詰めを踏襲）、(b) 完了した Tool 往復の要約 `tool_round`（tool 名、引数 digest、結果 record ref、結果先頭 256 bytes、status）、(c) 引き継ぎ `carry`（Segment 先頭のみ）。すべて Runtime が決定的に生成する |
| 4 | carry の内容と縮退 | v1 の carry は `scope_refs, constraints, active_operations, adopted_evidence_refs, omitted_history_locator` の 5 項目。`goal, target, completion_criteria, decisions, open_items, unresolved_conflicts` は `null` かつ `status: "not_extracted"` で出す。carry が 8,192 bytes を超えたら `adopted_evidence_refs` を新しい順に切り、次に `active_operations` 以外を落とす。`active_operations`（既存 continuations）は落とさず、落とせなければ RED |
| 5 | record の認可式 | v1: 読取可 ⇔ `records.principal_id == 呼出 principal` **かつ** `records.forget_epoch IS NULL` **かつ** 次のいずれか。(i) `record_scopes` に行が無く `records.conversation_id == 呼出 conversation_id`、(ii) `record_scopes.scope_key` のいずれかが呼出側 `allowed_scope_keys`（`runtime/context/scope.rs::ScopeSnapshot.scopes[].key`）に含まれる。`relation` 列は `"visible"` 固定で列だけ持つ。認可不可と不存在は同一の `unavailable` 応答 |

追加で確定する運用値:

- Segment 機能 flag: 環境変数 `SAAA_CONTEXT_SEGMENTS`（`"1"` で有効）。`AppState.context_segments_enabled: bool` に起動時 1 回読む。CW-40 まで既定 OFF、CW-40 の受入後に既定 ON へ切り替える（同カードで env を `SAAA_CONTEXT_SEGMENTS=0` で無効化できる形に反転）。
- B は `ProviderInputBudget::usable_context_bytes()` の値（chat 既定 53,760）を使う。設計書の 64,000 は wire 上限（`MAX_PROVIDER_CONTEXT_WIRE_BYTES`）であり別物。割合上限（25% 等）は B に対して計算する。
- 新 module は `src-tauri/src/records/`（保存・読取・認可・忘却）と `src-tauri/src/runtime/context/segment/`（F/L/D）。既存 `memory/context_window` は触らず、Segment 無効時の経路として残す。

## 1. 既存資産（変更せず呼ぶ、または最小変更）

| 用途 | 場所 | シグネチャ / 事実 |
| --- | --- | --- |
| SQLite 書込 | `persistence/sqlite/writer.rs` | `SqliteWriter::write(f)`, `write_transaction(f)`, `read_serialized(f)` |
| SQLite 読取 | `persistence/sqlite/readers.rs` | `sqlite_readers.read(f)`（`query_only=ON`） |
| schema version | `persistence/schema.rs` | `DATABASE_SCHEMA_VERSION: i64 = 38`。`initialize_database` が CREATE IF NOT EXISTS 群 → 段階 migrate → 最後に `user_version` |
| ID/時刻 | `util.rs`, `schedule/tick.rs` | `new_id(prefix) -> String`, `now_iso()`, `now_ms() -> i64` |
| SHA-256 | `generated_capabilities/contracts.d/02.rs` | `sha256_hex(&[u8]) -> String` |
| Generation manifest | `runtime/context/generation.d/01.rs` | `BeginGeneration{run_id, provider_session_id, provider_id, purpose, request_payload, envelope_payload, current_instruction_count}`、`begin(state, input) -> GenerationHandle`、`GenerationHandle::dispatch()`。表 `context_generations`（digest と `projected_bytes` のみ。本文なし） |
| Generation 入力記録 | `runtime/context/generation_inputs.rs` | `record(...)`。`context_generation_inputs` に source_kind/id/digest |
| Context 投影（現行） | `memory/context_window.d/01.rs` | `LoadedContextWindow`, `compose(loaded) -> ContextWindow`。順序: system(CONTEXT_POLICY) → MEMORY_PROJECTION → CONTINUITY_GROUPS → RECENT_DIALOGUE_HISTORY → user |
| Provider 予算 | `runtime/context/broker.rs` | `ProviderInputBudget::{openai_compatible, agent_session}`, `usable_context_bytes()`, `apply(base)`（先頭 assistant から削除）, `compose(BrokerInput)` |
| ターン準備 | `runtime/conversation_prepare.rs` 73–135 行 | `compose_after_connect(state, input, identity, regional, budget) -> FreshProviderContext{envelope, world, history}` |
| system 結合 | `runtime/conversation_context.rs` | `compose_provider_history(...)`: `.s11tnext/conversation-respond.txt` + CONTEXT_POLICY を 1 本の system にし、投影を `ConversationMessage` に写す |
| Scope | `runtime/context/scope.d/01.rs` | `ScopeSnapshot{status, scopes[]}`, `load(connection, run_id)`, `resolve(...)` |
| chat_completions 送信 | `providers/chat_completions/mod.rs` 33 行 | `run_with_options(endpoint, authorization, model, history, timeout_ms, context, mode, options)`。`world_body::build_messages(history, &context)`。同一ターンの Tool ループは `messages` に append |
| chat_completions chunk | `providers/chat_completions/chunks.rs` 61 行 | `choices` 空かつ `usage` あり → `Ok(String::new())` で **捨てている** |
| AgentSession 送信 | `providers/agent_session/sse/request.rs` | `render_turn_input(history)` → `{"input":[{"type":"text","text": <JSON>}]}`。usage なし |
| 直接 Tool 一覧 | `providers/stream/agent_dispatch.rs` 15–93 行 | `available_agent_tools(...)`。`calls_this_attempt < 12`。discovery 時は 3 入口、非 discovery 時は `gc_*` |
| Tool 実行 | `providers/stream/agent_dispatch.rs` 101 行 | `execute_agent_tool(output_persistence, input, call, timeout, generated, run_cancellation) -> String` |
| Web 取得 | `runtime/web_fetch/mod.rs` 124 行、`content.rs` | `execute_with_cancel(...)`。`FetchContentResult`、`fetch_content_result` JSON、`truncate_to_model_max`（文字数） |
| Tool Selection 3 入口 | `tool_selection/gateway.d/01.rs`, `gateway_schemas.rs` | `tools_search(intent, limit 1..8)`, `tools_describe(candidateRef \| resultRef+page)`, `tools_invoke(executionRef, arguments)` |
| Tool 参照 | `tool_selection/references.rs` | `ReferenceStore`、TTL 10 分、run/conversation 単位 |
| Catalog 登録 | `tool_selection/catalog.rs` 84 行 | `register_revision(connection, principal_id, source_id, &CatalogEntry, revision_id, created_at)`。`CatalogEntry{tool_id, backend_key, title, purpose, operations, objects, suitable, unsuitable, required_inputs, input_schema, output_schema, effect, usage_pages, backend_binding}` |
| Grant | `tool_selection/repository.d/01.rs` 232 行 | `upsert_grant(connection, principal_id, tool_id, scope_kind, scope_id)` |
| Backend | `tool_selection/backends/mod.rs` 91 行 | `trait ToolBackend { async fn invoke(&self, BackendRequest, &RunCancellation) -> BackendOutcome }`。`BackendRequest{call_id, tool_id, revision_id, backend_key, binding, arguments, timeout, origin, actor}` |
| Backend 振分 | `tool_selection/backends/router.rs`, `mcp/wiring.rs::assemble` | `BackendRouter::new(llang, mcp)`、`kind(binding)` は `mcp_http`/`llang`/`unknown` |
| MCP 大結果 | `tool_selection/mcp/results.rs`, `mcp/schema.rs` | `store_result(...)`, `read_page(...)`。表 `tool_selection_mcp_results`（TTL 10 分、1 MiB） |
| Service | `tool_selection/service.d/02.rs` 248 行、`03.rs` | `search`, `describe`, `invoke`, `describe_result`, `ensure_principal(&writer)` |
| 会話 FTS | `memory/recall/mod.d/01.rs` 351 行 | `conversation_messages_fts` (`tokenize='trigram'`)。`recall/search.rs::search_candidates` |
| Forget | `memory/personal_state/journal.rs`, `personal_state/schema.sql` | `.forget.json`（DB 外）、`personal_tombstones`、trigger 連鎖、起動時 `recover`。journal 無しでは DB を開けない |
| Backup | `database_backup.rs` | `backup_connection_to`。journal は同梱されない |
| テスト用 AppState | `test_support.rs` | `app_state(connection) -> AppState` |
| module size | `scripts/module-size.ts` | Rust production 行 1,600 上限、baseline +10%。`foo.rs` + `foo.d/01.rs` 分割規約 |

## 2. 現状からの差分見積

| 領域 | 種別 | 規模（production 行、概算） | 主な変更点 |
| --- | --- | --- | --- |
| usage/費用計測 | 新規 + 小変更 | 新規 ~350、変更 ~40 | 表 `generation_usage`、`chunks.rs` の usage 捕捉、`GenerationHandle::complete_usage` |
| records store | 新規 | ~1,400 | schema v39、`records/{schema,write,read,auth,outline,fts,forget}.rs` |
| Tool 結果の record 化 | 変更 | ~300 | `web_fetch` の 2 Tool と `execute_agent_tool` の出力を ref+outline に置換 |
| `read_record` / `recall_activity` | 新規 | ~600 | 直接 Tool 定義 + `RecordsBackend` + catalog 登録 |
| Segment（F/L/D） | 新規 + 中変更 | 新規 ~1,500、変更 ~150 | 表 `context_segments/context_entries`、`SegmentBuilder`、`compose_after_connect` の分岐、chat_completions の予算判定 |
| forget 波及 | 変更 | ~300 | `forget_personal_source` から records/segments の失効、journal 拡張 |
| adapter 契約・指標 | 新規 | ~200 | `AdapterContract` 定数、`diagnostics.rs` への集計出力 |
| Frontend | なし | 0 | 本計画では UI 変更なし（診断 JSON export に指標が乗るだけ） |

合計 新規 ~4,300 行、変更 ~800 行。1,600 行上限があるため、各 module は最初から `*.d/01.rs` 分割前提で置く。

## 3. Phase 構成

| Phase | 内容 | カード | 依存 |
| --- | --- | --- | --- |
| P0 | 準備・baseline | CW-00〜CW-02 | — |
| P1 | usage/費用の保存 | CW-10〜CW-14 | P0 |
| P2 | records store（保存・読取・認可・FTS・outline） | CW-20〜CW-27 | P0 |
| P3 | Tool 結果の record 化と `read_record` / `recall_activity` | CW-30〜CW-36 | P2 |
| P4 | Segment（F/L/D）for chat_completions | CW-40〜CW-46 | P1, P3 |
| P5 | forget 波及・backup・adapter 契約・指標 | CW-50〜CW-55 | P2, P4 |

P1 と P2 は独立なので並行可。P3 は P2 の後。P4 は P3 の `tool_round` entry が必要なので P3 の後。

## 4. 設計詳細

### 4-1. module 構成（新規）

```
src-tauri/src/records/
  mod.rs          pub(crate) mod schema; contract; write; read; auth; outline; fts; forget; tools; backend; catalog;
  README.md       所有・不変条件・検索アンカー（10 行以内）
  schema.rs       DDL 文字列 + migrate(connection) （v39）
  contract.rs     Record / RecordKind / Origin / CaptureState / Representation / RecordRef 型、JSON 変換
  write.rs        begin_record / append_chunk / commit_record / abort_record（stream 対応）
  read.rs         read_range / list_activity / search_in_record
  auth.rs         Authorization{principal_id, conversation_id, allowed_scope_keys} と SQL 断片生成
  outline.rs      決定的 outline（最大 20 項目、2,048 bytes）
  fts.rs          record_text_chunks / record_fts の同期更新
  forget.rs       tombstone + 依存閉包 + blob ref_count
  tools.rs        直接 Tool 定義 read_record / recall_activity（OpenAI function 形式）と実行
  backend.rs      RecordsBackend: ToolBackend（tools_invoke 経由の実行）
  catalog.rs      CatalogEntry 2 件と ensure_registered(connection, principal_id)
src-tauri/src/runtime/context/segment/
  mod.rs          pub(crate) mod schema; manifest; entries; builder; carry; dynamic; triggers;
  schema.rs       DDL 文字列（context_segments / context_entries / context_generation_records）
  manifest.rs     SegmentManifest 型、load_active / create / close
  entries.rs      ContextEntry 型、append_conversation / append_tool_round、render
  builder.rs      SegmentBuilder::build(...) -> SegmentEnvelope{history: Vec<ConversationMessage>, manifest_ids, bytes}
  carry.rs        Carry 型と build_carry（決定的）
  dynamic.rs      D ブロックの render（時刻、Scope、Tool 残回数、省略通知、Personal/World 選択結果）
  triggers.rs     needs_rebuild(...) -> Option<RebuildReason>
src-tauri/src/runtime/context/usage.rs   generation_usage の記録（P1）
src-tauri/src/providers/adapter_contract.rs   AdapterContract 定数（P5）
```

`lib.rs` には `mod records;` の 1 行のみ追加。`runtime/context/mod.rs` に `pub(crate) mod segment; pub(crate) mod usage;`。

### 4-2. P1: usage 保存

表（`runtime/context/schema.rs` に追加、v39 の一部）:

```sql
CREATE TABLE IF NOT EXISTS generation_usage (
  generation_id TEXT PRIMARY KEY REFERENCES context_generations(id) ON DELETE CASCADE,
  provider_id TEXT NOT NULL,
  model TEXT,
  input_tokens INTEGER,            -- NULL = 未提供
  cache_read_tokens INTEGER,
  cache_write_tokens INTEGER,
  output_tokens INTEGER,
  reasoning_tokens INTEGER,
  usage_source TEXT NOT NULL CHECK(usage_source IN ('provider','missing','disconnected')),
  raw_usage_json TEXT CHECK(raw_usage_json IS NULL OR json_valid(raw_usage_json)),
  wire_bytes INTEGER NOT NULL CHECK(wire_bytes >= 0),
  prefix_match_bytes INTEGER,      -- P4 で埋める。前 generation との先頭一致 bytes
  ttft_ms INTEGER,
  first_visible_ms INTEGER,
  completed_ms INTEGER,
  recorded_at INTEGER NOT NULL
);
```

型（`runtime/context/usage.rs`）:

```rust
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ProviderUsage {
    pub(crate) input_tokens: Option<u64>,
    pub(crate) cache_read_tokens: Option<u64>,
    pub(crate) cache_write_tokens: Option<u64>,
    pub(crate) output_tokens: Option<u64>,
    pub(crate) reasoning_tokens: Option<u64>,
    pub(crate) raw: Option<serde_json::Value>,
}
pub(crate) enum UsageSource { Provider, Missing, Disconnected }
pub(crate) struct UsageTimings { pub ttft_ms: Option<u64>, pub first_visible_ms: Option<u64>, pub completed_ms: Option<u64> }
pub(crate) fn parse_openai_usage(value: &serde_json::Value) -> ProviderUsage;  // prompt_tokens, prompt_tokens_details.cached_tokens, completion_tokens, completion_tokens_details.reasoning_tokens
pub(crate) fn record(connection: &Connection, generation_id: &str, provider_id: &str, model: Option<&str>, usage: &ProviderUsage, source: UsageSource, wire_bytes: usize, timings: &UsageTimings) -> Result<(), String>;
```

`chunks.rs` 61 行の分岐で `usage` を捨てず、`ChunkAccumulator`（該当 struct 名は実装時に確認）に `pub(crate) usage: Option<ProviderUsage>` を持たせて保持する。`run_with_options` の終端で `GenerationHandle` の id を使って `usage::record` を呼ぶ。切断・timeout で終わった場合は `UsageSource::Disconnected`、usage が来なかった場合は `Missing`。AgentSession は常に `Missing`。

費用（円/ドル）計算は P1 では **行わない**。価格表が無いため `null` とし、設計書 F 章の「未知なら null」に従う。cache 率は `diagnostics.rs` の export で `sum(cache_read_tokens) / sum(input_tokens)` を Provider 別に出す（CW-14）。

### 4-3. P2: records store

DDL（`records/schema.rs`、v39）。時刻は UTC ms INTEGER、ID は TEXT。

```sql
CREATE TABLE IF NOT EXISTS records (
  id TEXT PRIMARY KEY,
  kind TEXT NOT NULL CHECK(kind IN ('web_search','web_search_result','web_fetch','tool_result','mcp_result','conversation_message_ref')),
  origin TEXT NOT NULL CHECK(origin IN ('user_statement','external_observation','runtime_state','derived_claim')),
  principal_id TEXT NOT NULL,
  conversation_id TEXT NOT NULL,
  run_id TEXT,
  turn_id TEXT,
  parent_execution_id TEXT,        -- 検索結果→検索実行、fetch→fetch call
  rank INTEGER CHECK(rank IS NULL OR rank >= 1),
  observed_at INTEGER NOT NULL,
  recorded_at INTEGER NOT NULL,
  locator_json TEXT NOT NULL CHECK(json_valid(locator_json)),   -- {"url":..,"tool":..,"call_id":..,"message_id":..}
  capture_state TEXT NOT NULL CHECK(capture_state IN ('streaming','complete','partial','failed')),
  capture_reason TEXT,
  version INTEGER NOT NULL DEFAULT 1,
  existing_source_locator TEXT,    -- conversation_messages.id 等。本文複製禁止
  forget_epoch INTEGER             -- NULL = 有効
);
CREATE INDEX IF NOT EXISTS idx_records_run ON records(run_id, recorded_at, id);
CREATE INDEX IF NOT EXISTS idx_records_turn ON records(turn_id, id);
CREATE INDEX IF NOT EXISTS idx_records_kind ON records(kind, recorded_at, id);
CREATE INDEX IF NOT EXISTS idx_records_parent ON records(parent_execution_id, rank, id);
CREATE INDEX IF NOT EXISTS idx_records_conversation ON records(principal_id, conversation_id, recorded_at, id);

CREATE TABLE IF NOT EXISTS record_scopes (
  record_id TEXT NOT NULL REFERENCES records(id) ON DELETE CASCADE,
  scope_key TEXT NOT NULL,
  relation TEXT NOT NULL DEFAULT 'visible',
  policy_revision INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY(record_id, scope_key, relation)
);
CREATE INDEX IF NOT EXISTS idx_record_scopes_key ON record_scopes(scope_key, record_id);

CREATE TABLE IF NOT EXISTS blobs (
  id TEXT PRIMARY KEY,
  dedup_domain TEXT NOT NULL,      -- principal_id
  sha256 TEXT NOT NULL CHECK(length(sha256) = 64),
  codec TEXT NOT NULL CHECK(codec IN ('identity')),
  raw_bytes INTEGER NOT NULL CHECK(raw_bytes >= 0),
  stored_bytes INTEGER NOT NULL CHECK(stored_bytes >= 0),
  data BLOB,                        -- 65_536 bytes 以下なら inline。超えたら NULL で blob_chunks
  ref_count INTEGER NOT NULL CHECK(ref_count >= 0),
  UNIQUE(dedup_domain, sha256)
);
CREATE TABLE IF NOT EXISTS blob_chunks (
  blob_id TEXT NOT NULL REFERENCES blobs(id) ON DELETE CASCADE,
  sequence INTEGER NOT NULL CHECK(sequence >= 0),
  raw_offset INTEGER NOT NULL CHECK(raw_offset >= 0),
  raw_bytes INTEGER NOT NULL CHECK(raw_bytes > 0),
  codec TEXT NOT NULL CHECK(codec IN ('identity')),
  data BLOB NOT NULL,
  sha256 TEXT NOT NULL CHECK(length(sha256) = 64),
  PRIMARY KEY(blob_id, sequence)
);
CREATE TABLE IF NOT EXISTS record_representations (
  record_id TEXT NOT NULL REFERENCES records(id) ON DELETE CASCADE,
  name TEXT NOT NULL CHECK(name IN ('received_body','readable_text','structured_json')),
  blob_id TEXT NOT NULL REFERENCES blobs(id),
  sha256 TEXT NOT NULL CHECK(length(sha256) = 64),
  byte_length INTEGER NOT NULL CHECK(byte_length >= 0),
  parser_version TEXT NOT NULL,
  PRIMARY KEY(record_id, name)
);
CREATE TABLE IF NOT EXISTS record_dependencies (
  derived_record_id TEXT NOT NULL,
  source_record_id TEXT NOT NULL REFERENCES records(id),
  representation TEXT NOT NULL,
  start_byte INTEGER NOT NULL CHECK(start_byte >= 0),
  end_byte INTEGER NOT NULL CHECK(end_byte >= start_byte),
  source_hash TEXT NOT NULL,
  dependency_kind TEXT NOT NULL CHECK(dependency_kind IN ('rendering','citation','outline','tool_round')),
  PRIMARY KEY(derived_record_id, source_record_id, representation, start_byte, end_byte)
);
CREATE INDEX IF NOT EXISTS idx_record_dependencies_source ON record_dependencies(source_record_id, derived_record_id);
CREATE TABLE IF NOT EXISTS record_text_chunks (
  id INTEGER PRIMARY KEY,
  record_id TEXT NOT NULL REFERENCES records(id) ON DELETE CASCADE,
  representation TEXT NOT NULL,
  start_byte INTEGER NOT NULL,
  end_byte INTEGER NOT NULL,
  text TEXT NOT NULL,
  index_version INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_record_text_chunks_record ON record_text_chunks(record_id, representation, start_byte);
CREATE VIRTUAL TABLE IF NOT EXISTS record_fts USING fts5(
  text, content='record_text_chunks', content_rowid='id', tokenize='trigram'
);
CREATE TABLE IF NOT EXISTS record_tombstones (
  record_id TEXT PRIMARY KEY,
  forgotten_at INTEGER NOT NULL,
  forget_epoch INTEGER NOT NULL,
  reason_code TEXT NOT NULL
);
```

`record_fts` は external-content なので、`record_text_chunks` への INSERT/DELETE と同じ transaction で `INSERT INTO record_fts(rowid, text)` / `INSERT INTO record_fts(record_fts, rowid, text) VALUES('delete', ?, ?)` を手で発行する（`fts.rs`）。trigger は使わない（削除時に旧 text が必要で、`forget.rs` の順序制御と衝突するため）。

主要 API（`records/write.rs`）:

```rust
pub(crate) struct NewRecord<'a> {
    pub kind: RecordKind, pub origin: Origin,
    pub principal_id: &'a str, pub conversation_id: &'a str,
    pub run_id: Option<&'a str>, pub turn_id: Option<&'a str>,
    pub parent_execution_id: Option<&'a str>, pub rank: Option<u32>,
    pub observed_at: i64, pub locator: serde_json::Value,
    pub scope_keys: &'a [String],
}
/// 本文を一括保存。received_body を必ず保存し、readable_text があれば同時に保存・索引する。
pub(crate) fn commit(connection: &Connection, new: NewRecord<'_>, received_body: &[u8], readable_text: Option<&str>) -> Result<RecordRef, String>;
/// stream 用。begin → append_chunk (複数) → finish(complete|partial)。abort は capture_state='failed' で確定する。
pub(crate) fn begin(connection: &Connection, new: NewRecord<'_>) -> Result<StreamingRecord, String>;
pub(crate) fn append_chunk(connection: &Connection, rec: &mut StreamingRecord, bytes: &[u8]) -> Result<(), String>;
pub(crate) fn finish(connection: &Connection, rec: StreamingRecord, state: CaptureState, readable_text: Option<&str>) -> Result<RecordRef, String>;
pub(crate) struct RecordRef { pub id: String, pub sha256: String, pub bytes: u64, pub outline: Option<Outline> }
```

blob の判定: `received_body.len() <= 65_536` なら `blobs.data` に inline、超えたら 65,536 bytes ごとに `blob_chunks`。`readable_text` の索引は `records/fts.rs::index_text(connection, record_id, "readable_text", text)`。8 KiB（UTF-8 境界）ごとに chunk、境界に直前 2 文字の overlap。1 MiB を超える部分は索引しない（`index_version` に負値ではなく、`records` 側には `capture_reason = "index_truncated"` を残さず、`read.rs::coverage()` が `index_state: "partial"` を返す根拠として `record_text_chunks` の最終 `end_byte < byte_length` を使う）。

読取 API（`records/read.rs`）:

```rust
pub(crate) struct ReadRange { pub start: u64, pub max_bytes: u32 }   // max_bytes <= 8_192
pub(crate) struct ReadResult { pub record_id: String, pub representation: String, pub sha256: String, pub actual_start: u64, pub actual_end: u64, pub text: String, pub capture_state: CaptureState, pub source_refs: Vec<String>, pub truncated: bool }
pub(crate) fn read_range(connection: &Connection, auth: &Authorization, record_id: &str, representation: &str, range: ReadRange) -> Result<Option<ReadResult>, String>;  // None = unavailable
pub(crate) fn search_in_record(connection: &Connection, auth: &Authorization, record_id: &str, query: &str, limit: u8) -> Result<Option<Vec<SearchHit>>, String>;  // SearchHit{start_byte, end_byte, snippet}
pub(crate) struct ActivityQuery { pub kinds: Vec<RecordKind>, pub run_id: Option<String>, pub parent_id: Option<String>, pub rank: Option<u32>, pub before_record_id: Option<String>, pub since_ms: Option<i64>, pub until_ms: Option<i64>, pub query: Option<String>, pub limit: u8, pub cursor: Option<String> }
pub(crate) fn list_activity(connection: &Connection, auth: &Authorization, q: ActivityQuery) -> Result<ActivityPage, String>;  // ActivityPage{items, next_cursor, coverage}
```

認可（`records/auth.rs`）は §0-3 #5 の式を `WHERE` 句として返す関数 `auth.sql_filter(alias) -> (String, Vec<rusqlite::types::Value>)` に集約し、`read.rs` のすべての SELECT が LIMIT の前でこれを使う。

outline（`records/outline.rs`）:

```rust
pub(crate) struct Outline { pub parser_version: &'static str, pub items: Vec<OutlineItem>, pub bytes: usize }
pub(crate) struct OutlineItem { pub start_byte: u64, pub text: String }
pub(crate) fn build(content_type: OutlineKind /* Html, Markdown, Code, Json, Plain */, readable_text: &str) -> Outline;
```

規則: Markdown/HTML は `^#{1,6} ` または `<h1-6>` 由来の行とその直後の 1 文、Code/JSON は深さ 0〜1 の `{`/`}`/`fn `/`class ` 行、Plain は空行区切り段落の先頭 80 文字。最大 20 項目、合計 2,048 bytes。UTF-8 境界で切る。

### 4-4. P3: Tool 結果の record 化と 2 つの Tool

**Web 取得の保存点**: `execute_agent_tool` 内で `web_fetch::execute_with_cancel` の戻りを受けた直後。`output_persistence` から `sqlite_writer` と `principal`（`ensure_principal`）を得る。保存順序は **保存 → モデルへ返す**。保存に失敗したら Tool 結果を `{"type":"tool_error","reason":"record_store_failed"}` に置き換え、本文をモデルへ渡さない。

- `web_search`: 検索実行を `kind='web_search'` 1 件（`received_body` = 応答 JSON 全体、`locator = {"query": ..}`）、結果ごとに `kind='web_search_result'`（`rank` 1 始まり、`parent_execution_id` = 検索 record id、`received_body` = その結果の JSON）。モデルへの出力は既存 `web_search_result` JSON に `"recordId"` と `"searchRecordId"` を追加。
- `fetch_content`: `kind='web_fetch'`、`received_body` = 取得本文（sidecar が返した生テキスト。HTML 生 bytes が取れない現行実装では `readable_text` と同一でよい。その場合 `parser_version = "same-as-received"`）。モデルへの出力は §D 表に従い、8,192 bytes 以下なら本文 + `recordId`、超えたら `{"type":"fetch_content_result","recordId":..,"bytes":..,"sha256":..,"outline":[..],"readHint":"read_record"}`。既存 `maxCharacters` 引数は受け付けるが 8,192 bytes 上限で頭打ちにする。
- MCP 結果（`tools_invoke` の大結果）: `mcp/results.rs::store_result` の直後に `kind='mcp_result'` で保存し、`tool_selection_mcp_results` 行に `record_id` 列（v39 で追加）を書く。既存 `resultRef` ページングはそのまま。TTL 後は `read_record` で読める。
- 直接 Tool の結果（coding/steward/generative_ui 等）は本計画では record 化しない（8 KiB 以下が多く、優先度が低い）。

**Tool 定義**（`records/tools.rs`、`available_agent_tools` に常時追加。12 回制限の対象。`recall_conversation` と同様に常時オファー）:

```json
{"name":"read_record","parameters":{"type":"object","properties":{
  "id":{"type":"string"},
  "representation":{"type":"string","enum":["readable_text","received_body"],"default":"readable_text"},
  "range":{"type":"object","properties":{"start":{"type":"integer","minimum":0},"maxBytes":{"type":"integer","minimum":1,"maximum":8192}}},
  "query":{"type":"string","maxLength":1024}},
 "required":["id"]}}
{"name":"recall_activity","parameters":{"type":"object","properties":{
  "query":{"type":"string","maxLength":1024},
  "kinds":{"type":"array","items":{"type":"string","enum":["web_search","web_search_result","web_fetch","tool_result","mcp_result"]}},
  "time":{"type":"object","properties":{"preset":{"type":"string","enum":["this_turn","this_run","recent","all"]},"beforeRecordId":{"type":"string"}}},
  "parentId":{"type":"string"},"rank":{"type":"integer","minimum":1},
  "limit":{"type":"integer","minimum":1,"maximum":20,"default":10},"cursor":{"type":"string"}}}}
```

`range` と `query` は排他（両方あれば `{"status":"unavailable","reason":"range_and_query_exclusive"}`）。応答 envelope は設計書 C 章の共通出力（`status, items, refs, scope, observed_at, uncertainty, truncated, coverage, next, snapshot_id, reused_from, instruction_authority`）。envelope 全体 8,192 bytes 上限。超えたら `text` を短縮し `truncated: true` と `next`（同 id・次 start）を返す。

**catalog 登録**（`records/catalog.rs`）: 同じ 2 操作を `CatalogEntry`（`backend_key = "records"`, `backend_binding = {"kind":"records","operation":"read_record"|"recall_activity"}`, `effect = "read"`）として起動時に `register_revision` + `upsert_grant(principal, tool_id, "principal", principal)`。revision_id は `input_schema` の sha256 から決定的に作り、変化がない限り再登録しない。`BackendRouter::kind` に `Some("records") => "records"` を追加し、`BackendRouter` を 3 backend 対応に拡張（`new(llang, mcp, records)`）。`RecordsBackend::invoke` は `BackendRequest.actor`（principal/conversation/run）から `Authorization` を作り `tools.rs` の実行関数を呼ぶ。

### 4-5. P4: Segment（F/L/D）

**適用条件**: `state.context_segments_enabled && budget は openai_compatible`（AgentSession は対象外）。`compose_after_connect` の先頭で分岐し、Segment 経路は `segment::builder::build` が `FreshProviderContext` を返す。既存経路は無変更。

DDL（`runtime/context/segment/schema.rs`、v39）:

```sql
CREATE TABLE IF NOT EXISTS context_segments (
  id TEXT PRIMARY KEY,
  conversation_id TEXT NOT NULL,
  previous_segment_id TEXT,
  start_reason TEXT NOT NULL,        -- initial|budget|scope_change|forget|policy_version|task_switch|manual
  scope_snapshot_json TEXT NOT NULL CHECK(json_valid(scope_snapshot_json)),
  policy_version TEXT NOT NULL,      -- CONTEXT_POLICY + conversation-respond.txt の sha256
  bootstrap_tool_schema_digest TEXT NOT NULL,  -- F に含む Tool 定義 JSON の sha256
  renderer_version INTEGER NOT NULL,
  adapter_contract_version INTEGER NOT NULL,
  fixed_render_blob_id TEXT NOT NULL REFERENCES blobs(id),
  carry_record_id TEXT,
  last_entry_sequence INTEGER NOT NULL DEFAULT 0,
  input_budget INTEGER NOT NULL,
  forget_epoch INTEGER NOT NULL,
  created_at INTEGER NOT NULL,
  status TEXT NOT NULL CHECK(status IN ('active','closed','invalidated'))
);
CREATE INDEX IF NOT EXISTS idx_context_segments_active ON context_segments(conversation_id, status, created_at);
CREATE TABLE IF NOT EXISTS context_entries (
  segment_id TEXT NOT NULL REFERENCES context_segments(id) ON DELETE CASCADE,
  sequence INTEGER NOT NULL CHECK(sequence >= 1),
  role TEXT NOT NULL CHECK(role IN ('carry','user','assistant','tool_round')),
  record_id TEXT,                    -- conversation_messages.id または records.id
  rendered_blob_id TEXT NOT NULL REFERENCES blobs(id),
  serializer_version INTEGER NOT NULL,
  content_digest TEXT NOT NULL,
  dependency_refs_json TEXT NOT NULL CHECK(json_valid(dependency_refs_json)),
  created_at INTEGER NOT NULL,
  PRIMARY KEY(segment_id, sequence)
);
CREATE TABLE IF NOT EXISTS context_generation_records (
  generation_id TEXT NOT NULL REFERENCES context_generations(id) ON DELETE CASCADE,
  record_id TEXT NOT NULL,
  usage_kind TEXT NOT NULL CHECK(usage_kind IN ('exposed','claimed','citation','dynamic')),
  range_start INTEGER, range_end INTEGER,
  digest TEXT,
  selected INTEGER NOT NULL CHECK(selected IN (0,1)),
  reason TEXT,
  PRIMARY KEY(generation_id, record_id, usage_kind)
);
```

`context_generations` に列追加（v39）: `segment_id TEXT`, `entry_range_start INTEGER`, `entry_range_end INTEGER`, `dynamic_digest TEXT`, `wire_digest TEXT`, `wire_bytes INTEGER`, `omission_json TEXT`。

**build の手順**（`builder.rs`）:

1. `manifest::load_active(connection, conversation_id)`。無ければ `create(start_reason="initial")`。
2. `triggers::needs_rebuild(&manifest, &now_inputs)` を評価。`Some(reason)` なら `manifest::close(old)` → `create(start_reason=reason, previous_segment_id=old.id, carry=carry::build(...))`。同一 generation で再構成は 1 回。
3. 前ターンの未追記分を L に追記: `entries::append_conversation`（前ターンの user/assistant を `conversation_messages` から。`existing_source_locator` に message id）、`entries::append_tool_round`（前ターンで完了した Tool 往復。P3 の record を参照）。追記は `last_entry_sequence` 以降の差分のみ。
4. F を `fixed_render_blob_id` から読み、`policy_version` と `bootstrap_tool_schema_digest` を現在値と比較。違えば手順 2 の再構成に戻る（1 回まで）。
5. D を `dynamic::render(...)` で生成（時刻 UTC + 表示 timezone、Scope key、Tool 残回数 `12 - calls_this_attempt`、省略通知、Personal/World の選択結果）。Personal/World の選択は既存 `broker::compose` の候補選択ロジックを **そのまま呼び**、その出力 `[PERSONAL_STATE]...` を D の一部として扱う。
6. 予算: `B = budget.usable_context_bytes()`。F ≤ min(12,288, 25%B)、carry ≤ min(8,192, 15%B)、D ≤ min(8,192, 20%B)、U は無切り詰め。L は `B − F − D − U − wrapper` の残量に、**新しい entry から** 入る分だけ選ぶ（古い entry を省略）。省略があれば D の省略通知に `omitted_count` と `earliest_ref`。
7. F/D/carry/U の Must 合計が B を超えたら `Err("required_context_overflow: segment must exceeds budget")`。
8. `history: Vec<ConversationMessage>` を `[system(F), carry?, L entries..., assistant(D), user(U)]` の順で組み、`FreshProviderContext` を返す。`compose_provider_history` は使わない（F に system 結合を含めるため）。

**F の内容**: `render_conversation_system_context(...)` の出力のうち `input_origin` / `presentation_mode` を除いたもの + CONTEXT_POLICY + 直接 Tool 定義のうち常設分（`read_record`, `recall_activity`, `recall_conversation`, 3 入口）の名前と 1 行説明。`input_origin` / `presentation_mode` は D に移す（受入「音声/文字の切替で F/L の bytes が変わらない」のため）。F は Segment 作成時に一度 render して `blobs` に保存し、以後は読むだけ。

**再構成トリガー**（`triggers.rs`）: (1) 次送信の見積 `F+L_all+D+U > 0.70 * B` かつ省略可能 entry がある → `budget`。(2) `scope_snapshot_json` と現在 Scope の差、`policy_version` 差、`bootstrap_tool_schema_digest` 差、`forget_epoch` 差 → 即時。(3) 明示的タスク完了/切替は v1 では検出手段が無いため **実装しない**（設計書 B-3 は将来）。(4) TTL は見ない。再構成後の目標 `F+L ≤ 0.45 * B` は carry のみで開始するため自動的に満たされる。同一 conversation で連続 2 generation とも `budget` 理由で再構成した場合は 2 回目を行わず省略で済ませ、`omission_json` に `"rebuild_suppressed": true` を残す。

**Tool follow-up との関係**: 同一ターン内の Tool ループは既存どおり `messages` に append する（`run_with_options` 内）。Segment の L 追記はターン境界（次の `compose_after_connect`）でのみ行う。`trim_optional_history_for_tool_follow_up` は Segment 経路でも残すが、削除対象を L entry の古い側に限定するよう `history` の `id` 接頭辞（`segment-entry-{sequence}`）で判別する。

**prefix 一致計測**: `usage::record` の `prefix_match_bytes` に、前 generation の `wire` bytes と今回の共通接頭辞長を入れる。前 generation の wire 本文は保存していないので、`GenerationHandle` に前回の wire を **メモリ上**（`AppState` の `LruCache<conversation_id, Vec<u8>>` 相当、上限 4 conversation）で持つ。これは計測専用で永続化しない。

### 4-6. P5: forget 波及・backup・契約・指標

- `forget_personal_source`（`personal_state/commands.rs`）の transaction 内に `records::forget::forget_by_conversation_messages(connection, &message_ids, epoch)` と `segment::manifest::invalidate_for_conversation(connection, conversation_id, epoch)` を追加。手順は設計書 E 章の順序に従う: tombstone → `record_scopes` 削除 → FTS 削除（旧 text を先に読む）→ `record_text_chunks` 削除 → `record_representations` 削除 → `blobs.ref_count -= 1`、0 なら blob/chunks 削除 → `record_dependencies` の派生（rendering = `context_entries`）を持つ Segment を `invalidated`。
- 会話単位の forget に加え、record 単位の `forget_record(connection, record_id, reason)` を用意する（UI からの呼出しは本計画外。テストと将来の IPC 用）。
- `.forget.json` に `records: BTreeMap<record_id, forgotten_at>` を追加し、`recover` で `record_tombstones` にマージ。`personal_source_no_resurrection` と同様の trigger `record_no_resurrection` を追加。
- `PRAGMA secure_delete = ON` を `SqliteWriter::open` の pragmas に追加。FTS5 shadow table については保証しない旨を `records/README.md` に書く。
- `providers/adapter_contract.rs`: `pub(crate) struct AdapterContract { cache_support: CacheSupport /* Verified|Unsupported|Unknown */, history_binding: HistoryBinding /* None|Session */, usage_mapping: UsageMapping /* OpenAiChat|None */, token_capacity: Option<u32>, byte_transport_limit: usize }` と `CHAT_COMPLETIONS: AdapterContract`, `AGENT_SESSION: AdapterContract` の定数。`compose_after_connect` の Segment 分岐条件を `contract.history_binding == None` に置き換える。
- `diagnostics.rs::export_diagnostics` に `"contextMetrics"`: Provider 別 `{generations, cache_read_ratio, usage_missing, avg_wire_bytes, avg_prefix_match_ratio, rebuild_count_by_reason, records_total, records_bytes, db_bytes, db_ratio_of_limit}`。DB 目安上限 10 GiB は定数 `RECORDS_DB_SOFT_LIMIT_BYTES`。80% 超で `diagnosis` に `records.capacity` item（Warn）を追加。

## 5. 作業カード

標準: 実装 1〜3 ファイル + 試験。試験名 `cw_NN_条件`。各カードで `cargo test --manifest-path src-tauri/Cargo.toml <filter>` と `cargo clippy --all-targets -- -D warnings` を通す。schema を触るカードは `bun run size:check` も通す。`DATABASE_SCHEMA_VERSION` は全計画で **2 回だけ** 上げる。1 回目は P1/P2 のどちらか先に着手したカード（CW-10 または CW-20）で 38 → 39、2 回目は CW-41 で 39 → 40。後から着手した側のカードでは上げない。

### P0

| ID | 対象 | 実装すること | 合格条件 |
| --- | --- | --- | --- |
| CW-00 | `spec/evidence/context-window/progress.md`（新規） | HEAD、dirty 差分一覧、`DATABASE_SCHEMA_VERSION` 現在値 38 を記録。以降カード完了ごとに追記 | ファイル存在 |
| CW-01 | 同 evidence | 現行 baseline を取る: 会話 1 本で 10 ターン（Tool 無し）を `bun run tauri dev` で回し、`context_generations.projected_bytes` の推移と、chat_completions の TTFT（`http_metrics`）を記録 | 10 行の表が evidence にある |
| CW-02 | `src-tauri/src/README.md` | module 一覧に `records`（P2 で作成予定）と `runtime/context/segment` の 2 行を「計画中」として追加 | 差分のみ |

### P1: usage 保存

| ID | 対象 | 実装すること | 合格条件 |
| --- | --- | --- | --- |
| CW-10 | `runtime/context/schema.rs`, `persistence/schema.rs` | §4-2 の `generation_usage` DDL を `CREATE TABLE IF NOT EXISTS` 群に追加。`DATABASE_SCHEMA_VERSION = 39` | `cw_10_generation_usage_table_exists`（in-memory `initialize_database` 後に `PRAGMA table_info`）。既存 migrate テスト全通過 |
| CW-11 | `runtime/context/usage.rs`, `runtime/context/mod.rs` | `ProviderUsage`, `UsageSource`, `UsageTimings`, `parse_openai_usage`, `record` | `cw_11_parse_openai_usage_reads_cached_tokens`（`prompt_tokens_details.cached_tokens`）、`cw_11_parse_missing_fields_are_none`、`cw_11_record_inserts_row` |
| CW-12 | `providers/chat_completions/chunks.rs` | 61 行の分岐で `usage` を `self.usage = Some(parse_openai_usage(...))` に保持。struct にフィールド追加 | `cw_12_chunk_with_usage_only_is_retained`（既存 chunk テストの書き方に合わせる） |
| CW-13 | `providers/chat_completions/mod.rs` | `run_with_options` 終端（成功・失敗両方）で `usage::record`。`generation_id` は既存 `GenerationHandle` から取る（`handle.id()` が無ければ `pub(crate) fn id(&self) -> &str` を追加）。TTFT は `request_started` からの経過。切断は `Disconnected` | `cw_13_usage_row_written_on_success`、`cw_13_usage_missing_on_disconnect`（既存 mock server テストの枠を使う） |
| CW-14 | `diagnostics.rs`, `runtime/context/usage.rs` | `usage::summary(connection) -> Vec<ProviderUsageSummary{provider_id, generations, input_tokens, cache_read_tokens, cache_read_ratio: Option<f64>, usage_missing}>`。`export_diagnostics` payload に `"contextMetrics": {"usage": [...]}` | `cw_14_summary_ratio_is_null_when_no_input`、既存 diagnostics テスト通過 |

### P2: records store

| ID | 対象 | 実装すること | 合格条件 |
| --- | --- | --- | --- |
| CW-20 | `records/mod.rs`, `records/schema.rs`, `records/README.md`, `lib.rs`, `persistence/schema.rs` | §4-3 の DDL 全部。`records::schema::migrate(connection)` を `initialize_database` から呼ぶ。（CW-10 未実施なら）version 39 | `cw_20_records_schema_creates_all_tables`（9 表 + FTS）、`cw_20_record_fts_is_trigram`（`SELECT sql FROM sqlite_master`） |
| CW-21 | `records/contract.rs` | `RecordKind`, `Origin`, `CaptureState`, `RecordRef`, `Outline`, `OutlineItem` と `as_str()/parse()`。serde は camelCase | `cw_21_kind_roundtrip` |
| CW-22 | `records/write.rs` | `commit`。blob inline/chunk 分岐（65,536 境界）。dedup は `UNIQUE(dedup_domain, sha256)` に当たったら `ref_count += 1` | `cw_22_commit_small_body_inlines_blob`、`cw_22_commit_large_body_uses_chunks`（200 KiB → 4 chunk、`raw_offset` 連続）、`cw_22_same_body_shares_blob_and_increments_ref_count` |
| CW-23 | `records/write.rs` | `begin/append_chunk/finish`。`streaming` 中は `blobs` ではなく一時表を使わず、`Vec<u8>` をメモリに溜めて `finish` で `commit` と同じ経路へ流す（1 MiB 上限、超過で `partial`） | `cw_23_finish_partial_records_reason`、`cw_23_abort_marks_failed_without_blob` |
| CW-24 | `records/fts.rs` | `index_text`（8 KiB chunk、2 文字 overlap、1 MiB 上限）、`delete_text`（旧 text を読んでから `'delete'` コマンド） | `cw_24_index_chunks_align_utf8`（日本語 100 KiB で境界が文字中でない）、`cw_24_search_hits_after_first_64kib`（後半にだけある語がヒット）、`cw_24_delete_removes_fts_rows` |
| CW-25 | `records/auth.rs`, `records/read.rs` | `Authorization`, `sql_filter`。`read_range`（UTF-8 境界に丸め、`actual_start/actual_end` を返す）、`search_in_record`、`list_activity`（cursor は `recorded_at:id` の base64） | `cw_25_read_range_adjusts_utf8_boundary`、`cw_25_unauthorized_and_missing_are_both_none`、`cw_25_scope_filter_applies_before_limit`（他 scope の 30 件 + 自 scope の 5 件、limit 10 で 5 件）、`cw_25_list_activity_rank_filter`、`cw_25_cursor_is_stable_across_inserts` |
| CW-26 | `records/outline.rs` | `build` の 5 種 | `cw_26_markdown_outline_uses_headings`、`cw_26_plain_outline_paragraph_heads`、`cw_26_outline_caps_20_items_2048_bytes` |
| CW-27 | `records/README.md`, `src-tauri/src/README.md` | 不変条件（保存→公開の順、原典に redact を掛けない、認可は LIMIT 前、FTS は手動同期）を 10 行以内 | レビューのみ |

### P3: Tool 結果の record 化と 2 Tool

| ID | 対象 | 実装すること | 合格条件 |
| --- | --- | --- | --- |
| CW-30 | `providers/stream/agent_dispatch.rs`, `runtime/web_fetch/mod.rs` | `web_search` 結果を record 化（検索 1 + 結果 N）。出力 JSON に `recordId`/`searchRecordId`。保存失敗時は `tool_error` | `cw_30_web_search_persists_execution_and_ranked_results`（fixture 検索で `rank` 1..N、`parent_execution_id` 一致）、`cw_30_store_failure_blocks_output`（read-only 接続で失敗させる） |
| CW-31 | 同上、`runtime/web_fetch/content.rs` | `fetch_content` を record 化し、8,192 bytes 超は outline + ref | `cw_31_fetch_over_8kib_returns_outline_not_body`、`cw_31_fetch_under_8kib_returns_body_with_record_id` |
| CW-32 | `tool_selection/mcp/results.rs`, `mcp/schema.rs`, `records/schema.rs` | `tool_selection_mcp_results.record_id` 列追加（`ALTER TABLE ... ADD COLUMN` を v39 migrate に）。`store_result` 直後に `kind='mcp_result'` 保存 | `cw_32_mcp_result_has_record_id`、既存 mcp tests 全通過 |
| CW-33 | `records/tools.rs` | `read_record` / `recall_activity` の定義と `execute(connection, auth, name, args) -> serde_json::Value`。envelope 8,192 bytes 制限と `next` | `cw_33_read_record_range_and_query_exclusive`、`cw_33_envelope_truncates_with_next`、`cw_33_recall_activity_second_result`（「2 番目」= `parentId` + `rank: 2`） |
| CW-34 | `providers/stream/agent_dispatch.rs` | `available_agent_tools` に 2 定義を常時追加（`recall_conversation` の直後）。`execute_agent_tool` で名前一致時に `records::tools::execute` | `cw_34_records_tools_offered_regardless_of_discovery`、`cw_34_read_record_executes_with_actor_auth` |
| CW-35 | `records/backend.rs`, `records/catalog.rs`, `tool_selection/backends/router.rs`, `mcp/wiring.rs` | `RecordsBackend`、`BackendRouter::new(llang, mcp, records)` と `kind` に `records`、`catalog::ensure_registered` を `assemble` 後の起動処理から呼ぶ | `cw_35_router_dispatches_records_kind`、`cw_35_ensure_registered_is_idempotent`（2 回呼んで revision 1 件）、既存 router テストの `new` 引数を更新 |
| CW-36 | `spec/evidence/context-window/progress.md` | 受入 4 質問の手動確認: Memory OFF で Web 検索 → 「さっきの検索」「2 番目」「当時の本文」「何を根拠に」。新規 Web 検索が走らないこと（`records` に `web_search` が増えない） | evidence に対話ログの要点と `SELECT kind, count(*) FROM records` |

### P4: Segment

| ID | 対象 | 実装すること | 合格条件 |
| --- | --- | --- | --- |
| CW-40 | `app_state.rs`, `lib.d/02.rs`, `test_support.rs` | `context_segments_enabled: bool`（`SAAA_CONTEXT_SEGMENTS == "1"`） | `cargo build`。`rg -n "AppState \{"` で構築箇所 2 つを更新 |
| CW-41 | `runtime/context/segment/{mod,schema,manifest}.rs`, `runtime/context/schema.rs` | §4-5 DDL（v39 に含める。既に 39 なら **40 に上げる**。本カードで 1 回だけ）。`SegmentManifest`, `load_active`, `create`, `close`, `invalidate_for_conversation` | `cw_41_create_and_load_active`、`cw_41_close_then_create_links_previous` |
| CW-42 | `segment/entries.rs` | `ContextEntry`, `append_conversation`, `append_tool_round`, `render(entry) -> ConversationMessage`。rendered 本文は `blobs` に保存し `rendered_blob_id` | `cw_42_append_is_append_only`（既存 sequence の再書込で Err）、`cw_42_tool_round_summary_is_256_bytes_max` |
| CW-43 | `segment/carry.rs`, `segment/dynamic.rs` | §0-3 #4 の carry と §4-5 手順 5 の D | `cw_43_carry_drops_evidence_refs_first`、`cw_43_carry_fails_when_active_operations_exceed`、`cw_43_dynamic_contains_utc_and_tool_budget`、`cw_43_dynamic_carries_input_origin_not_fixed` |
| CW-44 | `segment/triggers.rs`, `segment/builder.rs` | `needs_rebuild`、`build`（手順 1〜8）。Personal/World は既存 `broker::compose` を呼んで結果を D に入れる | `cw_44_first_turn_creates_initial_segment`、`cw_44_fixed_blob_unchanged_across_turns`（3 ターンで `fixed_render_blob_id` と F bytes 不変）、`cw_44_budget_rebuild_at_70_percent`、`cw_44_no_consecutive_budget_rebuild`、`cw_44_must_overflow_returns_required_context_overflow`、`cw_44_voice_text_switch_keeps_fixed_bytes` |
| CW-45 | `runtime/conversation_prepare.rs`, `providers/chat_completions/mod.rs` | `compose_after_connect` 先頭で分岐。`context_generations` の新列（`segment_id`, `entry_range_*`, `wire_digest`, `wire_bytes`, `omission_json`）を `generation::begin` の呼出側で埋める（`BeginGeneration` に `segment: Option<SegmentGenerationMeta>` を追加）。`trim_optional_history_for_tool_follow_up` の対象を `segment-entry-` 接頭辞の id に限定。`usage::record` の `prefix_match_bytes` | `cw_45_segment_path_used_only_for_chat_completions`、`cw_45_agent_session_uses_legacy_path`、`cw_45_prefix_match_bytes_grows_when_prefix_stable`（2 generation 連続で F+L 一致 → `prefix_match_bytes >= F bytes`） |
| CW-46 | evidence, `lib.d/02.rs` | `bun run tauri dev` で flag ON、10 ターン + 大きい fetch 1 回を回し、`generation_usage.prefix_match_bytes / wire_bytes` と `cache_read_tokens` を記録。CW-01 と比較。問題なければ既定 ON（env `SAAA_CONTEXT_SEGMENTS=0` で OFF）に反転 | evidence に比較表。既定反転の commit |

### P5: forget・backup・契約・指標

| ID | 対象 | 実装すること | 合格条件 |
| --- | --- | --- | --- |
| CW-50 | `records/forget.rs` | `forget_record`, `forget_by_conversation_messages`。§4-6 の順序 | `cw_50_forget_removes_fts_and_blob_when_last_ref`、`cw_50_forget_keeps_shared_blob`、`cw_50_forget_invalidates_dependent_segment`、`cw_50_forgotten_record_is_unavailable` |
| CW-51 | `memory/personal_state/commands.rs`, `personal_state/journal.rs`, `records/schema.rs` | `forget_personal_source` から呼出。journal に `records` を追加、`recover` でマージ、`record_no_resurrection` trigger | `cw_51_journal_recover_restores_record_tombstones`、既存 `real_db_backup_restore_merges_current_journal_*` 通過 |
| CW-52 | `persistence/sqlite/writer.rs`, `records/README.md` | `PRAGMA secure_delete = ON` | `cw_52_secure_delete_is_on`（`PRAGMA secure_delete` が 1）。README に FTS shadow の注意 |
| CW-53 | `providers/adapter_contract.rs`, `runtime/conversation_prepare.rs` | `AdapterContract` 定数と分岐条件の置換 | `cw_53_agent_session_declares_session_binding`、CW-45 テスト継続通過 |
| CW-54 | `diagnostics.rs`, `diagnosis/checks/memory.rs` | `contextMetrics` の拡張（rebuild 理由別件数、records 件数/bytes、DB bytes と 10 GiB 比）。`records.capacity` item（80% で Warn） | `cw_54_context_metrics_present`、`cw_54_capacity_warn_at_80_percent`（定数を試験用に注入） |
| CW-55 | `records/README.md`, `runtime/context/README.md`（無ければ作らない）、`src-tauri/src/README.md`, evidence, 設計書 | README 更新。設計書に「§0-2 非目標」「§0-3 判断」を反映した追記。`bun run check:local` | 全通過。evidence に完了記録 |

## 6. カード別の補足（詰まりやすい点）

### CW-10 / CW-20 / CW-41（schema version）

- version を上げるのは全計画で **最大 2 回**（P1/P2 で 39、P4 で 40）。既に上がっていれば上げない。`schema.rs` 冒頭コメントに理由を 1 行追記。
- `CREATE TABLE IF NOT EXISTS` 群に足すだけなら migrate 関数は不要。`ALTER TABLE ... ADD COLUMN`（CW-32、CW-45 の `context_generations` 列追加）は `migrate.d/` に `migrate_v39_to_v40` として書き、`PRAGMA table_info` で列の有無を確認してから実行する（再実行安全）。
- 起動時 `backup_before_migration` が走るため、開発 DB の backups ディレクトリが増える。問題なし。

### CW-22 / CW-23（blob）

- `blobs.data` に inline するか `blob_chunks` に分けるかは `raw_bytes > 65_536` で判定。両方に入れない（`data IS NULL` ⇔ chunks あり）。
- dedup 時に `ref_count` を増やすが、`records` 側は別行。読取は `record_representations.blob_id` 経由のみ。hash から直接読む関数を作らない。
- sha256 は `generated_capabilities::contracts::sha256_hex` を再利用。

### CW-24（FTS external content）

- `INSERT INTO record_fts(rowid, text) VALUES (?, ?)` と `INSERT INTO record_fts(record_fts, rowid, text) VALUES ('delete', ?, ?)`。削除時に text が一致しないと index が壊れるため、必ず `record_text_chunks` から読んでから消す。
- trigram は 3 文字未満の query で 0 件になる。`search_in_record` は `query.chars().count() < 3` のとき `LIKE '%q%'` を `record_text_chunks` に対して **同一 record に限定** して実行（最大 2 MiB 読取、超過は `partial`）。

### CW-25（認可）

- `allowed_scope_keys` は `ScopeSnapshot.scopes.iter().map(|s| s.key.clone())`。`compose_after_connect` に同じ式がある。
- `sql_filter("r")` の返す断片は `r.principal_id = ? AND r.forget_epoch IS NULL AND (NOT EXISTS(SELECT 1 FROM record_scopes s WHERE s.record_id = r.id) AND r.conversation_id = ? OR EXISTS(SELECT 1 FROM record_scopes s WHERE s.record_id = r.id AND s.scope_key IN (?, ?, ...)))`。`IN` の要素数は `allowed_scope_keys.len()`、0 件なら `0=1`。
- cursor に scope を含めない。cursor は `recorded_at:id` のみで、認可は毎回再評価。

### CW-30 / CW-31（保存点）

- `execute_agent_tool` は `output_persistence: Option<...>`。`None`（永続化なし経路）のときは record 化せず、従来出力を返す。テストで `None` のケースを 1 本残す。
- `principal` は `tool_selection::service::ensure_principal(&writer)`（`gc_` 経路と同じ）。
- `turn_id` は現行に無い。`run_id` と `calls_this_attempt` から `format!("{run_id}:{ordinal}")` を作らず、`turn_id = None` にして `run_id` のみ入れる。`recall_activity` の `preset: "this_turn"` は v1 では `this_run` と同じ範囲にし、応答 `coverage.searched_ranges` にその旨を出す。

### CW-34（12 回制限）

- `read_record`/`recall_activity` は `calls_this_attempt < 12` の外で常時オファーする（`recall_conversation` と同じ位置）。ただし `execute_agent_tool` の共通カウンタには含める（設計書「search/describe/invoke もそれぞれ数える」に合わせ、既存カウント方法を変えない）。

### CW-35（Router 3 引数化）

- `BackendRouter::new` の呼出しは `mcp/wiring.rs` と `mcp/tests.d/{01,07,08}.rs` の 4 箇所。全部に `Arc::new(RecordsBackend::new(writer.clone()))` を渡す。
- `assemble` は `manager` が `None` のとき router を作らず `llang` を直接返す。この分岐でも records を使えるようにするため、`None` 側も `BackendRouter::new(llang, Arc::new(UnavailableBackend), records)` にする（`UnavailableBackend` は `BackendOutcome` の既存エラー種で「mcp 未構成」を返す。無ければ `FixtureBackend` を流用せず 10 行で追加）。

### CW-44（builder）

- Personal/World の選択は `broker::compose(BrokerInput{ base, candidates, .. })` に **空の履歴** を持つ `ContextWindow`（system + user のみ）を渡し、返った `messages` から `[PERSONAL_STATE]` の assistant を抜き出して D に入れる。broker 自体は変更しない。
- `world::turn::compose_for_app` は World の revalidate を含むので、Segment 経路でも **呼ぶ**。戻りの `envelope.messages` のうち system と user 以外を D 素材として扱う。
- F のバイト数は `render_conversation_system_context` の出力に依存する。`agent_name`/`user_name`/`regional` が変わったら `policy_version` の sha256 に含めて再構成させる（これらも F の入力にする）。

### CW-45（generation manifest）

- `BeginGeneration` はライフタイム付き struct。構築箇所は **22 箇所**（テスト含む、`rg -n "BeginGeneration \{"`）あるので、フィールド追加ではなく `begin_with_segment(writer, input, meta: SegmentGenerationMeta<'_>)` を **別関数** として追加し、`begin_with_writer` は内部で `meta = None` 相当を呼ぶ形にする。既存呼出しは無変更。
- `GenerationHandle` に `id()` は無い。CW-13 で `pub(crate) fn id(&self) -> &str` を追加する（フィールド名は実装を読んで合わせる）。
- `trim_optional_history_for_tool_follow_up` は `providers/chat_completions/world_trim.rs` にある。
- `prefix_match_bytes` の前回 wire は `AppState` に `Mutex<VecDeque<(String, Vec<u8>)>>`（最大 4 件）で持つ。永続化しない。

### CW-51（journal）

- `journal.rs` の `version` を上げ、旧版 JSON（`records` 無し）を読めるように `#[serde(default)]`。`recover` は `record_tombstones` に `INSERT OR IGNORE`。
- `forget_personal_source` の transaction は既に長い。records 側の削除は `records::forget::forget_by_conversation_messages` 1 関数に閉じ、commands.rs には 2 行だけ足す。

## 7. 検証コマンド

```
cargo test --manifest-path src-tauri/Cargo.toml usage
cargo test --manifest-path src-tauri/Cargo.toml records
cargo test --manifest-path src-tauri/Cargo.toml segment
cargo test --manifest-path src-tauri/Cargo.toml tool_selection      # CW-32, CW-35
cargo test --manifest-path src-tauri/Cargo.toml personal_state      # CW-51
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
bun run size:check
bun run check:local            # CW-36, CW-46, CW-55 で 1 回ずつ
```

手動確認:

- CW-36: Memory OFF、`bun run tauri dev`、「〜を Web で検索して」→「さっきの検索の 2 番目を開いて」→「その本文の後半に○○という語はある？」→「今の回答は何を根拠にした？」。`records` 表の `web_search` 件数が 1 のまま増えないこと。
- CW-46: `SAAA_CONTEXT_SEGMENTS=1 bun run tauri dev`、10 ターン。`SELECT wire_bytes, prefix_match_bytes, cache_read_tokens FROM generation_usage ORDER BY recorded_at` で `prefix_match_bytes` が 2 ターン目以降 F+L 以上であること。音声/文字を切り替えても `context_segments.fixed_render_blob_id` が変わらないこと。

## 8. リスクと対処

| リスク | 対処 |
| --- | --- |
| module size 1,600 行超過 | 新 module は最初から `*.d/01.rs` 分割。`records/read.rs` と `segment/builder.rs` が膨らみやすいので、`read.d/01.rs`（range/search）と `read.d/02.rs`（activity）に分ける |
| `BackendRouter::new` の引数変更で test が一斉に落ちる | CW-35 で 4 箇所を同時に更新。`rg -n "BackendRouter::new"` を合格条件に含める |
| FTS external-content の不整合 | trigger を使わず `fts.rs` の 2 関数だけが `record_fts` を触る。README に明記。`cw_24_delete_removes_fts_rows` で確認 |
| F の bytes が想定外に変わる（agent 名変更、locale 変更） | 変わる入力を全部 `policy_version` の hash に入れ、変化時は正当な再構成として記録する。`cw_44_fixed_blob_unchanged_across_turns` は入力固定で確認 |
| 70% 再構成の連発 | `no_consecutive_budget_rebuild` で 2 回目を抑止。抑止時は省略で対応し `omission_json` に記録 |
| Segment 経路で Personal/World の Must が落ちる | `broker::compose` をそのまま呼ぶことで Must 判定を再実装しない。D 枠に入らないときは既存 `required_context_overflow` で失敗させ、黙って削らない |
| `web_search`/`fetch_content` の出力形式変更でモデル挙動が変わる | `recordId` 等は **追加** のみ。8,192 bytes 超のみ outline 形式。既存 `maxCharacters` は互換維持（上限が下がるだけ） |
| record 化による保存失敗でターンが失敗する | 設計どおり本文をモデルへ渡さず `tool_error` を返す。`output_persistence == None` 経路では従来動作 |
| secure_delete による書込性能低下 | CW-52 は独立カードなので、CW-46 の計測後に p95 を再測し、劣化が 20% を超えるなら OFF に戻して評価を evidence に残す |
| AgentSession 経路に何も効果がない | 明示的に範囲外（§0-2）。`AdapterContract.history_binding = Session` で宣言し、指標には Provider 別で出す |
