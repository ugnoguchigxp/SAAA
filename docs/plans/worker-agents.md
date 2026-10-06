# Worker Agent 基盤と Web Search Worker 実装仕様

作成日: 2026-10-06 / 状態: 設計（コード未変更）/ 対象: `src-tauri/src`, `src`

## 0. 前提

- 本設計は以下を前提とする（外部のコンセプト文書への照合を着手の条件にはしない。前提が変わったらこの節を更新する）。
  - A1: 会話 Agent（現行実装名は `conversation_answer`。モデル名に依存しない呼称とする）の context は会話履歴・Memory・WorldModel・委譲結果の要約だけを持つ。
  - A2: Worker は Memory/WorldModel を更新しない（読み取りもしない）。
  - A3: Worker 定義をユーザーが追加できる（データとして）。
  - A4: クラウド昇格は既定で無効、ユーザー承認制（`purpose-based-cloud-api-switching.md` の「用途別の送信許可」と同じ考え方）。
- 音声契約（AGENTS.md）は変更しない。Worker は ASR/TTS/VoiceProcessingIO の経路に一切触れず、受付の発話は既存の speech lane（`queue_progress::enqueue_search` 経由の progress speech）だけを使う。

## 1. Goal / Non-goals

Goal
1. Worker Agent の定義（目的・SystemContext・事前読込 Skill・ツール集合・モデル階梯・入出力スキーマ・完了条件・上限）を SQLite に置き、コード変更なしで追加・編集・無効化できる。
2. 会話 Agent は「ツールを探す」のではなく「この依頼を扱える Agent があるか」を探し、`delegate` で委譲する。Worker は自分の固定ツール集合だけを使う。
3. 委譲契約: 永続化済みユーザー入力への束縛、同期/非同期、型付き結果、取消、期限、重複排除、再起動時に外部効果を再実行しない、ホスト証拠による検証。
4. 失敗の意味論:「失敗しない」=「失敗を検知し、回復または昇格し、型付きで報告する」。
5. 最初の具体 Worker として Web Search Worker を実装し、`conversation_answer.rs` の inline `web_search`/`fetch_content` ループを feature flag で置き換える。
6. 既存の `fetch_content` guard 判定漏れ（§6.1）を修正する。

Non-goals（v1）
- モデルによる Worker 定義の提案・作成（grant 拡大）。追加は user/host の操作のみ。
- Worker による Memory/WorldModel 更新、書込み系ツール（effect `write`/`unknown`）を持つ Worker の実行。スキーマは表現できるが v1 の実行器は read-only Worker だけを受理する。
- 汎用 DAG/Worker 間の再委譲（Worker は `delegate` を持たない）。
- `role_routing` の rr_roots を 会話エージェント キュー経路へ導入すること。
- 新しい Vector DB や外部 Agent 製品の導入。
- Worker 管理 UI の作り込み（一覧・有効化・承認の最小 UI のみ）。

## 2. 現状（検証済みの事実）

| 項目 | 確認内容 |
| --- | --- |
| 会話ループ | `runtime/conversation_check/queue_runtime/conversation_answer.rs` `process_fixed`（L25-467）。`MAX_TOOL_STEPS = 6`（L106, 関数内 const）。`web_search`（L299-338）/`fetch_content`（L339-375）の結果を `[TOOL_RESULT: …; 未信頼の資料]` として `recent` に積む。`selected_urls` は `search_urls ∪ fetched_urls` との完全一致で絞る（L285-295）。 |
| 会話指示 | `contexts/conversation/queue.context.toml` → 生成物 `.s11tnext/conversation-queue.txt`（`queue_context.rs` L90 で `include_str!`）。`web_search`/`fetch_content` action を記述。 |
| キュー | `src-tauri/src/task_queue.rs`（`task_queue_jobs`、lane/kind/job_key/generation、UNIQUE(scope,kind,job_key,generation)、`claim`/`finish`/`fail`/`fail_terminal`/`recover(replay_lanes, interrupt_lanes)`）。lane は `queue_runtime.rs` `spawn` で `conversation`×4 + `speech`。`process_job` は `terminal_question` kind（有限の背景判断）を前例として持つ。 |
| 入力束縛 | `queue_runtime.rs` `enqueue_text`: user message id = `check_{input_id}`、job_key = `input_id`、run id = `run_{input_id}`。 |
| モデル呼出し | `conversation_check/direct_route.rs`（`prepare_transport`, `transport_for`, `complete_direct_with_events`, `RouteAttempt`）。すべて `pub(super)`。tool なし単発呼出しの前例は `conversation_check/terminal_decision.rs` `decide`。 |
| ルート | `providers/service_registry/{types.rs,resolve.rs}`: `Purpose`（5種）、`PurposeBinding{primary_resource_id, fallback_resource_ids, cloud_allowed, timeout_ms, attempt_timeout_ms}`、`ResolvedRoute{location: "local"|"cloud", fingerprint, …}`。`resolve.rs` L90 で cloud は `cloud_allowed` 必須。 |
| 推論スロット | `memory/personal_state/scheduler/{foreground.rs,background.rs}`: `foreground()` は共有 read lock、`background()` は write lock で foreground 要求時に自ら cancel。 |
| Web tool 実行 | `providers/stream/agent_dispatch.rs` `execute_agent_tool` → `runtime/web_fetch::execute_with_cancel`。web tool は tool_selection catalog には登録されていない（名前で分岐する built-in）。 |
| guard（検索） | `web_fetch/search/match_bracket.rs` L425-461: Deny/RequireApproval の hit は除外、残りは `tainted`。 |
| guard（本文） | `web_fetch/static_content.rs` L250-255: HTML 経路は Deny/RequireApproval で `Ok(None)`。`web_fetch/content.rs` L148 → `content/projection.rs` `project_document`: webview 経路は decision を label 化するだけで本文を保持。`content/projection/compact.rs` `render_compact` は decision に関係なく `document.text` を出力。プラグイン（`LLM-fetch@64afbce` `manager.rs` L473）も decision に関係なく `text` を返す。 |
| 監査 | `queue_runtime/web_result.rs` `audit_web_tool_result`: bytes/hitCount/errorCode/retrievalStatus のみ。`persistence/audit`: 保持 7 日（`AUDIT_RETENTION_DAYS`）、attributes 最大 2,048 bytes。 |
| role_routing | 会話キュー経路（`runtime/conversation_check/**`）から `role_routing` の参照なし（grep 確認）。rr_roots は「会話あたり active root 1 つ」制約。`tools.rs` `classify_effect`/`permits_effect`、`tool_ledger.rs`（reserve→settle、operation_key の冪等性、unknown は再試行しない）は純粋に再利用可能。`tool_specialist.rs` は `offline-contracts` のみ。 |
| tool_selection | `schema.rs`（catalog/revisions/grants/embeddings/decisions/candidates/fts5 trigram/meta epochs）、`retrieval.rs`（`fts_match_query`, `lexical_eligible`, `embedding_candidates`）、`ranking.rs`（`fuse`, `rrf_score`）、`inference.rs`（`EmbeddingProvider` trait）。 |
| steward outbox | `steward/repository/terminal_report.rs` `enqueue_task_report`（`steward_reports.task_id` に FK なし、UNIQUE(task_id,task_revision,destination)）、`steward/report/publish.rs` `flush_held_reports`（speech hold を尊重して assistant message と発話を配信）。 |
| スキーマ | `persistence/schema.rs` `initialize_database`、`DATABASE_SCHEMA_VERSION = 44`。ドメイン schema は単一 transaction 内で `crate::<domain>::schema::migrate(&transaction)` を呼ぶ冪等 `CREATE TABLE IF NOT EXISTS` 方式。 |
| 生成型 | `ts-rs`、`src/lib/generated/*.ts`（例 `diagnosis.ts` 先頭「Generated from … Do not edit.」）。 |
| サイズ上限 | `scripts/module-size.ts`: Rust production 1,600 行、ts 700、tsx 550。 |

## 3. 主要な設計判断

D1. **Worker 定義は新テーブル群 `worker_*`（tool_selection/role_routing を拡張しない）**。
理由: (a) worker_profile は SystemContext・Skill 固定・ツール集合・階梯・完了条件など tool revision と意味が違い、`tool_selection_revisions` に詰めると `effect`/`backend_binding` の意味が壊れる。(b) `tool_selection_sources.kind` の CHECK 拡張は親テーブル再構築（`migrate_sources_kind` 前例、FK 無効化が必要）でリスクが高い。(c) Worker 数は数十規模で、FTS+埋め込みの純関数を再利用すれば十分。再利用するのは `tool_selection::retrieval::{fts_match_query, lexical_eligible, embedding_candidates}`、`tool_selection::ranking::fuse`、`tool_selection::inference::EmbeddingProvider`、`tool_selection_grants` 相当の epoch 方式（自前 `worker_meta`）。

D2. **実行の永続化は既存 `task_queue` の新 lane `worker` を使う**（新キューを作らない）。lease/claim/recover/cancel_key をそのまま使う。

D3. **rr_roots は使わない。role_routing からは不変条件と純関数を再利用**: `tools::classify_effect`/`permits_effect` 相当の判定（Worker role は read-only のみ許可）、`tool_ledger` と同じ reserve→dispatched→settled/unknown の台帳（`worker_tool_calls`）、premium proposal と同じ形の承認（`worker_escalations`）。理由: 会話エージェント キュー経路は rr_roots を作っておらず、「会話あたり active root 1」制約が並列 Worker と衝突する。

D4. **非同期結果の配信は steward outbox を再利用**（`steward_reports` に `task_id = wtask_*` で `enqueue_task_report`、`flush_held_reports`）。本文はホストのテンプレートで組む（モデル生成なし）。

D5. **Agent 発見は「ホストが事前に順位付けし、会話エージェント の dynamic context に候補カードを載せる」方式**。追加のモデル往復を作らない（音声遅延）。候補は fixed context ではなく dynamic references に置き、prefix cache を壊さない。順位は権限ではない: `delegate` は永続化した offer に含まれる profile だけを受理する。

D6. **Worker のモデル呼出しは `foreground()` スロットを使う**。会話エージェント が foreground read lock を保持したまま Worker を待つため、`background()`（write lock）を使うとデッドロックする。

D7. **Web Search Worker のモードは `worker_meta.web_search_mode ∈ {'inline','worker'}`（既定 `inline`）**。ロールバックは `inline` へ戻すだけ（データ移行不要）。会話指示は 2 系統（既存 `conversation-queue.txt` と新 `conversation-queue-worker.txt`）をモードで切替える。

## 4. 契約（WS-0 で先に固定する）

### 4.1 Rust 型 `src-tauri/src/worker_agents/contracts.rs`

```rust
pub(crate) const WORKER_LANE: &str = "worker";
pub(crate) const WEB_SEARCH_PROFILE_ID: &str = "web_search";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ReviewState { Draft, Approved, Rejected, Superseded }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub(crate) enum OutputKind { WebClaimsV1, JsonV1 }        // 閉じた集合。新種はコード追加

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Tier { Local, LocalLarge, Cloud }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CloudPolicy { Never, RequireApproval }    // v1 に自動課金はない

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct WorkerLimits {
    pub max_steps: u32,            // 既定 6（会話エージェント の MAX_TOOL_STEPS と同値）
    pub max_same_tier_retries: u32,// 既定 1
    pub deadline_ms: u64,          // 既定 40_000
    pub sync_wait_ms: u64,         // 既定 15_000（会話エージェント 残期限 - 5s で上限）
    pub max_restarts: u32,         // 既定 1（read-only Worker のみ）
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct TierPolicy { pub max_tier: Tier, pub cloud: CloudPolicy }

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ToolRef {                 // ツールチェーン参照
    pub kind: ToolRefKind,                  // builtin | catalog
    pub key: String,                        // builtin: "web_search" 等 / catalog: tool_selection_catalog.id
    pub catalog_revision_id: Option<String>,// catalog の場合は必須（固定）
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ProfileDraft {            // IPC upsert 入力
    pub profile_id: String,                 // ^[a-z][a-z0-9_]{2,39}$
    pub purpose: String,                    // 1..=2000 bytes、検索対象
    pub system_context: String,             // 1..=16384 bytes
    pub skill_revision_ids: Vec<String>,    // <= 8
    pub tools: Vec<ToolRef>,                // 1..=8
    pub input_schema: serde_json::Value,    // JSON Schema（jsonschema crate で検証）
    pub output_kind: OutputKind,
    pub output_schema: Option<serde_json::Value>, // JsonV1 のとき必須
    pub completion: CompletionCriteria,
    pub limits: WorkerLimits,
    pub tier_policy: TierPolicy,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct CompletionCriteria {      // ホストが検証できる述語だけ
    pub min_items: u32,                     // WebClaimsV1: claims 数 / JsonV1: 0
    pub sources_must_be_host_recorded: bool,// WebClaimsV1 は true 固定
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct DelegateRequest {         // 会話エージェント の action を host が解釈した後
    pub agent: String,                      // offer に含まれる profile_id
    pub input: serde_json::Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub(crate) enum FailureCode {
    NoMatchingAgent, StaleOffer, InputInvalid, Duplicate,
    NoSafeSources, NoResults, BudgetExhausted, DeadlineExceeded,
    ToolUnavailable, InvalidOutput, CompletionUnmet,
    EscalationRequiresApproval, EscalationDeclined,
    OutcomeUnknown, Cancelled, Interrupted,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WorkerFailure { pub code: FailureCode, pub retryable: bool }

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(tag = "status", rename_all = "snake_case")]
pub(crate) enum WorkerOutcome {             // 会話側が受け取る唯一の形
    Pending { task_id: String },
    Succeeded { task_id: String, output: WorkerOutput },
    Failed { task_id: Option<String>, failure: WorkerFailure },
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum WorkerOutput { WebClaimsV1(WebClaims), JsonV1(serde_json::Value) }

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct WebClaims {
    pub claims: Vec<WebClaim>,              // 1..=8
    pub excluded: ExcludedSummary,
    pub confidence: Confidence,             // ホストが算出（Worker 自己申告ではない）
    pub coverage: Coverage,                 // Worker 申告、enum のみ
}
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct WebClaim {
    pub text: String,                       // 1..=240 chars、単一行、§6.4 の形式制約
    pub source_url: String,                 // host 記録 URL と完全一致
    pub basis: ClaimBasis,                  // page | snippet
    pub published_or_fetched_at: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ExcludedSummary { pub count: u32, pub categories: Vec<String>, pub domains: u32 }
// Confidence: corroborated（2 以上の異なる host が支持）| single_source | snippet_only
// Coverage: complete | partial
// ClaimBasis: page（fetched_ok の URL）| snippet（usable search hit の URL）
```

ts-rs の `#[ts(export, export_to = "../../src/lib/generated/workerAgents.ts")]` は既存 `diagnosis/contract.rs` の書式に合わせる（WS-0 が確認して付与）。

### 4.2 SQLite スキーマ `src-tauri/src/worker_agents/schema.rs`（`pub(crate) fn migrate(&Connection) -> rusqlite::Result<()>`、冪等）

```sql
CREATE TABLE IF NOT EXISTS worker_meta (
  singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
  registry_epoch INTEGER NOT NULL DEFAULT 0 CHECK(registry_epoch >= 0),
  acl_epoch INTEGER NOT NULL DEFAULT 0 CHECK(acl_epoch >= 0),
  web_search_mode TEXT NOT NULL DEFAULT 'inline' CHECK(web_search_mode IN ('inline','worker'))
);
INSERT OR IGNORE INTO worker_meta(singleton) VALUES (1);
CREATE TABLE IF NOT EXISTS worker_profiles (
  id TEXT PRIMARY KEY CHECK(length(id) BETWEEN 3 AND 40),
  origin TEXT NOT NULL CHECK(origin IN ('builtin','user')),
  enabled INTEGER NOT NULL CHECK(enabled IN (0,1)),
  pinned_offer INTEGER NOT NULL DEFAULT 0 CHECK(pinned_offer IN (0,1)),
  current_revision_id TEXT,              -- 承認済み revision のみ指せる（repository で検証）
  created_at_ms INTEGER NOT NULL, updated_at_ms INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS worker_profile_revisions (
  id TEXT PRIMARY KEY,
  profile_id TEXT NOT NULL REFERENCES worker_profiles(id),
  revision INTEGER NOT NULL CHECK(revision > 0),
  review_state TEXT NOT NULL CHECK(review_state IN ('draft','approved','rejected','superseded')),
  created_by TEXT NOT NULL CHECK(created_by IN ('host_seed','user_ipc')),
  purpose TEXT NOT NULL CHECK(length(purpose) BETWEEN 1 AND 2000),
  system_context TEXT NOT NULL CHECK(length(system_context) BETWEEN 1 AND 16384),
  definition_hash TEXT NOT NULL CHECK(length(definition_hash) = 64), -- sha256(正規化 ProfileDraft)
  input_schema_json TEXT NOT NULL CHECK(json_valid(input_schema_json)),
  output_kind TEXT NOT NULL CHECK(output_kind IN ('web_claims_v1','json_v1')),
  output_schema_json TEXT CHECK(output_schema_json IS NULL OR json_valid(output_schema_json)),
  completion_json TEXT NOT NULL CHECK(json_valid(completion_json)),
  limits_json TEXT NOT NULL CHECK(json_valid(limits_json)),
  tier_policy_json TEXT NOT NULL CHECK(json_valid(tier_policy_json)),
  created_at_ms INTEGER NOT NULL, approved_at_ms INTEGER,
  UNIQUE(profile_id, revision), UNIQUE(profile_id, id)
);
CREATE TABLE IF NOT EXISTS worker_profile_tools (
  profile_revision_id TEXT NOT NULL REFERENCES worker_profile_revisions(id) ON DELETE CASCADE,
  ordinal INTEGER NOT NULL CHECK(ordinal >= 0),
  tool_kind TEXT NOT NULL CHECK(tool_kind IN ('builtin','catalog')),
  tool_key TEXT NOT NULL CHECK(length(tool_key) BETWEEN 1 AND 256),
  catalog_revision_id TEXT REFERENCES tool_selection_revisions(id),
  effect TEXT NOT NULL CHECK(effect IN ('pure','read','write','unknown')), -- 登録時に信頼できる側から解決
  PRIMARY KEY(profile_revision_id, tool_key),
  CHECK((tool_kind = 'catalog') = (catalog_revision_id IS NOT NULL))
);
CREATE TABLE IF NOT EXISTS worker_skills (id TEXT PRIMARY KEY, name TEXT NOT NULL CHECK(length(name) BETWEEN 1 AND 80));
CREATE TABLE IF NOT EXISTS worker_skill_revisions (
  id TEXT PRIMARY KEY, skill_id TEXT NOT NULL REFERENCES worker_skills(id),
  content_hash TEXT NOT NULL CHECK(length(content_hash) = 64),
  body TEXT NOT NULL CHECK(length(body) BETWEEN 1 AND 16384),
  created_at_ms INTEGER NOT NULL, UNIQUE(skill_id, content_hash)
);                                        -- 不変（UPDATE しない）
CREATE TABLE IF NOT EXISTS worker_profile_skills (
  profile_revision_id TEXT NOT NULL REFERENCES worker_profile_revisions(id) ON DELETE CASCADE,
  ordinal INTEGER NOT NULL, skill_revision_id TEXT NOT NULL REFERENCES worker_skill_revisions(id),
  PRIMARY KEY(profile_revision_id, ordinal)
);
CREATE VIRTUAL TABLE IF NOT EXISTS worker_profile_fts USING fts5(revision_id UNINDEXED, search_text, tokenize = 'trigram');
CREATE TABLE IF NOT EXISTS worker_profile_embeddings (
  revision_id TEXT NOT NULL REFERENCES worker_profile_revisions(id) ON DELETE CASCADE,
  model_hash TEXT NOT NULL, dimension INTEGER NOT NULL CHECK(dimension > 0), vector BLOB NOT NULL,
  PRIMARY KEY(revision_id, model_hash), CHECK(length(vector) = 4 * dimension)
);
CREATE TABLE IF NOT EXISTS worker_discovery_decisions (
  id TEXT PRIMARY KEY, conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
  input_message_id TEXT REFERENCES conversation_messages(id) ON DELETE CASCADE,
  job_key TEXT NOT NULL, registry_epoch INTEGER NOT NULL, acl_epoch INTEGER NOT NULL, model_hash TEXT,
  status TEXT NOT NULL CHECK(status IN ('ok','ambiguous','no_match','degraded')),
  created_at_ms INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS worker_discovery_candidates (
  decision_id TEXT NOT NULL REFERENCES worker_discovery_decisions(id) ON DELETE CASCADE,
  profile_revision_id TEXT NOT NULL REFERENCES worker_profile_revisions(id),
  lex_rank INTEGER, vec_rank INTEGER, vec_score REAL, score REAL NOT NULL,
  final_rank INTEGER NOT NULL, offered INTEGER NOT NULL CHECK(offered IN (0,1)), pinned INTEGER NOT NULL CHECK(pinned IN (0,1)),
  PRIMARY KEY(decision_id, profile_revision_id)
);
CREATE TABLE IF NOT EXISTS worker_tasks (
  id TEXT PRIMARY KEY,                    -- 'wtask_' + uuid
  conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
  input_message_id TEXT NOT NULL REFERENCES conversation_messages(id) ON DELETE CASCADE,
  origin_job_key TEXT NOT NULL, decision_id TEXT REFERENCES worker_discovery_decisions(id) ON DELETE SET NULL,
  profile_id TEXT NOT NULL REFERENCES worker_profiles(id),
  profile_revision_id TEXT NOT NULL REFERENCES worker_profile_revisions(id),
  idempotency_key TEXT NOT NULL UNIQUE,   -- sha256(input_message_id|profile_revision_id|canonical input)
  input_json TEXT NOT NULL CHECK(json_valid(input_json)),
  state TEXT NOT NULL CHECK(state IN ('accepted','running','verifying','succeeded','failed','cancelled')),
  delivery TEXT NOT NULL CHECK(delivery IN ('sync_waiting','sync_delivered','async_queued','async_delivered','suppressed')),
  tier_index INTEGER NOT NULL DEFAULT 0, attempts INTEGER NOT NULL DEFAULT 0, restarts INTEGER NOT NULL DEFAULT 0,
  deadline_at_ms INTEGER NOT NULL, sync_wait_until_ms INTEGER NOT NULL,
  result_json TEXT CHECK(result_json IS NULL OR json_valid(result_json)),
  failure_code TEXT, created_at_ms INTEGER NOT NULL, updated_at_ms INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_worker_tasks_input ON worker_tasks(input_message_id, state);
CREATE TABLE IF NOT EXISTS worker_attempts (
  task_id TEXT NOT NULL REFERENCES worker_tasks(id) ON DELETE CASCADE, ordinal INTEGER NOT NULL,
  tier TEXT NOT NULL CHECK(tier IN ('local','local_large','cloud')), route_fingerprint TEXT NOT NULL,
  status TEXT NOT NULL CHECK(status IN ('running','succeeded','failed','cancelled','interrupted')),
  error_code TEXT, steps_used INTEGER NOT NULL DEFAULT 0, started_at_ms INTEGER NOT NULL, finished_at_ms INTEGER,
  PRIMARY KEY(task_id, ordinal)
);
CREATE TABLE IF NOT EXISTS worker_tool_calls (    -- rr_tool_links と同じ意味論
  task_id TEXT NOT NULL REFERENCES worker_tasks(id) ON DELETE CASCADE,
  operation_key TEXT NOT NULL,                     -- attempt:step:tool:args_digest
  attempt_ordinal INTEGER NOT NULL, tool_key TEXT NOT NULL,
  effect TEXT NOT NULL CHECK(effect IN ('pure','read','write','unknown')),
  dispatch_state TEXT NOT NULL CHECK(dispatch_state IN ('reserved','dispatched','settled','unknown')),
  outcome TEXT, created_at_ms INTEGER NOT NULL, updated_at_ms INTEGER NOT NULL,
  PRIMARY KEY(task_id, operation_key)
);
CREATE TABLE IF NOT EXISTS worker_sources (
  task_id TEXT NOT NULL REFERENCES worker_tasks(id) ON DELETE CASCADE,
  url TEXT NOT NULL CHECK(length(url) <= 2048), host TEXT NOT NULL,
  kind TEXT NOT NULL CHECK(kind IN ('search_hit','fetched','user_supplied')),
  status TEXT NOT NULL CHECK(status IN ('recorded','usable','failed','excluded')),
  guard_decision TEXT, warning_categories_json TEXT NOT NULL DEFAULT '[]' CHECK(json_valid(warning_categories_json)),
  check_suspected INTEGER CHECK(check_suspected IN (0,1)),
  check_categories_json TEXT NOT NULL DEFAULT '[]' CHECK(json_valid(check_categories_json)),
  check_excerpt TEXT CHECK(check_excerpt IS NULL OR length(check_excerpt) <= 240),
  fail_reason TEXT, created_at_ms INTEGER NOT NULL,
  PRIMARY KEY(task_id, kind, url)
);
CREATE TABLE IF NOT EXISTS worker_url_blocklist (
  url_hash TEXT PRIMARY KEY CHECK(length(url_hash) = 64), host TEXT NOT NULL,
  reason TEXT NOT NULL, created_at_ms INTEGER NOT NULL   -- 期限なし。ユーザーが IPC で削除するまで残る
);
CREATE TABLE IF NOT EXISTS worker_escalations (     -- rr_premium_proposals と同形
  id TEXT PRIMARY KEY, task_id TEXT NOT NULL REFERENCES worker_tasks(id) ON DELETE CASCADE,
  tier TEXT NOT NULL, route_fingerprint TEXT NOT NULL, estimated_cost_micros INTEGER,
  status TEXT NOT NULL CHECK(status IN ('proposed','approved','declined','expired','consumed')),
  expires_at_ms INTEGER NOT NULL, created_at_ms INTEGER NOT NULL, decided_at_ms INTEGER
);
```

`schema.rs` は加えて `pub(crate) fn seed_builtin(connection, now_ms)`（Web Search Worker の `revision=1, review_state='approved', created_by='host_seed', pinned_offer=1, enabled=1`。`INSERT OR IGNORE`。既存 revision があれば何もしない＝ユーザー編集を上書きしない）を持つ。

### 4.3 IPC（`src-tauri/src/worker_agents/registry/commands.rs`）

| command | 入力 | 動作 |
| --- | --- | --- |
| `list_worker_agents` | なし | profile + current revision 要約 + draft 有無 |
| `get_worker_agent` | `{profileId}` | 全 revision、ツール、skill |
| `save_worker_agent_draft` | `ProfileDraft` | 新 revision を `draft` で作成。`registry_epoch` は変えない |
| `approve_worker_agent_revision` | `{profileId, revisionId, definitionHash}` | hash 一致時のみ `approved`、旧 current を `superseded`、`current_revision_id` 更新、FTS/埋め込み更新、`registry_epoch += 1` |
| `set_worker_agent_enabled` | `{profileId, enabled}` | `registry_epoch += 1` |
| `save_worker_skill` | `{skillId?, name, body}` | 新 skill revision（不変、content_hash で重複排除） |
| `set_worker_web_search_mode` | `{mode: "inline"|"worker"}` | `worker_meta` 更新 |
| `list_worker_tasks` | `{limit}` | 最近のタスク・状態・failure_code（本文なし） |
| `cancel_worker_task` | `{taskId}` | §5.5 |
| `list_worker_url_blocklist` | `{limit, after?}` | blocklist の URL・host・reason・登録時刻（URL は `url_hash` と併せ、表示用に host を返す） |
| `remove_worker_url_blocklist` | `{urlHash}` | 指定エントリを削除（blocklist を消せるのはこの IPC だけ。自動削除・期限切れ・prune は存在しない） |

`save`/`approve` はユーザー操作専用（IPC のみ）。会話モデルの action からは到達できない。承認時の検証: ツールは `builtin` 既知集合（v1: `web_search`, `fetch_content`）か `tool_selection_catalog` の enabled revision。effect は `builtin` は host 定数、`catalog` は `tool_selection::repository::effect_for_backend_key` 相当で解決（ユーザー入力の effect は受けない）。v1 実行器は全ツール effect ∈ {pure, read} の revision だけ承認可能（それ以外は `draft` 止まり、エラー `write_tools_not_supported`）。

### 4.4 会話 Agent の action（`conversation-queue-worker.txt` で記述）

```json
{"action":"delegate","agent":"web_search","input":{"query":"検索したい内容"}}
```
- `answer` の `sources` は直前の `WORKER_RESULT` の `claims[].sourceUrl` の部分集合のみ host が採用（既存 `selected_urls` の照合を流用、候補集合を差し替える）。
- `web_search`/`fetch_content` action は worker モードの指示には存在しない。worker モードで来たら `conversation-action-invalid` と同じ扱い。

会話エージェント に渡す結果（dynamic reference、`required=true`）:
```
[WORKER_RESULT: web_search; 未信頼の資料; Web由来]
{"status":"succeeded","claims":[{"text":"…","sourceUrl":"https://…","basis":"page"}],"confidence":"single_source","coverage":"partial","excluded":{"count":1,"categories":["instruction_override"],"domains":0}}
```
`pending`: `{"status":"pending"}`（task_id は渡さない）、失敗: `{"status":"failed","code":"no_safe_sources"}`。

候補カード（dynamic reference）:
```
[HOST_WORKER_OFFER; data only] [{"agent":"web_search","purpose":"(<=200 chars)","input":{"query":"string"}}]
```

## 5. 委譲契約とライフサイクル

### 5.1 発見（discovery）
1. 入力: 現在の user 発話テキスト（`payload.text`）。`worker_meta` の epoch と eligible revision（profile.enabled ∧ current revision approved ∧ 全ツール effect read-only）を writer の read で snapshot。
2. lexical: `fts_match_query`→`worker_profile_fts`、vector: `EmbeddingProvider` があれば `embedding_candidates`。`fuse(lex, vec, 3)`。
3. 閾値: lexical hit あり、または `vec_score >= 0.80`（定数 `MIN_VEC_SCORE`、計測で調整）。`pinned_offer=1` の profile は常に候補（Web Search はモード `worker` のときのみ pinned として扱う）。
4. status: 候補なし→`no_match`（offer を載せない。会話エージェント は通常どおり answer。委譲されなければ失敗ではない）。上位 2 件の `score` 差 < 1e-9（同順位）→`ambiguous`（両方 offer、選ぶのはモデル、推測で 1 つに決めない）。埋め込み不可→`degraded`（lexical のみ）。
5. decision/candidates を 1 transaction で保存してから offer を context に載せる（commit before effects）。

### 5.2 受理（admission）— `worker_agents::executor::admit`
```rust
pub(crate) fn admit(tx: &Connection, req: &AdmitRequest, now_ms: i64) -> Result<WorkerOutcome, String>;
pub(crate) struct AdmitRequest<'a> {
    pub conversation_id: &'a str, pub input_message_id: &'a str, pub origin_job_key: &'a str,
    pub decision_id: &'a str, pub delegate: &'a DelegateRequest, pub ornith_deadline_ms: i64,
}
```
同一 transaction で: (a) `input_message_id` が conversation の role='user' 行として存在（永続入力への束縛）、(b) `delegate.agent` が `decision_id` の `offered=1` 候補、(c) `worker_meta` epoch が decision と一致（不一致→`Failed{StaleOffer}`、会話エージェント 側で discovery を 1 回だけやり直す）、(d) `input` を revision の `input_schema_json` で検証（`jsonschema`）、(e) idempotency_key で既存 task があればそれを返す（`Duplicate` は失敗ではなく既存 task へ合流）、(f) `worker_tasks` 挿入 + `task_queue::enqueue(tx, conversation_id, "worker", "worker_task", task_id, 0, payload, None)`。task はこの時点で profile_revision_id を固定（以後の編集は影響しない）。

### 5.3 同期/非同期
- admit 成功直後、会話エージェント 側は `queue_progress::enqueue_search` 相当の受付発話を即時に積む（既存の「調べています」行。音声経路は不変）。
- 会話エージェント の当該 step は `min(sync_wait_ms, ornith_deadline - 5s)` まで task の終端を待つ（in-process `tokio::sync::Notify` を task_id で引き、真実は DB で再読込）。
  - 終端済み→`Succeeded`/`Failed` を `WORKER_RESULT` として積み次 step へ。`delivery='sync_delivered'` を同 transaction で記録。
  - 未終端→`Pending` を積む。指示により 会話エージェント は「調べて後で伝える」旨を answer する。`delivery='async_queued'`。
- 非同期完了時（Worker lane の終端 transaction 内）: `delivery='async_queued'` なら steward outbox に `enqueue_task_report(tx, conversation_id, &TerminalReport{task_id, task_revision: 0, goal_id: "", notify: "always", digest}, None, now_ms, true)` し、commit 後 `steward::report::publish::flush_held_reports(state, conversation_id)`。digest はホストテンプレート:「先ほどの『{query 先頭40字}』の調査結果です。{claim1}（{host1}）…」または失敗文言（`no_safe_sources`→「安全に確認できる情報源が見つからず、確認できませんでした。」）。配信後 `delivery='async_delivered'`。UNIQUE(task_id,task_revision,destination) で二重配信なし。
  - 要確認（WS-C 着手時）: `steward::repository`/`steward::report::publish` の可視性、`TerminalReport.goal_id/notify` の空値許容。不可なら WS-C が steward 内に `pub(crate) fn enqueue_external_report(...)` を薄く追加する（steward 側ファイルへの書込みは WS-C に限定）。

### 5.4 Worker 実行（lane `worker`）
- `queue_runtime::spawn` に `worker` lane runner を 2 本追加。`process_job` の lane match に `"worker" => worker_lane::process(app, job, cancellation)`。
- 推論は `foreground()` スロット（D6）。
- ループ（`executor::run_attempt`）: 入力は profile.system_context + pinned skill 本文（順序固定）+ delegated input のみ。会話履歴・Memory・WorldModel・資格情報・`HOST_CODING_CONTEXT` は入れない。step ごとに Worker は `{"action":"web_search"|"fetch_content"|"finish"|"give_up", …}` を返す。ツール呼出しは `worker_tool_calls` に reserve→dispatched→settled を commit してから実行（`tool_ledger` と同じ意味論）。
- 完了判定はホスト: `OutputKind` ごとの validator（`WebClaimsV1`→§6.4、`JsonV1`→`output_schema_json`）＋ `CompletionCriteria`。Worker の「成功」テキストは使わない。

### 5.5 取消・期限・再起動
- ユーザー入力取消（`cancel_input`→`task_queue::cancel_key`）: 同 `input_message_id` の active worker task を `cancelled` にし、worker lane job を `cancel_key(scope, task_id)`、in-process の `RunCancellation` を発火。既に async_queued なら outbox に入れない（`delivery='suppressed'`）。
- 期限: `deadline_at_ms` 超過→`Failed{DeadlineExceeded}`。
- 再起動: worker lane の `recover` で `running` task を検査。全ツール effect read-only かつ `restarts < max_restarts`→`restarts+1` で再キュー（read-only なので再実行は外部効果の replay ではない）。`worker_tool_calls` に `dispatched`/`unknown` の非 read-only がある、または上限超過→`Failed{Interrupted}` で報告。sync 待ちの 会話エージェント は再起動で消えるため、復旧 task は常に async 配信。

## 6. 失敗の意味論と Web Search Worker

### 6.1 既存ギャップ（確認済み、WS-G で修正）
`fetch_content` の webview 経路は guard `deny`/`require_approval` でも本文をモデル入力に渡す: `static_content.rs` は Deny/RequireApproval で `Ok(None)` を返し webview にフォールバック → `content.rs` L148 `project_document` は decision を保持したまま `text` を返す → `render_compact` は decision を見ずに `document.text` を出力 → `conversation_answer.rs` L371 が `[TOOL_RESULT: fetch_content; 未信頼の資料]` として context に積む。HTML 経路で落とした危険ページを webview 経路で取り込み直す形になっている。
修正: `render_compact` で `decision ∈ {"deny","require_approval"}` のとき `document.text` を空にし `retrievalStatus:"blocked"` とし、`security.decision` は残す（inline モードにも即効く）。テスト追加。

### 6.2 再試行・昇格
1. 同一 tier 内: モデル出力の JSON 不正、transport timeout、`CompletionUnmet` は `max_same_tier_retries`（既定 1）まで同じ attempt context で再試行（モデル呼出しは外部効果なし）。ツール呼出しの再試行は effect ∈ {pure, read} のみ、`dispatch_state='unknown'` の非 read-only は再試行せず `OutcomeUnknown`。
2. tier 昇格: 上の失敗が尽きたら次 tier。階梯 = `[conversation.respond の primary route]` + （WS-H 実装後）`worker.escalate` binding の `fallback_resource_ids` 順。`ResolvedRoute.location=="cloud"` の tier は `tier_policy.cloud == RequireApproval` かつ `binding.cloud_allowed` のときだけ候補で、`worker_escalations` に `proposed` を書いて `Failed{EscalationRequiresApproval}` を返す（v1 は会話内承認フローなし、§9 Q2）。`tier > max_tier` は候補にしない。新 attempt は同 task の `worker_sources`（usable/failed）と exclusion を引き継ぎ、取得済みの安全な資料を再取得しない。
3. 昇格しない失敗: `NoSafeSources`, `NoResults`, `InputInvalid`, `Cancelled`, `DeadlineExceeded`, `Interrupted`, `OutcomeUnknown`。
4. 終端は必ず `WorkerOutcome::Failed{code}` を会話へ報告（sync なら WORKER_RESULT、async なら outbox）。

### 6.3 Web Search Worker の手順と制限
- 入力スキーマ: `{"query": string(1..=400 bytes), "urls"?: [string] (<=3)}`。`urls` は委譲元 user 発話テキストに完全一致で含まれる URL のみ受理（`user_supplied`）。
- ツール: builtin `web_search`, `fetch_content` のみ（`execute_agent_tool` を `output_persistence=None` で呼ぶ。conversation_answer.rs と同じ引数形 `{"query","limit":5}` / `{"url","maxCharacters":3000,"query"}`）。
- 上限（既定、profile の `limits` と web_search 固有定数）: `max_steps=6`、検索 3 回、fetch 4 回、失敗ソース 4 件、deadline 40s。同一 host の flagged が 2 件で host を task 内除外（`HOST_EXCLUDE_THRESHOLD = 2`、host は小文字化・先頭 `www.` 除去。PSL は依存にないため登録ドメイン単位ではない）。
- 検索後: hit ごとに host 側で `worker_sources(kind='search_hit')` 記録。blocklist/除外 host の hit は捨てる。残りの hit の title+snippet をまとめて injection checker（§6.5）にかけ、flagged index の hit は `failed`（Worker には URL も渡さない）。残りを Worker context へ（url, title, snippet<=300 chars, date）。
- fetch 前: URL は当該 task の `usable` search hit または `user_supplied` と**完全一致**のみ（conversation_answer.rs の `selected_urls` 照合の拡張）。不一致→ツール結果 `{"error":"url_not_recorded"}`、step 消費。
- fetch 後: plugin flag = `decision ∈ {deny, require_approval}` または（`allow_with_warning` かつ `warningCategories` に `low_trust_attribute` 以外を含む）。checker flag = §6.5 の `suspected`。**どちらか**が立てば source failure: `status='failed'`、本文は破棄し Worker には `{"url":"…","status":"failed","reason":"unsafe_source"}` だけを返す、`worker_url_blocklist` に永続登録（期限なし。ユーザーが削除するまで残す）、task 内の host カウント加算、Worker は別ソースを探す。
- 終了: `finish{claims, coverage}` を §6.4 で検証。usable source が 0 で失敗ソース ≥1 →`NoSafeSources`、検索結果自体が 0 →`NoResults`、上限到達→`BudgetExhausted`。flagged 資料由来の claim で成功を返すことはない（source_url が usable 集合に無ければ claim は棄却）。

### 6.4 出力（claims）のホスト検証 `web_search/claims.rs`
- 1..=8 claims、`text` 1..=240 chars、改行・制御文字なし。
- 禁止: `http`/`www.` を text に含む（URL は `sourceUrl` のみ）、`[`/`]`/`<`/`>`/```、`TOOL_RESULT`/`HOST_`/`WORKER_`、`system`/`assistant`/`user`/`あなた`/`ユーザー`/`AI`/`指示`/`無視` 等の呼びかけ・役割語（正規表現リストを定数化）、命令形の簡易検出（文末 `ください`/`しろ`/`せよ`/`!` と英語の行頭命令動詞リスト）。
- 各 text と全体連結に `tauri_plugin_llm_fetch::inspect_plain_text_bounded` を適用し decision ≠ `allow` の claim は棄却（既存 guard の再利用）。
- `sourceUrl`: `basis=page`→当該 task の `fetched` かつ `usable`、`basis=snippet`→`search_hit` かつ `usable`。完全一致。
- 棄却後 `min_items` 未満→同 tier 再試行（`CompletionUnmet`）。
- `confidence` はホスト算出（usable claim の異なる host 数 ≥2→`corroborated`、page が 1 host→`single_source`、snippet のみ→`snippet_only`）。

### 6.5 Injection checker（tool なし別呼出し）
- 固定 SystemContext `contexts/worker/injection_check.context.toml`（生成物 `.s11tnext/worker-injection-check.txt`）。入力は random nonce 境界で囲んだ未信頼テキスト（最大 3,000 chars、超過分は「truncated」を付けて渡し、truncated は flag 扱い＝plugin と同じ fail-closed）。
- 出力（厳格 JSON、`deny_unknown_fields`）: `{"suspected":bool,"categories":[plugin の SecurityFindingCategory snake_case 名],"excerpt":string(<=160)}`。検索 hit バッチ版は `{"flagged":[index...],"categories":[...]}`。
- パース失敗・timeout→suspected=true（fail-closed）。checker 出力は Worker/会話エージェント の context に入れず、監査（excerpt はさらに redact）のみ。
- route は当該 attempt と同じ tier。

### 6.6 監査と保持
- `worker_sources` 行: URL、guard decision、warningCategories、checker 結果、redacted excerpt（flagged のみ、<=240 chars、メール・長い数字列・`sk-`/`Bearer` 様トークンを `[redacted]` に置換）、fail_reason。本文は保存しない。
- `audit_events`（既存、2KB 上限）に task 終端ごとに `component='worker', event_name='worker-task-terminal'`、attributes: profile/revision、tier、attempts、steps、searches、fetches、usable/failed/excluded 件数、カテゴリ集合、failure_code、所要 ms、delivery。
- 保持期間は設けない（決定済み）。`worker_sources`/`worker_tool_calls`/`worker_attempts`/`worker_tasks` を自動削除しない（`prune` は作らない）。`worker_url_blocklist` は期限なしで、ユーザーが `remove_worker_url_blocklist` で削除したときだけ消える。疑いのあったページの本文・抜粋は blocklist には持たず、`url_hash`・host・reason のみを保持する（記憶は blocklist が担う）。既存の汎用 `audit_events` は既存の `AUDIT_RETENTION_DAYS` に従い、本書では変更しない。

### 6.7 残余リスク（明記）
- 検知は保証ではない。Worker は fetched 本文を読むので Worker 内部で injection が成功しうる。保証は**能力制限**（read-only ツール 2 つ、会話・Memory・秘密なし、Memory/WorldModel 書込み不可、URL は host 記録との完全一致）と**型付き出力**（短い宣言文、形式検証、guard 再検査）である。
- 残るのは誤情報（汚染された主張）。緩和: claim は Web 由来として明示、source URL は検証済み、`confidence` はホスト算出、会話側は自動 fetch しない。
- checker 自身も敵対テキストを読む。tool なし・構造化出力・fail-closed で影響を「誤判定」に限定。
- false positive で正当なページが落ちる。`NoSafeSources` が増える可能性を計測（§7）で確認する。

## 7. 移行と計測

計測指標（`audit_events` と `context_metrics::RequestMetrics` から取る。inline/worker 両モードで同じ評価セット）
- 応答遅延: 発話受付→最初の音声（受付発話）、→最終回答。worker は sync/async 別。
- 会話 context のツール結果量: `[TOOL_RESULT: web_search|fetch_content` エントリの bytes 合計 vs `[WORKER_RESULT` の bytes。
- 成否: 回答に検証済み source がある率、`failure_code` 分布、`Pending` 率、async 配信成功率。
- injection: plugin flag 数、checker flag 数、両方/片方のみ、除外 host 数、`NoSafeSources` 率。
- 評価セット: `conversation-queue-e2e` の fixture（`conversation_queue_e2e::web_search`/`fetch_content`）に注入ページ・正常ページ・混在を追加。

手順
1. WS-G を先行マージ（inline のままギャップ修正）。
2. 全 WS 完了・統合後、既定 `inline` のまま出荷。評価セットで両モードを計測し、本書末尾に記録。
3. 合格基準: worker の最終回答遅延中央値が inline の +20% 以内、context ツール結果 bytes が 50% 以上減、注入 fixture で flagged 本文が 会話エージェント context に 0 件、正常 fixture の `NoSafeSources` < 5%。
4. `set_worker_web_search_mode('worker')` で切替。ロールバック = `inline` に戻す（テーブル・データはそのまま、inline 経路のコードは削除しない）。inline 経路の削除は別計画。

## 8. Workstreams

着手前に coordinator が `git status` を確認し、下表の書込みパスが dirty なら配らない。worker は対象テストのみ実行（`cargo test -p saaa <filter>` は crate 名を `src-tauri/Cargo.toml` で確認のこと）。全体 build/test は coordinator が最後に 1 回。

### WS-0（coordinator、他の全 WS の前）: 契約固定
- 書込み: `src-tauri/src/worker_agents/mod.rs`（全サブモジュール宣言と空 stub）、`src-tauri/src/worker_agents/contracts.rs`（§4.1 全文）、`contexts/conversation/queue-worker.context.toml`、`contexts/worker/web_search.context.toml`、`contexts/worker/injection_check.context.toml`、`bun run s11tnext:build` による `.s11tnext/*` 生成、`src-tauri/src/lib.rs` に `mod worker_agents;`。
- 受入: `cargo check` が通る（stub）。

### WS-A: Registry とスキーマ
- 目的: §4.2 全テーブル、seed、repository（CRUD・承認・epoch・FTS/埋め込み更新）、§4.3 IPC。
- 書込み: `src-tauri/src/worker_agents/schema.rs`, `src-tauri/src/worker_agents/registry/**`, `src-tauri/src/worker_agents/README.md`。
- 禁止: `persistence/**`, `tool_selection/**`, `command_registry.rs`, `lib.rs`, 他 WS のディレクトリ。
- 依存: WS-0。
- 受入テスト（`worker_agents::registry::tests`）: 既存 DB コピー相当（v44 スキーマを `initialize_database` で作った in-memory DB）に `migrate` を 2 回適用して冪等、seed が既存 revision を上書きしない、draft は discovery 対象外、approve の hash 不一致拒否、write-effect ツールを含む draft は承認拒否、approve/enable で `registry_epoch` 増加、skill revision 不変。

### WS-B: Discovery
- 目的: §5.1。`pub(crate) fn discover(writer, embedder: Option<&dyn EmbeddingProvider>, conversation_id, input_message_id, job_key, text, web_search_mode) -> Result<Offer, String>` と `pub(crate) fn render_offer(&Offer) -> String`、`pub(crate) fn validate_choice(tx, decision_id, agent) -> Result<String /*revision_id*/, FailureCode>`。
- 書込み: `src-tauri/src/worker_agents/discovery/**`。
- 禁止: `tool_selection/**`（関数を呼ぶだけ）、他 WS。
- 依存: WS-0、WS-A の repository read API（WS-0 の stub シグネチャで並行可）。
- 受入: no_match で offer 空、同順位で ambiguous と 2 件 offer、pinned は mode=`worker` のときだけ出る、epoch 変更後の `validate_choice` が `StaleOffer`、未 offer の agent 拒否、embedder なしで `degraded`。

### WS-C: Executor（汎用）
- 目的: §5.2–5.5、§6.2。admit、worker lane `process`（モデル呼出しは trait 越し）、attempt/tier 階梯、ledger、recover、cancel、sync 待ち Notify、async の steward outbox 配信、`OutputKind` validator 呼出し点。
- モデル呼出し抽象（WS-0 で contracts に置く）:
  ```rust
  #[async_trait] pub(crate) trait WorkerModel: Send + Sync {
      async fn complete(&self, tier_route: &ResolvedRoute, system: &str, input: &str,
                        cancellation: &RunCancellation, timeout: Duration) -> Result<String, String>;
  }
  ```
- 書込み: `src-tauri/src/worker_agents/executor/**`。必要時のみ `src-tauri/src/steward/repository/terminal_report.rs` に `enqueue_external_report` を追加（それ以外の steward ファイルは禁止）。
- 禁止: `runtime/**`, `web_fetch/**`, 他 WS。
- 依存: WS-0、WS-A。
- 受入（fake `WorkerModel` と fake tool で）: 同一入力の二重 admit が同 task を返す、未永続 input を拒否、JSON 不正 1 回→再試行成功、再試行尽き→次 tier、cloud tier は `EscalationRequiresApproval`、`unknown` の write 呼出しは再試行しない、再起動で read-only task は 1 回だけ再キュー・2 回目は `Interrupted`、取消で outbox に入らない、async 終端で `steward_reports` に 1 行（2 回終端処理しても 1 行）。

### WS-D: Web Search Worker
- 目的: §6.3–6.6。URL 照合・除外・blocklist、checker の入出力 parse（fail-closed）、claims validator、confidence 算出、監査行作成、web_search profile の seed 定義（`ProfileDraft` 定数）。
- 書込み: `src-tauri/src/worker_agents/web_search/**`。
- 禁止: `runtime/web_fetch/**`（呼ぶだけ）、`conversation_answer.rs`、他 WS。
- 依存: WS-0（WS-C の executor へは `OutputKind::WebClaimsV1` validator と tool hook trait で接続）。
- 受入: 未記録 URL の fetch 拒否、`require_approval`/`deny`/高リスク warning の source が failed かつ Worker 入力に本文が入らない、checker parse 失敗で flagged、同 host 2 回 flagged で host 除外、全 source flagged で `NoSafeSources`、命令形・役割語・URL 混入 claim の棄却、flagged URL を source にした claim の棄却、excerpt redaction。

### WS-E: 会話統合（don't split: `conversation_answer.rs` と `queue_runtime.rs` は同一 owner）
- 目的: `web_search_mode` で指示テキスト切替（`queue_context.rs`）、discovery 呼出しと offer 注入、`delegate` action の処理（admit→受付発話→sync 待ち→WORKER_RESULT 注入）、`sources` 照合を worker 検証 URL へ、`worker` lane runner と `process_job` 分岐、`WorkerModel` の実装（`direct_route` を使う `conversation_check/worker_lane.rs`、`foreground()` 取得）、cancel_input から worker task 取消。
- 書込み: `src-tauri/src/runtime/conversation_check/queue_runtime/conversation_answer.rs`, `…/queue_runtime/action.rs`, `…/queue_runtime/web_result.rs`, `src-tauri/src/runtime/conversation_check/queue_runtime.rs`, `src-tauri/src/runtime/conversation_check/queue_context.rs`, 新規 `src-tauri/src/runtime/conversation_check/worker_lane.rs`。
- 禁止: `worker_agents/**`（呼ぶだけ）、`web_fetch/**`、`conversation_check.rs`（mod 宣言は coordinator）、音声関連（`streaming_speech.rs`, `speech_*`, `voice/**`）。
- 依存: WS-0, WS-B, WS-C, WS-D のシグネチャ（stub で並行着手可、結合テストは全 WS 後）。
- 受入（`conversation-queue-e2e` feature の fixture）: inline モードで既存テストが不変、worker モードで 会話エージェント context に `[TOOL_RESULT: web_search`/`fetch_content` が 0 件・`[WORKER_RESULT` が 1 件、注入 fixture の本文文字列が 会話エージェント の全 request に現れない、sync 超過で pending 回答＋後から steward report 配信、`sources` が claim URL 以外を含まない、ASR/TTS 回帰（既存 voice テスト）不変。

### WS-F: 最小 UI
- 目的: 設定内「Worker Agents」一覧、有効/無効、draft 承認、web search モード切替、最近のタスク状態表示。
- 書込み: `src/features/workerAgents/**`, `tests/worker-agents-page.test.tsx`。
- 禁止: `src/lib/generated/**`, `src/App.tsx`/ルーター、i18n の共有 index。
- 依存: §4.3 の IPC 形（WS-0 の生成型）。
- 受入: 単体テストでの描画・invoke 呼出し。

### WS-R: 会話エージェントの名称整理（モデル名 `ornith` を役割名へ。モデル切替に備える）
方針: コード上の役割名はモデル名に依存させない。**モデルそのものを指す識別子は改名しない**（モデルは今後も変わり、履歴データと一致する必要があるため）。段階を分け、保存データに触れる段階は移行を伴うので別 PR にする。

分類（2026-10-06 時点の調査）:
| 区分 | 該当 | 扱い |
| --- | --- | --- |
| A. コード上の名前（完了） | モジュール `ornith`→`conversation_answer`、`process_ornith`→`process_conversation_answer`、`OrnithAnswer`→`ConversationAnswer`、`quality_eval`/`scripts/conversation-quality-eval.ts` の `runtimePath` ラベル | **実施済み**（`cargo check --lib --tests` 通過）。 |
| B. 保存データに無関係な名前 | IPC のステージ名 `"ornith"`（`runtime/conversation_check.rs` `report_stage`、`src/lib/runtime.ts`、`ConversationCheckPage.tsx` の `RouteStage`）、画面メッセージ中の「Ornith」、監査イベント名 `conversation-ornith-followup-failed`、テスト/e2e のレーン名 `"ornith"`（`task_queue.rs` tests、`conversation_queue_e2e.rs`）、`ornith_task` 等の kind | Rust と TS を**同時に**変更（ステージは `"answer"` 等）。監査イベント名は過去ログとの連続性を壊すため、変更時に旧名を併記して注記。WS-E と書込み先が重なるため **WS-E の前に単独で実施**。 |
| C. モデル識別子（改名しない） | LARM プロファイル ID `saaa-conversation-ornith15`（`providers/dynamic_lan/profile_catalog.rs`、`larm_resources/profile.rs`、`role_routing/schema.rs`）、モデル名 `ornith-1.5-35b`、設定 UI の「LARM: Ornith 1.5」 | これは実在するモデル/プロファイルの名前。gemma4 は**新しいプロファイルを追加**して切り替える。旧 ID は履歴・既存設定のため残す。 |
| D. 出荷済みプロンプトと移行 | `role_routing/frontend.rs` の system prompt 文中「Ornithへ引き継ぎます」、`persistence/settings_migration/stored_document.rs` の `migrate_shipped_ornith_frontdesk`（出荷済み文面との一致で移行） | 文面変更は**移行関数が一致判定に使う出荷済み文面の履歴**を壊す。新文面を追加し、旧文面も一致対象に残す移行を伴う別 PR（要レビュー）。名称は `migrate_shipped_frontdesk_prompt` 等へ。 |
| E. 文書 | `docs/plans/*ornith*.md`、`docs/evals/*`、`spec/evidence/*` | 履歴文書はそのまま（ファイル名も変えない）。現行の計画書は本文の役割名を中立語へ（`worker-agents.md` は完了）。 |

- 受入（B）: `bun run` の型チェックと `tests/provider-unit-test-page.test.tsx` 等の該当 TS テスト、`cargo test` の `queue_runtime`/`conversation_check` 対象テストが通る。ステージ名の Rust/TS 不一致が無い（grep で `"ornith"` が B 区分から消える）。
- 受入（D）: 旧出荷文面・旧文面の customized/extended/referenced の 4 経路すべてが従来どおり移行される既存テストが通り、新文面追加分のテストが加わる。
- 順序: A（完了）→ B → WS-E。C は改名しない。D は WS-E と独立でいつでも可（優先度低）。

### WS-G: fetch_content guard ギャップ修正（独立、先行可）
- 書込み: `src-tauri/src/runtime/web_fetch/content/projection/compact.rs`, `src-tauri/src/runtime/web_fetch/content.rs`（テストのみ）。
- 受入: `deny`/`require_approval` の `render_compact` 出力で `document.text == ""`、`retrievalStatus == "blocked"`、`security.decision` 保持。`allow_with_warning` は従来どおり。

### WS-H（phase 2、任意）: `Purpose::WorkerEscalate` と設定 UI
- `providers/service_registry/types.rs` の enum 追加は exhaustive match に波及するため coordinator 管理。v1 は階梯 1 段（conversation.respond primary）で出荷可能。

### 統合ファイル（各 WS 完了後に coordinator のみが編集）
- `src-tauri/src/lib.rs`（`mod worker_agents;`、WS-0 で実施）
- `src-tauri/src/persistence/schema.rs`: `crate::worker_agents::schema::migrate(&transaction)?` を `crate::tool_selection::schema::migrate` の**後**（`worker_profile_tools` が `tool_selection_revisions` を参照）に追加、`seed_builtin` 呼出し、`DATABASE_SCHEMA_VERSION` 44→45 とコメント「45 adds worker agent registry, tasks, and source audit.」
- `src-tauri/src/runtime/command_registry.rs`（§4.3 の 9 command）
- `src-tauri/src/runtime/conversation_check.rs`（`mod worker_lane;`）
- `src/lib/generated/workerAgents.ts`（ts-rs 生成）、`.s11tnext/*`（s11tnext build）
- `src/App.tsx` 等ルーティング、i18n index（`src/i18n/locales/*` の barrel）
- `scripts/module-size-baseline.json`、`src-tauri/src/README.md`（モジュール索引）、`spec/docs/INDEX.md`（必要なら）
- 終了後: 各 WS の書込みが割当範囲外に出ていないことを `git status` で確認。全体 `cargo test`、`bun run test:frontend`、`bun run s11tnext:check` を 1 回。大きな変更なので reviewer による独立レビューを 1 回。

## 8.5 着手順と暫定既定

1. WS-G（現行 inline 経路の guard 穴埋め。他に依存しない）。
2. WS-R の B 区分（単独、WS-E の前）。
3. WS-0（契約固定、coordinator）→ WS-A / WS-B / WS-C（書込み先が分離）→ WS-D → WS-E → WS-F。
4. Open questions のうち Q1・Q2・Q3・Q5 は**ユーザー未回答**。以下の暫定既定で着手し、回答で変更する。WS-G・WS-R・WS-0・WS-A は Q1〜Q5 のどれにも依存しない。
   - Q1: checker は常時実行（安全側。遅延は §7 で計測）。
   - Q2: v1 に承認 UX は入れない（`EscalationRequiresApproval` で終える）。
   - Q3: 非同期結果はホストテンプレートで発話（v1）。会話エージェントによる再構成は後続。
   - Q5: `low_trust_attribute` 以外の警告は一律 failure（計測後に緩める）。
5. §0 の前提 A1〜A4 は設計上の前提であり、着手のゲートではない。

## 9. Open questions / Risks

Open questions（ユーザー判断が必要）
1. Q1 checker の常時実行: plugin decision が `allow` かつ warning なしのページでも checker LLM 呼出しを行うか（遅延とローカル LLM 負荷が増える）。本書は「常時」を既定にしている。
2. Q2 クラウド昇格の承認 UX: 音声会話中に「クラウドで調べてよいか」を聞いて承認を永続ユーザー入力に束縛するフローを v1 に入れるか（本書は入れず `EscalationRequiresApproval` で終える）。
3. Q3 非同期結果の配信形: ホストテンプレート（安全・単調）で発話するか、会話エージェント に再構成させる `worker_report` 会話ジョブにするか（自然だが後者は user 発話なしの会話ジョブという新経路が要る）。
4. （決定済み）Q4 保持期間: 設けない。blocklist は期限なしで、ユーザーのみ削除できる。同一 host の連続 flagged は task 内除外にとどめ、host 単位の永続 blocklist は誤検知の影響が大きいため v1 では作らない。
5. Q5 plugin flag の閾値: `allow_with_warning` で `low_trust_attribute` 以外の警告を一律 failure 扱いにすると誤検知率が高い可能性。計測後に「高 severity のみ」へ緩めるか。

Risks
- R1 ローカル LLM の同時要求: Worker と他の会話 lane が同時に LARM/直結モデルを叩く。LARM の同時実行可否は未検証。
- R2 遅延: inline（モデル 1 往復＋ツール）に対し、worker は 会話エージェント→Worker 複数 step→checker→会話エージェント となり往復が増える。受付発話で体感は補うが、最終回答遅延は計測必須。
- R3 discovery 閾値（`MIN_VEC_SCORE=0.80`）は根拠のない初期値。埋め込みモデル（E5 系、tool_selection と同じ）が無い環境では lexical のみ。
- R4 `steward::repository`/`report::publish` の可視性と `TerminalReport` の空 goal 許容は未検証（§5.3）。
- R5 `queue_progress::enqueue_search` の受付発話は 1 job 1 回のみ（既存挙動）。複数委譲時の受付は 1 回になる。
- R6 claims の命令形検出は簡易で、日本語の婉曲な指示は通過しうる。最終防御は「会話側にツール実行・自動 fetch が無い」こと。
- R7 `ProfileDraft.input_schema` の任意 JSON Schema は `jsonschema` で検証するが、外部 `$ref` 解決は無効化すること（`default-features = false` で remote 解決が無いことを WS-A が確認）。
- 未検証: ts-rs の export 設定の正確な書式、crate 名（テストコマンド）、`EmbeddingProvider` インスタンスを `AppState` から取得する経路。

## 10. 実装結果（2026-10-07）

WS-0 / A / B / C / D / E / F / G と WS-R の B 区分まで実装済み。既定は `web_search_mode='inline'`（従来動作）で、`set_worker_web_search_mode('worker')`（設定画面「Worker Agents」）で切り替える。

### 計画からの差分・実装判断
- 組み込み Agent（Web Search）の投入は `initialize_database` ではなくアプリ起動時（`queue_runtime::spawn` → `worker_lane::startup`、`registry::seed_builtin`。既存編集は上書きしない）。DB version は 45。
- ワーカーのモデル呼び出しは `AppWorkerModel`（会話の直接経路、v1 は 1 段）。ツールは `AppToolRunner`（Web ツールのみ。e2e では fixture に差し替え）。`worker` レーンは 2 本。`lib.rs` の起動復旧は worker レーンを interrupted にし、`executor.recover` が read-only のみ 1 回再投入する。
- 非同期配信は steward の outbox に直接 1 行書く（`steward::repository` が外部から参照できないため）。文面はホストのテンプレートで「（Web由来の情報）」を明記する。
- Blocklist は永続・ユーザー削除のみ。ただし **チェッカーが判定できなかった（timeout・解析失敗）ページは、そのタスク内で失敗扱いにするだけで Blocklist に入れない**（インフラ障害をページの判定として固定しないため）。
- 検索結果 URL は、短い（300 byte 以下）・空白なし・復号した語がガードを通る場合にだけ出典として使える（URL を通じた自由文の持ち込みを防ぐ）。
- 検索・取得の上限はタスク単位（再試行で増えない）。
- worker モード中は inline の Web ツールへフォールバックしない。Agent の候補が無ければ `no_matching_agent` の失敗として会話側へ返す。
- 入力の取消・置換・「やめて」はいずれも `cancel_input_state` を通り、そのワーカータスクも止める。`task_queue::snapshot` は worker レーンのジョブを会話画面に出さない。
- 会話側の Web ツール処理は `queue_runtime/web_steps.rs` に切り出した（挙動不変、サイズ基準のため）。

### 既知の制限（未対応）
- `StaleOffer` の再探索（§5.2）は未実装。Agent 発見は埋め込みなし（語彙のみ、`Degraded`）。`registry::index_embedding` は承認時に呼ばれない。
- 会話ジョブがワーカーを待つ間 `foreground()` を保持する。背景ジョブが書込み待ちのときは、ワーカーの `foreground()` が一時的に待たされ、同期待ちが非同期に落ちることがある（デッドロックにはならない）。
- `admit` は `web_search_mode` を再確認しない。`json_v1` の出力サイズは profile の `output_schema` 次第。
- 暫定既定のままの Q1・Q2・Q3・Q5（§8.5）。

### 検証記録
- `cargo test --lib worker_agents`（107 件）、`conversation_queue_e2e`（既存シナリオ + worker モード: 委譲・注入ヒットの除外・会話文脈に生テキストが入らないことを含む）、契約テスト、IPC 型の整合、フロントのテストと型・lint は通過。
- `cargo test --lib` 全体は 1,880 件通過・12 件失敗。失敗はすべて本変更と無関係（旧会話ランタイム撤去 `rr_*`/`typed_memory_*`、別作業のツール数 `coding_sse_bridge`、並列負荷でのみ落ちる `codex_app_server`/`wasm_host_poc`）。ハングする 4 件（`generative_ui_history…100000`、`rr_09…`、`model_provider_redirects…`、`provider_stream_stops…`）は除外して実行した。
- 独立レビュー 1 回（重大 0、中 3、低 7）。中 3 と低のうち L1・L2・L4〜L7 と L8 の一部を修正済み。
