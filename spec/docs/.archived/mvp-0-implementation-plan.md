# MVP 0 Implementation Plan

## Status

- Status: Complete — P0 through P6 accepted on 2026-08-26
- Target: first runnable desktop MVP
- Concept: [`plan.md`](./plan.md)
- Storage: SQLite only
- UI reference: `../../../hono-standard` `variant/rag`
- Settings reference: `../../../contextStill`
- Codex runtime reference: `../../../nightWorkers`

この文書は、コンセプトを最初の実装へ変換するための計画である。長期のSituation-aware機能は設計上の拡張先として保持するが、MVP 0の実装対象には含めない。

## Implementation Record

2026-08-26にP0からP6までを実装し、MVP Acceptanceを完了した。

- Tauri 2 + React + TypeScript workspaceを作成した。
- Rust側にSQLiteのmigration、settings document、conversation、messageの永続化commandを実装した。
- UIからSettings、Conversation、MessageをTauri IPC経由でSQLiteへ保存できるようにした。
- `@openai/codex-sdk`をexact versionで固定し、Bun import、app-server model/status、read-only thread start/resume/stream/cancel、native bundle stagingを実装した。
- `bun run check`、`bun run codex:smoke`、packaged Codex live turn、`bun run desktop:smoke`を実行した。

同日にSettings SurfaceをContextStillの操作モデルへ合わせて更新した。

- Chat右ペインの簡易フォームを廃止し、独立したSettings画面へ分離した。
- `General`、`LLM Providers`、`Task Routing`、`Voice`、`Codex SDK`、`Privacy & Security`のsection menuを追加した。
- LLM Providerは複数のOpenAI-compatible endpointを登録できる。
- Settings画面はSQLite snapshotからdraftを作り、未保存差分、Discard、Save Settings、保存成功・失敗表示を持つ。
- Save Settingsはdocument群を一つのSQLite transactionで保存し、成功後にRuntime UIを保存済みの返却値へ同期する。

P1/P2ではZod/Rust二重validation、transactional Settings、OpenAI-compatible SSE、Primary/fallback、Connection Test、local-only destination制約を実装した。P3ではpersistent Chat、stream/cancel/retry/reconcileを、P4ではPush-to-talk、local whisper.cpp、system TTSと各停止経路を実装した。P5ではCodex app-serverの安全なread-only routeを、P6ではmigration backup、diagnostics redaction、keyboard/accessibility、CSP、native packaging smokeを完了した。再現手順と結果は[`mvp-0-release-evidence.md`](./mvp-0-release-evidence.md)に保存する。

## 1. Outcome

MVP 0で成立させる体験は次のとおり。

```text
SettingsでProviderとTask Routeを保存する
        ↓
ChatでTextまたはPush-to-talkを入力する
        ↓
conversation.respondまたはcoding.assistを明示的に選ぶ
        ↓
保存済みEffective Routeで処理する
        ↓
途中状態と結果を同じChatへstreamする
        ↓
通常応答はLocal TTSでも再生できる
```

MVP 0は「設定画面の試作品」ではない。Settings、SQLite、Route解決、Provider実行、Conversation、Voice、UIを一本につないだ最小の縦切りとする。

## 2. Fixed Decisions

### Runtime and UI

- Desktop shellはTauri 2とする。
- UIはReact、Vite、Tailwindを使用する。
- Application runtimeはTypeScriptを中心とし、native audioとcredential accessはRustへ分離する。
- Hono backendは導入しない。
- UIとDesign Systemは`hono-standard`の`variant/rag`から必要部分だけを抽出する。
- Core DomainはTauri IPCの具体APIへ直接依存させない。

### Persistence

- 永続データベースはSAAA所有のSQLiteだけを使う。
- PostgreSQLとpgvectorは依存関係、開発環境、テスト環境へ入れない。
- MVP 0はEmbeddingを必要としないため、SQLite Vectorも起動条件にしない。
- 将来Embedding Providerを有効化した場合だけ、別migrationでSQLite Vectorを追加する。
- Provider、Model、Endpoint、Task Route、Conversation、Turn、Codex thread idはSQLiteへ保存する。
- API keyの実値はOS credential storeに保存し、SQLiteにはcredential referenceとmasked statusだけを保存する。
- Codexの既存認証ファイルをSQLiteへコピーしない。

### Provider Boundary

Providerは用途と能力で分ける。

| Task Route | Provider kind | MVP target | Selection | Side effect |
|---|---|---|---|---|
| `conversation.respond` | `model` | OpenAI-compatible local/cloud adapter | Primary + fallback | none |
| `coding.assist` | `agent` | Codex SDK | explicit only | read-only |
| `speech.transcribe` | `stt` | one local adapter | configured | microphone read |
| `speech.synthesize` | `tts` | one local adapter | configured | audio output |

Codex SDKはLLM completion adapterへ偽装しない。SDK固有のthread、event、cancel、sandboxを`AgentProvider` contractに閉じ込める。

### Codex SDK Safety Profile

MVP 0のCodex routeは次を固定値にする。

```text
sandboxMode       read-only
approvalPolicy    never
networkAccess     disabled
webSearch         disabled
write-capable MCP disabled
route selection   explicit
```

Workspaceを使う場合は、ユーザーがdirectory pickerで明示的に選択した単一rootだけを読み取り対象とする。root未選択時はSAAA管理の空workspaceを使う。`/`、home directory全体、解決前のsymlink、存在しないpathは受け付けない。

Repositoryを変更するCoding TaskはMVP 0では実行しない。将来NightWorkersへ委譲する。

## 3. Runtime Shape

```text
React UI
├─ Chat Surface
└─ Settings Surface
        │ versioned commands/events
        ▼
Application Runtime
├─ Settings Service ─────────────── SQLite
├─ Conversation Service ────────── SQLite
├─ Task Router
│  ├─ Model Provider Adapter
│  └─ Agent Provider Adapter ───── Codex SDK process boundary
├─ Speech Service
│  ├─ STT Provider
│  └─ TTS Provider
└─ Event Stream
        │ native commands/events
        ▼
Rust Native Runtime
├─ Microphone capture
├─ Audio playback
└─ OS credential store
```

UIはProvider SDK、SQLite、audio deviceを直接呼ばない。すべての操作はversion付きCommand、State、EventとしてApplication Runtimeを通す。

## 4. Repository Layout

最初に次の境界でscaffoldする。実装中に名称を調整してよいが、依存方向は維持する。

```text
apps/
  desktop/
    src/
      features/chat/
      features/settings/
      design-system/
    src-tauri/

packages/
  contracts/
    src/commands/
    src/events/
    src/settings/
  domain/
    src/conversation/
    src/routing/
    src/providers/
    src/speech/
  runtime/
    src/application/
    src/providers/model/
    src/providers/codex/
    src/providers/stt/
    src/providers/tts/
  persistence/
    src/sqlite/
    migrations/

spec/docs/
  plan.md
  mvp-0-implementation-plan.md
```

依存方向:

```text
UI / Native / Provider adapters
              ↓
       Application services
              ↓
       Domain + Contracts
```

`domain`からReact、Tauri、SQLite、Codex SDK、特定STT/TTS engineをimportしない。

## 5. Core Contracts

実装前にZod schemaとTypeScript typeを同じsourceから定義する。IPC boundaryとSQLite JSON columnから取得した値は必ずschema validationを通す。

### Task and Provider

```ts
type TaskRouteId =
  | "conversation.respond"
  | "coding.assist"
  | "speech.transcribe"
  | "speech.synthesize";

type ProviderKind = "model" | "agent" | "stt" | "tts" | "embedding";
type ProviderLocation = "local" | "cloud";

type ProviderCapabilities = {
  streaming: boolean;
  threadResume: boolean;
  audioInput: boolean;
  audioOutput: boolean;
  workspaceRead: boolean;
  workspaceWrite: boolean;
  network: boolean;
};
```

Route validationはTaskが要求するcapabilityとProvider kindの一致を検証する。`coding.assist`へModel Provider、`conversation.respond`へAgent Providerを保存できないようにする。

### Effective Route

```ts
type EffectiveRoute = {
  taskId: TaskRouteId;
  primary: ResolvedProviderTarget;
  fallbacks: ResolvedProviderTarget[];
  timeoutMs: number;
  source: "saved" | "safe-default";
};
```

Chat実行時はeditable settingsを再解釈せず、Settings Serviceがvalidation済みsnapshotから生成した`EffectiveRoute`だけを使う。Settings UIにも同じ結果を表示する。

Local-only routeではCloud Providerを暗黙fallbackに追加しない。Cloudへの遷移は保存済みrouteに明示されている場合だけ許可する。

### Conversation Event

```ts
type ConversationEvent =
  | { type: "turn.started"; turnId: string; route: EffectiveRouteSummary }
  | { type: "transcript.delta"; turnId: string; text: string }
  | { type: "transcript.final"; turnId: string; text: string }
  | { type: "assistant.delta"; turnId: string; text: string }
  | { type: "assistant.final"; turnId: string; text: string }
  | { type: "agent.activity"; turnId: string; activity: AgentActivity }
  | { type: "speech.started" | "speech.completed"; turnId: string }
  | { type: "turn.cancelled"; turnId: string; reason: string }
  | { type: "turn.failed"; turnId: string; error: PublicRuntimeError };
```

Provider固有payloadをそのままUIへ渡さない。Codex eventはbounded、redactedな`AgentActivity`へ変換する。

### Lifecycle

Model、Agent、STT、TTS adapterは共通して次を提供する。

```text
available / health
start or execute
cancel through AbortSignal
dispose
```

同一turnでGenerationとSpeechを個別に停止できるよう、AbortControllerを処理単位で所有する。

## 6. SQLite Design

SQLiteはWAL、foreign key、busy timeoutを有効にし、migrationをApplication起動時にtransaction内で適用する。schema version不一致時に無言で再作成しない。

### Initial Tables

```sql
settings_documents(
  namespace TEXT NOT NULL,
  key TEXT NOT NULL,
  schema_version INTEGER NOT NULL,
  value_json TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  PRIMARY KEY(namespace, key)
)

credential_references(
  provider_id TEXT PRIMARY KEY,
  credential_ref TEXT NOT NULL,
  source TEXT NOT NULL,
  masked_value TEXT,
  updated_at TEXT NOT NULL
)

workspaces(
  id TEXT PRIMARY KEY,
  label TEXT NOT NULL,
  canonical_path TEXT NOT NULL UNIQUE,
  read_only INTEGER NOT NULL,
  created_at TEXT NOT NULL,
  last_used_at TEXT
)

conversations(
  id TEXT PRIMARY KEY,
  title TEXT,
  task_mode TEXT NOT NULL,
  workspace_id TEXT,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  FOREIGN KEY(workspace_id) REFERENCES workspaces(id)
)

conversation_turns(
  id TEXT PRIMARY KEY,
  conversation_id TEXT NOT NULL,
  status TEXT NOT NULL,
  task_id TEXT NOT NULL,
  provider_id TEXT NOT NULL,
  model TEXT,
  input_text TEXT NOT NULL,
  output_text TEXT,
  error_code TEXT,
  created_at TEXT NOT NULL,
  completed_at TEXT,
  FOREIGN KEY(conversation_id) REFERENCES conversations(id)
)

provider_sessions(
  conversation_id TEXT NOT NULL,
  provider_id TEXT NOT NULL,
  provider_thread_id TEXT NOT NULL,
  status TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  PRIMARY KEY(conversation_id, provider_id),
  FOREIGN KEY(conversation_id) REFERENCES conversations(id)
)
```

`conversation_turns.status`は`queued | running | completed | cancelled | failed`に限定する。Applicationが異常終了した場合、次回起動時に`running`を`failed`へreconcileし、UIへ中断理由を表示する。

Streaming deltaは永続化しない。確定transcript、最終応答、turn lifecycleだけをtransactionで保存し、SQLite write amplificationを避ける。

### Settings Documents

MVP 0では次のnamespaceを持つ。

```text
providers.model
providers.agent
providers.stt
providers.tts
routing.tasks
speech.capture
speech.playback
ui.preferences
```

JSONを使っても無型にはしない。namespaceごとのZod schema、schema version、migrationを必須にする。

### Vector Policy

MVP 0 migrationにはvector tableを入れない。Embedding modelが実際に導入される時点で次を一組として追加する。

- extension availability check
- embedding provider/model identity
- dimension
- vector table/index
- rebuild command
- degraded state

Embedding未設定やextension未導入はText Chat、Voice Chat、Codex routeを停止させない。

## 7. Settings and Routing Slice

### Settings UI

Settings Surfaceは次のsectionに絞る。

```text
Model Providers
  enabled, label, local/cloud, endpoint, model, credential status

Codex SDK
  enabled, installed, authenticated, model, runtime mode, health result

Speech
  input device, STT model, output device, TTS voice, test buttons

Task Routing
  conversation.respond primary/fallback
  coding.assist codex-sdk
  timeout/resource limits
  effective route preview
```

保存操作は`validate → write transaction → rebuild effective snapshot → publish settings.changed`の順とする。validation failure時は既存snapshotを維持する。

### First Model Adapter

通常会話はOpenAI-compatible HTTP adapterを最初に実装する。endpoint、model、optional credential、locationを設定できるため、local serverとcompatible cloud endpointの双方を同じtransport contractで扱える。

ただし、Provider IDとlocationは設定上明示し、local endpointをCloudへ自動置換しない。request timeout、stream parse error、HTTP status、rate limitをpublic error codeへ正規化する。

### Connection Test

Connection testはChat turnを作らず、短いtimeoutで以下を返す。

```ts
type ProviderHealth = {
  status: "ready" | "degraded" | "unavailable";
  checkedAt: string;
  latencyMs?: number;
  code?: string;
  message: string;
};
```

API key、Authorization header、Codex auth file内容、local filesystem pathをlogへ含めない。

## 8. Text Conversation Slice

1. User messageをSQLiteへ`running` turnとして保存する。
2. `conversation.respond`のEffective Routeをsnapshotから解決する。
3. Primary Providerをstreaming実行する。
4. retry可能なProvider failureだけ、保存済みfallbackを順に試す。
5. UIへ`assistant.delta`を送る。
6. `assistant.final`で最終文を保存し、turnを`completed`にする。
7. cancel時はAbortSignalを伝播し、turnを`cancelled`にする。
8. failure時はsecretを除いたerror codeと短い理由を保存する。

Fallback前に途中出力が表示された場合は自動fallbackしない。二重応答を避け、失敗として明示する。

Chat Surfaceの最小要素:

- Conversation listは直近分だけをbounded表示
- Task mode selector
- Workspace selectorはCoding mode時だけ表示
- Message list
- Typed composer
- Push-to-talk
- Live/final transcript
- Provider/model badge
- Recording、Generating、Agent working、Speaking status
- Stop、Retry
- Codex activityの折りたたみ表示

## 9. Voice Slice

MVP 0はPush-to-talkだけを実装する。Always-on、wake word、system audio、speaker diarizationは実装しない。

### Audio Lifecycle

```text
Idle
 → Capturing
 → Transcribing
 → Finalized
 → Generating
 → Speaking
 → Idle
```

どの状態からもStopで`Idle`へ戻れる。audio buffer、event queue、conversation historyには上限を設ける。

### STT

P0 spikeで`sherpa-onnx`と`whisper.cpp`系のうち、対象OSで次を満たす一つを固定する。

- offline execution
- distributable model/license
- partialまたは短周期のinterim transcript
- AbortSignal相当の停止
- Tauri packageから起動可能

partialは`transcript.delta`、確定結果は一度だけ`transcript.final`として発行する。確定文字列はtyped messageと同じConversation commandへ渡す。

### TTS

対象OSでoffline利用できる一つのadapterを固定し、voiceとspeedをSettingsへ保存する。応答テキスト確定後に再生し、生成途中の細切れ音声合成はMVP 0では行わない。

`coding.assist`はactivityやcodeを含むため、既定ではTTSを無効にする。ユーザーが明示的に有効化した場合も最終summaryだけを読み上げる。

## 10. Codex SDK Slice

Codex SDKはlocal Codex agentのthreadを開始・継続・再開するserver-side TypeScript libraryである。実装は公式SDK contractを基準とし、NightWorkersの実装からthread resume、stream event projection、secret redactionの考え方だけを参照する。NightWorkersのMission lifecycleは持ち込まない。

公式情報: [Codex SDK documentation](https://learn.chatgpt.com/docs/codex-sdk)

### P0 Compatibility Gate

公式要件はNode.js 18以上であるため、最初にisolated smokeを実行する。

1. SDK versionをexact pinする。
2. Bun processからimport、`startThread`、stream、cancelを試す。
3. auth、child process、event stream、shutdownを確認する。
4. すべて通ればin-process adapterを採用する。
5. 一つでもruntime incompatibilityがあればNode.js 20 sidecarを採用する。

互換性問題をSDK wrapper内の`any`や無検証castで隠さない。判定結果をADRへ残す。

Node sidecar採用時のprotocolはstdin/stdoutのlength-bounded JSON Linesとし、request id、schema version、cancel commandを持たせる。sidecarはloopback portを公開せず、SAAAのchild processとして明示的に終了させる。

### Thread Lifecycle

```text
No session
  → startThread
  → thread.started event
  → provider thread idをSQLiteへ保存

Existing session
  → resumeThread(saved id)
  → runStreamed
  → resume不能なら理由を表示
  → ユーザー確認なしのsilent fallbackはしない
```

新規threadへの切替はUI上の「New coding thread」で明示する。別Conversationのthread idを共有しない。

### Event Mapping

最低限、次を共通eventへ写像する。

| SDK event family | SAAA event |
|---|---|
| thread/turn start | `turn.started`, `agent.activity` |
| agent message delta/final | `assistant.delta`, `assistant.final` |
| command execution | redacted `agent.activity` |
| MCP call | redacted `agent.activity` |
| file change | policy violation + `turn.failed` in MVP 0 |
| turn completed | persisted `completed` |
| turn failed/error | normalized `turn.failed` |

Raw command outputやMCP resultはサイズ上限を設け、secret patternをredactする。SDK event object全体をlogまたはSQLiteへ保存しない。

### Authentication

- Codex login/authの有無をhealth checkで確認する。
- 認証ファイルの本文やtokenは読んでUIへ返さない。
- Settingsには`configured/source/checkedAt`だけを保存する。
- 認証不足時は`coding.assist`だけをdisabledにし、通常ChatとVoiceは継続する。

## 11. Design System Import

`hono-standard`の`variant/rag`をUI referenceとして、以下をcomponent単位で棚卸しする。

```text
Import or adapt
  token definitions
  Button / IconButton
  Input / TextArea / Select
  message presentation
  composer
  panel / status / loading / error patterns

Rebuild for SAAA
  Chat state machine
  transcript presentation
  task mode selector
  Provider settings forms
  Codex activity panel
  Tauri command/event hooks

Do not import
  Hono routes or client
  RAG hooks and source ingestion
  PostgreSQL / pgvector client
  RAG authentication
  Agentic Search backend
```

移植したcomponentはSAAA namespaceへ置き、upstream repositoryをruntime dependencyにしない。tokenとprimitiveの由来はfile headerまたはdocsに記録する。

## 12. Delivery Order

### P0 — Bootstrap and Risk Spikes

Deliverables:

- workspace、package manager、Tauri/React shell
- lint、typecheck、unit test、desktop smoke commands
- SQLite driverとmigration runnerの選定
- Codex SDK Bun/Node compatibility ADR
- local STT/TTS adapter selection record
- `hono-standard` UI inventory

Exit gate:

- empty desktop appが起動する
- temporary DBへmigrationを適用できる
- Codex SDK runtime boundaryをin-processかNode sidecarのどちらかへ確定する
- STT/TTSの配布可能性と停止方法を確認する

### P1 — Contracts and SQLite

Deliverables:

- Zod command/event/settings schema
- SQLite migrations
- Settings Repository
- Conversation Repository
- Provider Session Repository
- startup reconciliation

Exit gate:

- save/restart/loadのintegration testが通る
- invalid settingsで既存snapshotが破損しない
- raw secretがSQLiteとlogに存在しない

### P2 — Settings and Model Routing

Deliverables:

- Settings Surface
- OpenAI-compatible Model Provider
- `conversation.respond` resolver
- Primary/fallback execution
- connection testとEffective Route preview

Exit gate:

- UIで保存したrouteをRuntimeが実際に使う
- local-only routeのnetwork destination testが通る
- Provider failure reasonがUIへ表示される

### P3 — Persistent Text Chat

Deliverables:

- Conversation Service
- Chat Surface
- typed message streaming
- cancel、retry、history restore
- bounded event projection

Exit gate:

- app再起動後にconversationと完了済みturnを復元できる
- running turnを中断済みとしてreconcileできる
- cancel後にbackground generationが残らない

### P4 — Voice Loop

Deliverables:

- microphone device selection
- Push-to-talk capture
- local STT delta/final
- local TTS playback
- recording/transcribing/speaking stop

Exit gate:

- 音声入力が同じConversationへ文字として残る
- local routeで音声がCloudへ送られない
- device errorとmodel missingを復帰可能な状態で表示する

### P5 — Codex Agent Route

Deliverables:

- Codex SDK adapterまたはNode sidecar
- health/auth status
- `coding.assist` explicit route
- read-only workspace selection
- streamed event mapping
- thread id persistence/resume
- cancelとprocess cleanup

Exit gate:

- start、stream、cancel、resumeのcontract testが通る
- repositoryに変更を作れない
- Network、Web Search、write-capable MCPが無効である
- Codex利用不能でも通常ChatとVoiceが動く

### P6 — Packaging and MVP Acceptance

Deliverables:

- clean-machine package smoke
- database migration/backup failure handling
- provider/model missing UX
- keyboard and basic accessibility pass
- diagnostics export with secret redaction
- acceptance evidence

Exit gate:

- Section 14の全Acceptanceを再現可能な手順で通す
- PostgreSQL、pgvector、Hono serviceなしでdesktop packageが起動する

## 13. Verification Strategy

### Unit Tests

- settings schemaとversion migration
- Provider capabilityとTask Routeの不一致拒否
- Effective Routeとfallback順序
- local-only routeのCloud拒否
- Conversation state transition
- Codex event mapping、redaction、size limit
- audio state transitionとcancel idempotency

### Integration Tests

- SQLite fresh migration、upgrade、transaction rollback
- Settings save → cache rebuild → route execution
- app restart → settings/history/thread id restore
- OpenAI-compatible streaming fixture
- Primary failure → fallback
- Codex start/resume/stream/cancel fixture
- Node sidecar crash/restart。ただしsidecar採用時のみ
- STT/TTS fixtureとnative smoke

### End-to-End Scenarios

1. Local modelを登録し、`conversation.respond`へ設定して再起動する。
2. Text messageを送り、保存済みmodel名と応答をChatで確認する。
3. Push-to-talkし、delta、final、assistant response、TTSを確認する。
4. GenerationとSpeechをそれぞれ停止する。
5. Primaryを失敗させ、明示済みfallbackだけが使われることを確認する。
6. Codex authなしでCoding modeだけがdisabledになることを確認する。
7. Codex authありでread-only threadを開始し、streamとcancelを確認する。
8. Applicationを再起動し、同じCodex threadを再開する。
9. workspaceへwriteを要求し、変更されずに拒否されることを確認する。
10. PostgreSQL、Hono、Embedding serviceなしで全シナリオを実行する。

### Security Checks

- DB、log、diagnostics snapshotにAPI key/tokenがない
- Provider endpointはschemeとhostをvalidationする
- local-only routeのoutbound destinationを記録・検証する
- selected workspace rootをcanonicalizeする
- Codex child environmentはallowlist方式で構築する
- stdout/stderrとevent payloadにsize limitを持つ
- cancel/exit時にchild processとaudio deviceを解放する

## 14. MVP Acceptance

MVP 0は次がすべて満たされたとき完了とする。

1. Model Provider、Model、Endpoint、Task RouteをSettings UIで編集できる。
2. 設定がSQLiteへ保存され、再起動後に復元される。
3. 保存済み設定から生成したEffective RouteをUIとRuntimeが共有する。
4. `conversation.respond`でPrimaryと明示的fallbackが動く。
5. Text Chatのstream、cancel、retry、history restoreが動く。
6. Push-to-talkの途中文字起こしと確定文字起こしが同じChatへ出る。
7. 確定文字起こしから応答を生成し、local TTSで再生できる。
8. Recording、Generation、Agent、Speechを停止できる。
9. Codex SDKのinstalled/auth/health状態を確認できる。
10. `coding.assist`を明示選択してread-only Codex threadを実行できる。
11. Codexの応答とbounded activityをChatへstreamできる。
12. Codex thread idをSQLiteへ保存し、再起動後にresumeできる。
13. Codex routeからworkspace変更、Network、Web Search、write-capable MCPを実行できない。
14. Codex、STT、TTSの一つが利用不能でも、影響しない機能は起動できる。
15. local-only routeで会話音声とpromptをCloudへ送らない。
16. Hono、RAG、PostgreSQL、pgvector、Embedding modelなしでMVPが動く。

## 15. Explicit Deferrals

```text
Automatic task classification
Automatic Situation Classification
Always-on listening / Wake Word
Meeting detection / system audio / diarization
Screen and Accessibility observation
RAG / ingestion / Embedding / SQLite Vector
contextStill runtime integration
NightWorkers task execution
Codex workspace-write
Codex write-capable MCP
Computer Use
Application automation
Long-term personal memory
Generative Surface catalog
Cloud STT as the default path
```

## 16. Implementation Risks and Stop Conditions

### Codex SDK runtime compatibility

Bun上で公式supportを仮定しない。P0で確証を得られなければNode sidecarへ切り替える。SDK versionを曖昧なrangeで追従しない。

### Local voice packaging

精度だけでengineを選ばない。model size、license、CPU負荷、初回setup、cancel、Tauri packagingがExit gateを満たさなければ別adapterへ切り替える。

### UI source coupling

`hono-standard`のRAG data modelやHono clientを移植し始めた場合は作業を止め、token/primitive/message UIへ範囲を戻す。

### Secret storage

OS credential storeを安定して利用できない場合、Cloud credentialのDB平文保存へ退行しない。Environment-only credentialを暫定laneとし、制約をUIに表示する。

### Scope growth

MVP Acceptanceに不要なSituation、RAG、automation、workspace-writeは次のmilestoneへ送る。先行してdomain名やempty interfaceだけを大量に作らない。

## 17. Definition of Done

- 実装とschemaに対応するunit/integration/E2E testがある。
- Clean databaseとupgrade databaseの双方で起動する。
- Settingsが実行経路へ接続され、UIだけのmockで終わっていない。
- 全streaming処理を停止でき、process/audio resource leakがない。
- Errorは復旧方法を伴ってUIへ表示される。
- raw secretとunbounded provider payloadを保存・出力しない。
- SQLite fileのownershipとbackup対象が文書化されている。
- Codex SDK runtime判定、STT/TTS選定、SQLite driver選定をADRに残す。
- `plan.md`と実装が矛盾していない。
- MVP Acceptanceの再現手順と結果をrelease evidenceとして保存する。
