# MVP 1 Implementation Plan — Situation Shadow Mode

## Status

- Status: Complete — P0 through P6 accepted on 2026-08-26
- Target: first evaluable Situation runtime
- Prerequisite: MVP 0 accepted on 2026-08-26
- Concept: [`plan.md`](./plan.md)
- Previous milestone: [`mvp-0-implementation-plan.md`](./mvp-0-implementation-plan.md)

この文書は、MVP 0で成立したSettings、SQLite、Conversation Event、Voice Runtime、Model / Agent Routeの上に、最初のSituation Runtimeを追加するための実装計画である。

MVP 1ではSituation候補と介入方針を評価可能にするが、自動介入は行わない。Situation判定を既存Chat、TTS、Notification、外部操作の実行条件へ接続するのは後続MVPの責務とする。

## Implementation Record

2026-08-26にP0からP6までを実装し、MVP Acceptanceを完了した。

- contract / database versionを4へ更新し、`situation.runtime`、bounded ledger、structured feedbackを追加した。
- macOS `NSWorkspace` foreground adapter、SAAA-owned Conversation / Voice lifecycle bridge、signal healthとsafe degradationを実装した。
- deterministic classifier、freshness、enter / exit hysteresis、4種類のcounterfactual attention decisionを実装した。
- Shadow invariantをSQLite CHECK、runtime validation、call-graph guardで固定し、自動実行経路を追加しなかった。
- Situation Surface、Settings、timeline、feedback、pause、retention、history clearをSQLite source of truthへ接続した。
- migration、privacy、fixture replay、MVP 0 regression、packaging、packaged Codex live turnを検証した。

再現手順と結果は[`mvp-1-release-evidence.md`](./mvp-1-release-evidence.md)、signalとprivacyの判断は[`adr/0002-situation-signal-privacy.md`](./adr/0002-situation-signal-privacy.md)に保存する。

## 1. Outcome

MVP 1で成立させるループは次のとおり。

```text
Local hard signals
        ↓
Normalized SignalSnapshot
        ↓
Deterministic scoring + hysteresis
        ↓
Stable SituationState + evidence
        ↓
Shadow Interaction Decision
        ↓
Bounded SQLite ledger + Situation UI + user feedback
```

ユーザーは、現在のSituation候補、信頼度、根拠、Signalの取得状態、実行されなかった介入案を確認できる。Runtimeは`would observe / would suggest / would respond / stay silent`を記録するだけで、Situationを理由にProvider、TTS、Notification、Application Adapterを自動実行しない。

## 2. Scope and Safety Invariant

### In scope

- Foreground Applicationの分類済みHard Signal
- SAAA自身のConversation、Generation、Agent、Push-to-talk、Transcription、TTS lifecycle signal
- ユーザーが明示的に有効化したCalendarの粗いbusy / meeting signal
- Signal health、permission、unsupported状態
- 決定論的なSituation scoring
- enter / exit hysteresisによる安定した状態遷移
- Shadow Interaction Decision
- boundedなSituation ledgerとユーザー評価
- Situation SurfaceとSettingsの観測制御

### Safety invariant

MVP 1では次を常に満たす。

```text
shadowMode          = true
actualExecution     = NONE
actualPresentation  = SILENT
automaticModelCall  = false
automaticTTS        = false
automaticNotify     = false
automaticAppAction  = false
```

Shadow decisionは既存の明示的なChat、Push-to-talk、Codex routeを停止または変更しない。ユーザーが開始したMVP 0の処理は従来どおり動作し、そのlifecycleだけがSituation判定の入力になる。

### Explicitly out of scope

```text
実際の自動介入
Always-on listening / Wake Word
System Audio capture
音声、画面、Window title、Calendar本文の保存
Screen / Accessibility observation
LLMによるSituation分類
Meeting transcription / Translation / Overlay
Notification配信
Application automation / Computer Use
contextStill / NightWorkers integration
24/7 background daemon
Cloud telemetry / analytics
```

## 3. Fixed Decisions

### Runtime boundary

- ClassificationとPolicyはRust側のSituation moduleへ置き、React stateをsource of truthにしない。
- 現在の単一Tauri application lifetime内だけで観測する。OS login itemや常駐daemonは追加しない。
- `src-tauri/src/lib.rs`全体の再構成は行わず、新規実装を`src-tauri/src/situation/`へ分離してcommand registrationだけを既存entryへ追加する。
- UIとの通信はversion付きCommand、Snapshot、Event contractにする。
- MVP 1のclassifierは決定論ルールだけを使い、Model ProviderとNetworkを呼ばない。

### Platform boundary

- macOSを最初の実装対象とする。Foreground Applicationはpermission不要のread-only APIを優先し、Accessibility permissionを要求しない。
- platform固有処理は`SignalProvider` adapterへ閉じ込める。
- 未実装OSやpermission拒否時は起動失敗にせず、Signal healthを`unsupported`または`permission-denied`にする。
- Calendarは既定で無効とし、ユーザーのopt-in後だけ読み取る。MVP 1で保持するのは`free | busy | meeting-likely | unavailable`と粗い時間bucketだけで、件名、参加者、場所、メモ、URLはRuntimeへ渡さない。
- System-wide microphone / audio usageは安全で安定したread-only APIをP0で確認できた場合だけ補助Signalにする。必須入力はSAAA自身のcapture / transcription / speech lifecycleとし、System Audio captureへ拡張しない。

### Source of truth and retention

- liveな直近Signalはmemory上のbounded snapshotをsource of truthにする。
- 状態遷移、周期heartbeat、Shadow decision、rule versionはSQLiteのSituation ledgerをsource of truthにする。
- 生のsampleをsampling intervalごとに保存しない。stable stateまたはdecisionが変化した時と、設定したheartbeat時だけ追記する。
- 既定retentionは7日、上限は10,000 entriesとし、古いentryはtransaction内で削除する。
- diagnosticsには件数、health、scene集計だけを含め、時系列、Application identity、Calendar由来情報は含めない。

### Privacy defaults

- Situation monitoringは既定で無効とする。
- Foreground Applicationのraw bundle id、process name、window titleは永続化しない。adapter内で`communication | coding | writing | browser | media | sensitive | other | unknown`へ分類し、raw値を破棄する。
- Sensitive categoryではCalendar以外の補助Contextを使用せず、Shadow decisionを`IGNORE / NONE / SILENT`へ固定する。
- SettingsとSituation Surfaceの両方にPauseを置き、停止時はpollerとsubscriptionを終了する。
- History削除はSituation ledgerとfeedbackだけを対象にし、Conversation historyやSettingsを削除しない。

## 4. Core Contracts

TypeScriptのZod schemaとRustのSerde type / validationを同時に更新し、IPCとSQLite JSONを両側で検証する。

### Signal snapshot

```ts
type SignalHealth =
  | "ready"
  | "disabled"
  | "permission-denied"
  | "unsupported"
  | "degraded";

type SignalSnapshot = {
  sequence: number;
  observedAt: string;
  foreground: {
    category:
      | "communication"
      | "coding"
      | "writing"
      | "browser"
      | "media"
      | "sensitive"
      | "other"
      | "unknown";
    health: SignalHealth;
  };
  conversation: {
    state: "idle" | "user-input" | "model-running" | "agent-running";
  };
  microphone: {
    state: "inactive" | "saaa-capturing" | "saaa-transcribing" | "external-active" | "unknown";
    health: SignalHealth;
  };
  audio: {
    state: "silent" | "saaa-speaking" | "external-media" | "unknown";
    health: SignalHealth;
  };
  calendar: {
    state: "free" | "busy" | "meeting-likely" | "unavailable";
    timeBucket: "now" | "within-15m" | "later" | "none";
    health: SignalHealth;
  };
};
```

`sequence`はprocess内で単調増加させ、古いsampleが新しい状態を上書きしないようにする。文字列、配列、event queueには上限を設ける。

### Situation state

```ts
type SituationState = {
  scene: string;
  confidence: number;
  userAttention: "available" | "busy" | "unknown";
  audioEnvironment: "silence" | "speech" | "multi-speaker" | "media" | "unknown";
  evidence: Array<{
    code: string;
    weight: number;
  }>;
  candidateSince: string;
  stableSince: string;
  updatedAt: string;
  ruleVersion: string;
};
```

`scene`は将来拡張できるbounded identifierとし、MVP 1のbuilt-in valueは`SOLO | MEETING | CONVERSATION | WRITING | CODING | FOCUS | MEDIA | UNKNOWN`とする。UIへraw signalを理由文として渡さず、boundedな`evidence.code`を表示文へ変換する。

### Shadow decision

```ts
type ShadowDecision = {
  mode: "shadow";
  proposedAttention: "IGNORE" | "OBSERVE" | "SUGGEST" | "RESPOND";
  actualExecution: "NONE";
  actualPresentation: "SILENT";
  reasonCodes: string[];
  decidedAt: string;
  policyVersion: string;
};
```

UI表示は次の対応にする。

| `proposedAttention` | UI label |
|---|---|
| `IGNORE` | Stay silent |
| `OBSERVE` | Would observe |
| `SUGGEST` | Would suggest |
| `RESPOND` | Would respond |

### Commands and events

```text
get_situation_snapshot
set_situation_monitoring
watch_situation
report_owned_signal
submit_situation_feedback
clear_situation_history
```

```ts
type SituationEvent =
  | { type: "signalHealthChanged"; source: string; health: SignalHealth }
  | { type: "candidateChanged"; state: SituationState }
  | { type: "stableStateChanged"; entry: SituationLedgerEntry }
  | { type: "shadowDecisionChanged"; entry: SituationLedgerEntry }
  | { type: "monitoringStopped"; reason: string }
  | { type: "failed"; code: string; message: string; recovery: string };
```

`watch_situation`はbounded channelを使用する。遅いconsumerに全sampleを配送せず、最新snapshotを優先し、stable transitionは欠落させない。

## 5. Classification and Policy Rules

### Evaluation order

```text
Hard Signal
    ↓
Signal normalization and freshness check
    ↓
Deterministic weighted rules
    ↓
Candidate Situation
    ↓
Enter / exit hysteresis
    ↓
Stable Situation
    ↓
Shadow Interaction Policy
```

staleまたはunavailableなSignalは負の意味として扱わず、scoreから除外する。Signal不足時は`UNKNOWN`、判断不能時は`IGNORE / NONE / SILENT`を選ぶ。

### Initial rule set

最初のweightは仮説としてversion管理し、user feedbackとfixture評価によって調整する。

| Situation | Positive evidence | Inhibitor |
|---|---|---|
| `CONVERSATION` | explicit Chat、SAAA capture / transcription、model run | none |
| `MEETING` | communication app、Calendar meeting-likely、external mic if available | sensitive app、stale Calendar |
| `CODING` | coding app、active `coding.assist` | communication app + meeting evidence |
| `WRITING` | writing app | active conversation / meeting evidence |
| `MEDIA` | media app、external media if available | active conversation |
| `FOCUS` | coding / writing + Calendar busy | active conversation / meeting evidence |
| `SOLO` | app available、no active audio / conversation / busy event | conflicting evidence |
| `UNKNOWN` | insufficient or contradictory signals | none |

同点時は`CONVERSATION > MEETING > CODING > WRITING > MEDIA > FOCUS > SOLO > UNKNOWN`の固定優先順を使い、rule versionへ含める。明示的なSAAA操作を受動Signalより優先する。

### Hysteresis

- sampling default: 2 seconds
- enter threshold: 70 / 100
- exit threshold: 45 / 100
- candidate hold: 3 consecutive fresh samples
- exit hold: 5 consecutive fresh samples
- stale threshold: sampling intervalの3倍
- transition cool-down: 10 seconds。ただし明示的なSAAA操作は即時反映してよい

閾値は`SituationRuntimeSettings`へ保存するが、UIから編集できるのはMonitoring、Calendar opt-in、retention、privacy controlに限定する。weightやthresholdを一般ユーザー向けUIへ露出させない。

### Shadow policy

- explicit SAAA input / active run: `RESPOND`
- meetingまたはuserAttention=`busy`: `OBSERVE`または`IGNORE`
- availableかつ高confidenceの補助候補: `SUGGEST`
- sensitive、unknown、低confidence、stale signal: `IGNORE`
- すべてのcaseでactual resultは`NONE / SILENT`

MVP 1のPolicyは既存の明示操作をguardしない。後続MVPで実行Policyへ昇格させる前に、false suggestion rateとpermission behaviorを評価する。

## 6. Persistence and Settings

database `user_version`とapplication contract versionを4へ上げ、起動前backupを既存migration flowで作成する。

### Settings document

```text
namespace: situation.runtime
key: default
schemaVersion: 4
```

```json
{
  "enabled": false,
  "sampleIntervalMs": 2000,
  "calendarEnabled": false,
  "retentionDays": 7,
  "maxLedgerEntries": 10000,
  "heartbeatIntervalMs": 300000,
  "sensitiveApplicationCategories": true
}
```

既存documentもcontract version 4へ機械的に移行し、値は変更しない。validation failure時はversion 3 snapshotを破損させず、MVP 0機能を起動できるrecoverable migration errorを返す。

### Situation ledger

```sql
situation_ledger(
  id TEXT PRIMARY KEY,
  observed_at TEXT NOT NULL,
  scene TEXT NOT NULL,
  confidence INTEGER NOT NULL CHECK(confidence BETWEEN 0 AND 100),
  user_attention TEXT NOT NULL,
  audio_environment TEXT NOT NULL,
  proposed_attention TEXT NOT NULL,
  actual_execution TEXT NOT NULL CHECK(actual_execution = 'NONE'),
  actual_presentation TEXT NOT NULL CHECK(actual_presentation = 'SILENT'),
  evidence_json TEXT NOT NULL,
  signal_health_json TEXT NOT NULL,
  rule_version TEXT NOT NULL,
  policy_version TEXT NOT NULL,
  entry_kind TEXT NOT NULL CHECK(entry_kind IN ('transition', 'decision', 'heartbeat'))
)

situation_feedback(
  ledger_id TEXT PRIMARY KEY,
  verdict TEXT NOT NULL CHECK(verdict IN ('accurate', 'inaccurate', 'unsure')),
  corrected_scene TEXT,
  created_at TEXT NOT NULL,
  FOREIGN KEY(ledger_id) REFERENCES situation_ledger(id) ON DELETE CASCADE
)
```

`evidence_json`はcodeとweightだけ、`signal_health_json`はsourceとhealthだけに制限する。free-text feedbackは保存しない。

### Retention

- monitoring開始時、heartbeat追記後、manual cleanup時にretentionを適用する。
- 日数とentry上限の厳しい方を採用する。
- ledger削除時はfeedbackをforeign key cascadeで削除する。
- cleanup failureはmonitoring全体を停止させず、healthを`degraded`にしてUIへ復旧方法を表示する。

## 7. UI Slice

Primary navigationへ`Situation` Surfaceを追加する。

```text
Situation
├─ Monitoring on / paused
├─ Current stable scene + confidence
├─ Would observe / suggest / respond / stay silent
├─ "No automatic actions" safety badge
├─ Evidence codes and signal freshness
├─ Signal health / permission recovery
├─ Bounded transition timeline
├─ Accurate / inaccurate / unsure feedback
└─ Clear Situation history
```

Settingsへ`Situation` sectionを追加する。

```text
Situation Settings
├─ Enable monitoring
├─ Calendar opt-in and permission status
├─ Sampling summary
├─ Retention days
├─ Privacy summary
└─ Clear history
```

raw application identity、window title、Calendar details、audio contentをUIにも表示しない。Situation Surfaceを閉じてもmonitoring設定がenabledならRust runtimeは継続し、Pause時は即時停止する。

## 8. Repository Changes

```text
src/
  features/
    situation/
      SituationPage.tsx
      SituationTimeline.tsx
  lib/
    contracts.ts          # Situation contract追加
    schemas.ts            # v4 / situation validation
    runtime.ts            # Situation commands / channel

src-tauri/src/
  lib.rs                  # stateとcommand wiringのみ
  situation/
    mod.rs                # lifecycle / orchestration
    contracts.rs          # validated native types
    signals.rs            # normalization / freshness
    classifier.rs         # scoring / hysteresis
    policy.rs             # shadow decision
    repository.rs         # ledger / retention / feedback
    platform/
      mod.rs
      macos.rs
      unsupported.rs

tests/
  situation.test.ts

spec/docs/
  adr/0002-situation-signal-privacy.md
  mvp-1-implementation-plan.md
  mvp-1-release-evidence.md
```

MVP 0の型やcommandをSituation moduleへ移す大規模refactorは行わない。新規境界が安定してから別作業として判断する。

## 9. Delivery Order

### P0 — Baseline and Platform Feasibility

Deliverables:

- `bun run check`と`bun run desktop:smoke`のbaseline
- macOS Foreground Application adapter spike
- Calendar permission / coarse projection spike
- System mic / audio read-only signal feasibility record
- signalごとのprivacy data-flow表
- ADR 0002

Exit gate:

- permission不要でforeground categoryを取得できるか、明確なfallbackを決定する
- Calendar detailsをRust contractへ入れずにbusy stateへprojectionできる
- unsupported / denied状態でapplication全体が起動する
- raw title、Calendar本文、audio contentを保存しない設計がレビュー可能である

Stop condition:

- Foreground取得がAccessibility / screen capture permissionを必須とする場合は、その方式を採用しない
- System mic / audio signalがprivate API、process scraping、captureを必要とする場合はMVP 1から外す
- Calendar adapterを安全にbundleできない場合は`unavailable` adapterを出荷し、Foreground + SAAA lifecycleをMVP 1のminimum signal setとする

### P1 — Contract v4 and SQLite Migration

Depends on: P0 privacy decisions

Deliverables:

- Zod / TypeScript / Rust Situation contracts
- Settings namespace追加とversion 3 → 4 migration
- `situation_ledger`、`situation_feedback` migration
- repository、retention、startup load
- pre-migration backup coverage

Exit gate:

- fresh / version 3 / version 4 database testが通る
- invalid Situation settingsで既存SettingsとConversationを破損しない
- ledger rowへraw signalを保存できないvalidationがある
- deleteとretentionがSituation dataだけへ作用する

### P2 — Signal Providers and Lifecycle

Depends on: P1 contracts

Deliverables:

- `SituationRuntime.start / stop / dispose / health`
- foreground adapter
- Calendar opt-in adapterまたは明示的`unavailable`
- existing runtime run、capture、transcription、speech lifecycle bridge
- freshness、sequence、bounded channel
- pause / resume command

Exit gate:

- monitoring disabled時にplatform pollerが起動しない
- enable / pause / app exitでresourceが確実に解放される
- permission denied / adapter failureが他のSignalとMVP 0機能を止めない
- stale eventが最新snapshotを上書きしない

### P3 — Deterministic Classifier and Hysteresis

Depends on: P2 normalized snapshots

Deliverables:

- versioned rule table
- score、tie-break、freshness filtering
- candidate / stable state machine
- enter / exit hysteresis
- fixture replay harness

Exit gate:

- 同じSignal列から常に同じ結果が得られる
- noisy foreground切替でstable sceneが連続反転しない
- insufficient / conflicting / stale signalsが`UNKNOWN`へ落ちる
- explicit SAAA interactionが規定時間内に`CONVERSATION`へ反映される

### P4 — Shadow Policy and Evaluation Ledger

Depends on: P3 stable state

Deliverables:

- versioned Shadow policy
- transition / decision / heartbeat persistence
- retention cleanup
- accurate / inaccurate / unsure feedback
- aggregate evaluation query

Exit gate:

- すべてのdecisionが`NONE / SILENT`を保持する
- Shadow decisionからProvider、TTS、Notification、Application commandへ到達するcode pathがない
- restart後にbounded historyとfeedbackを復元できる
- diagnosticsはaggregateだけを出力する

### P5 — Situation and Settings Surfaces

Depends on: P2 health、P4 ledger

Deliverables:

- Situation Surface
- current state、evidence、health、timeline
- feedback controls
- monitoring / Calendar / retention Settings
- permission recoveryとclear history UX
- keyboard / screen reader labels

Exit gate:

- UI表示がmemory-only React stateではなくRust snapshot / SQLite ledgerと一致する
- Pauseが表示更新だけでなくpollerを停止する
- denied / unsupported / staleの違いと復旧方法を確認できる
- Situationを無効化してもChat、Voice、Codexが従来どおり動く

### P6 — Acceptance and Packaging

Depends on: P0–P5

Deliverables:

- unit / integration / fixture replay / desktop smoke
- resource soak test
- privacy inspection
- migration / backup / restore evidence
- `mvp-1-release-evidence.md`
- `plan.md` Current Milestoneの完了更新

Exit gate:

- Section 11を再現可能な手順で通す
- packaged macOS applicationでmonitoring enable / pause / restartを確認する
- Calendar permission拒否とadapter failureの双方でMVP 0機能が継続する
- network captureまたはtest doubleでSituation Runtimeからoutbound requestがないことを確認する

## 10. Verification Strategy

### Unit tests

- Signal validation、freshness、sequence ordering
- app category projectionとsensitive override
- rule score、tie-break、confidence bound
- candidate / stable hysteresis
- Shadow policy全分岐の`NONE / SILENT` invariant
- retentionのdays / max entries境界
- evidence / health JSONのsizeとfield allowlist

### Integration tests

- version 3 database → backup → version 4 migration
- monitoring settings save → restart → runtime start condition
- platform adapter fixture → classifier → ledger → UI snapshot contract
- transition / heartbeat deduplication
- feedback upsert、restore、cascade delete
- permission denied / unsupported / adapter crash isolation
- existing runtime run → owned signal → Situation projection

### Fixture replay scenarios

1. idle desktop → coding app → short app switch → coding appで`CODING`が安定する。
2. communication app + Calendar meeting-likelyで`MEETING`候補になり、busy policyが`OBSERVE`または`IGNORE`になる。
3. SAAA Push-to-talk開始で`CONVERSATION`が優先され、停止後にhysteresisを経て元のsceneへ戻る。
4. sensitive categoryで他の根拠にかかわらず`IGNORE / NONE / SILENT`になる。
5. Calendar permission拒否後もforeground + owned lifecycleで判定を続ける。
6. stale / out-of-order sampleでstable stateが巻き戻らない。
7. noisyなapp切替でledgerがsample数と同じ速度で増えない。
8. monitoring pause後に新規poll / ledger writeが発生しない。

### Resource and privacy checks

- 8時間fixture soakでevent queue、thread、file descriptor、ledger sizeがboundedである
- idle時pollingのCPU duty cycleをbaselineと比較する
- app exit / pause後にplatform observerとtimerが残らない
- SQLite、backup、diagnosticsにwindow title、Calendar title / attendee / URL、audio、prompt、workspace pathがない
- Situation RuntimeがModel Provider、Codex、Cloud endpointへrequestを出さない

## 11. MVP Acceptance

MVP 1は次がすべて満たされたとき完了とする。

1. Situation monitoringを明示的にenable / pauseでき、設定が再起動後も復元される。
2. Foreground categoryとSAAA所有のConversation / Voice lifecycleをnormalized Signalとして取得できる。
3. Calendar opt-inが利用可能な場合はcoarse stateだけを取得し、拒否または未対応時はdegraded動作する。
4. raw window title、Calendar details、audio content、process listをSQLite、log、diagnosticsへ保存しない。
5. 決定論ルールがSituation候補、confidence、bounded evidenceを返す。
6. hysteresisが短時間のSignal変動によるscene反転を抑止する。
7. stale、insufficient、conflicting signalでは`UNKNOWN`またはsafe defaultになる。
8. Shadow decisionが4種類の介入案を記録し、実際のExecutionとPresentationは常に`NONE / SILENT`である。
9. Shadow modeから自動Model call、TTS、Notification、Application actionを実行できない。
10. Situation Surfaceでcurrent state、根拠、Signal health、bounded historyを確認できる。
11. ユーザーがaccurate / inaccurate / unsureを記録し、再起動後も復元できる。
12. retentionとClear historyがSituation dataだけに作用する。
13. adapter failure、permission拒否、Situation無効化のいずれでもMVP 0のChat、Voice、Codexが動く。
14. version 3 databaseがbackup後にversion 4へ移行し、既存Settings、Conversation、Codex threadを保持する。
15. monitoring pause / application exitでpoller、timer、subscriptionを解放する。
16. Situation RuntimeはCloud service、Hono、RAG、PostgreSQL、pgvector、Embedding modelへ依存しない。

## 12. Suggested PR Slices

| PR | Scope | Depends on |
|---|---|---|
| 1 | P0 feasibility、privacy ADR、fixture contract | — |
| 2 | contract v4、migration、repository | PR 1 |
| 3 | Signal providers、lifecycle、health | PR 2 |
| 4 | classifier、hysteresis、fixture replay | PR 3 |
| 5 | Shadow policy、ledger、feedback、retention | PR 4 |
| 6 | Situation / Settings UI | PR 3, PR 5 |
| 7 | packaging、soak、privacy、release evidence | PR 1–6 |

各PRは対応するunit / integration testを同時に含める。PR 3とPR 4の間でrule parameterを固定し、UI実装中に判定ロジックを調整しない。

## 13. Definition of Done

- MVP Acceptanceと再現手順がrelease evidenceに保存されている。
- classifier / policy / privacy projectionにversionとfixtureがある。
- Monitoringのstart / stop / dispose / healthが実装されている。
- すべてのqueue、history、payload、retentionに上限がある。
- Permission拒否とunsupported platformが通常状態として扱われる。
- Situationを無効化したMVP 0 regression suiteが通る。
- 実データを含まないfixtureで判定を再現できる。
- `plan.md`の責務境界と次のMVP 2 Meeting Modeを先取りしていない。
