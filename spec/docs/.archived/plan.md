# Situation-Aware Ambient Agent Runtime

## Project Concept & Direction

この文書は、最終的な製品像と、その方向へ進むための責務境界を示すコンセプト文書である。

すべての機能を最初から実装するための固定仕様ではない。特に、Situationの自動推定、Application Automation、NightWorkers連携、Capability Factoryは長期方向として保持し、最初のMVPでは扱わない。

最初のMVPは、次の一本の体験を成立させることに集中する。

```text
LLM / Agent ProviderとTask Routingを設定する
        ↓
文字または音声で話しかける
        ↓
文字起こしがChat UIへ反映される
        ↓
選択されたTask RouteでLLMまたはCodex SDKを実行する
        ↓
Chat UIへ表示し、必要な応答は音声でも返す
```

この最小ループを、後続のSituation、Policy、Context、Capabilityへ拡張できるRuntimeの最初の縦切りとする。

## 1. Vision

ユーザーのPC環境に常駐し、

* 音声
* 画面
* アプリケーション
* 会話
* 作業状態
* カレンダー
* OS状態
* 外部ツール

などから現在の状況を理解し、

> **今、このAIは何をするべきか。あるいは、何もしないべきか。**

を継続的に判断するPersonal AI RuntimeをOSSとして構築する。

本プロジェクトは単なるChat Assistantではない。

また、

* Alexa型Wake Word Assistant
* Coding Agent
* Memory Engine
* Computer Use専用Agent
* Meeting Assistant
* Email Copilot

のいずれか一つでもない。

それらを必要な状況で適切に利用する、

> **Local-first, Situation-aware Ambient Agent Runtime**

を目指す。

ただし、最初のMVPが証明するのはAmbient Intelligence全体ではない。

最初に証明するのは、

1. Local / Cloudを選択できるLLM Runtime設定
2. Taskごとの明示的なModel Routing
3. Codex SDKを通常LLMとは異なるAgent Providerとして安全に選択できること
4. 音声と文字が同じConversationへ入ること
5. UI、音声Runtime、LLM / Agent Runtime、永続設定を後からSituation-awareへ拡張できること

である。

長期Visionと最初のMVPを混同しない。

---

## 2. Fundamental Model

従来型AI:

```text
User Input
    ↓
LLM
    ↓
Response
```

本プロジェクト:

```text
Perception
    ↓
Situation
    ↓
Interaction Policy
    ↓
Assist Planning
    ↓
Capability Resolution
    ↓
Execution
    ↓
Presentation
```

重要なのは、LLMそのものではない。

本プロジェクトの中核価値は、

1. 現在の状況を理解する
2. 介入すべきか判断する
3. 必要なCapabilityを選択する
4. 必要なら新しいCapabilityを獲得する
5. 最も邪魔にならない方法で結果を提示する

ことにある。

---

## 3. Local-first Principle

AI処理は原則としてローカルで実行する。

対象には以下を含む。

```text
LLM
STT
TTS
Translation
Embedding
Classification
Intent Detection
Situation Classification
Summarization
Simple Vision Processing
```

Cloud AIは主処理系ではない。

基本ルーティング:

```text
Task
 ↓
Deterministic local processing possible?
 ├─ YES
 │   ↓
 │  Local processing
 │
 └─ NO
      ↓
Local model available?
 ├─ YES
 │   ↓
 │  Local model
 │
 └─ NO
      ↓
Alternate local model available?
 ├─ YES
 │   ↓
 │  Local fallback
 │
 └─ NO
      ↓
Cloud fallback permitted?
 ├─ YES → Cloud
 └─ NO  → Graceful failure
```

デフォルト:

```text
Local Preferred
```

ユーザー設定として最低限、

```text
Local Only
Local Preferred
Cloud Allowed
Cloud Preferred
```

を持てるようにする。

Sensitive ContextについてはCloud送信を禁止できること。

---

## 4. Runtime Responsibility

本プロジェクトが所有する中核機能は以下とする。

```text
Perception
Situation Manager
Interaction Policy
Assist Planner
Capability Registry
Capability Router
Capability Factory
Model Router
Context Broker
Presentation Manager
Application Adapters
Local Voice Runtime integration
Permission / Security
Runtime Lifecycle
Observability
```

逆に、既存の別プロジェクトで担当する領域は本体へ再実装しない。

---

## 5. External Responsibility — contextStill

再利用可能なKnowledgeとTask固有ContextのCompilationは **contextStill** に委譲する。

contextStillは現時点では、AIコーディングエージェント向けのrule / procedure / lesson / agent logを扱うKnowledge Control Planeである。会議、人物、予定、メールなどを無条件に保存する汎用Personal Memoryとしては扱わない。

本プロジェクト内ではcontextStillと競合するKnowledge Distillation Engineを構築しない。一方、現在のSituation、Conversation、音声文字起こしなど、Ambient Runtime固有の短命Contextは本体が所有する。

## 本プロジェクト側の責務

```text
Situation
 ↓
必要なContextを判断
 ↓
contextStill query
 ↓
Relevant Context
 ↓
Current Taskへ利用
```

また、再利用可能なrule / procedure / lessonとして一般化でき、保存価値があるObservationだけを、

```text
Observation Candidate
 ↓
contextStill
```

へ渡す。

単なる会話履歴、音声文字起こし、画面内容、個人情報をObservation Candidateとして自動送信しない。

## contextStill側の責務

```text
Knowledge persistence
Long-lived rule / procedure / episode memory
Semantic Search
Vector Search
Full-text Search
Knowledge Distillation
Deduplication
Scoring
Staleness
Context Compilation
Knowledge Lifecycle
```

本体内部に保持してよいMemoryは短命なものに限定する。

```text
Current Situation
Recent Utterances
Current UI State
Current Task State
Temporary Meeting Context
Temporary Working Context
```

最初のMVPではcontextStill Retrieval / Candidate Registrationを実行経路へ含めない。設定画面とRouting設計はcontextStillの実装を参照するが、SAAA自身の設定はSAAA所有のSQLiteへ保存し、contextStillのDBを直接共有しない。

---

## 6. External Responsibility — NightWorkers

Software Engineering実行は **NightWorkers** に委譲する。

本プロジェクト自身ではCoding Agentを実装しない。

以下も本体の責務外とする。

```text
Repository Investigation
Architecture Planning for implementation
Code Generation
Implementation
Test execution
Code Review
Security Review
Coding Mission execution
Software Engineering Task Graph
```

Coding関連の要求はCapability Routerを通じてNightWorkersへ渡す。

NightWorkersは単純なCode Generation APIではない。Task、明示的な実行権限、Plan Review、専用Git Workspace、Implementation Queue、Test、Review、Security、Git Closeoutを所有するHuman-governed Development Control Planeとして扱う。

本プロジェクトは、NightWorkers内部のMission Pilot、Planning、Review、Completion判断を再実装しない。

本プロジェクトがNightWorkersへ渡すもの:

```text
Goal
Capability Contract
Inputs / Outputs
Permissions
Integration Points
Acceptance Criteria
Presentation Requirements
Local-first Constraints
```

NightWorkersから受け取るもの:

```text
Task / Run Reference
Current Lifecycle State
User Attention Required
Artifact / Diff Reference
Verification / Review Result
Closeout Result
```

NightWorkersの承認やReview GateをAmbient Runtimeが迂回してはならない。

```text
User
 ↓
「このバグを直して」
 ↓
Ambient Runtime
 ↓
Situation = CODING
 ↓
Capability Router
 ↓
NightWorkers
 ↓
Implement / Test / Review
 ↓
Result
 ↓
Presentation Manager
```

---

## 7. Self-Extending Capability Model

本プロジェクトの重要な長期目標の一つ。

最初のMVPの完了条件には含めない。

ユーザーが、

> 「こういうことができるようにしたい」

と要求したとき、そのCapabilityが存在しなければ、

**不足している能力を新しく実装できるようにする。**

基本フロー:

```text
User Requirement
      ↓
Assist Planner
      ↓
Capability Registry
      ↓
Capability exists?
   ┌───────┴────────┐
  YES              NO
   │                │
Execute       Capability Gap
                    ↓
              Capability Spec
                    ↓
                Plan Draft
                    ↓
              NightWorkers
                    ↓
          Implement / Test / Review
                    ↓
           Capability Artifact
                    ↓
             Validation Gate
                    ↓
             Capability Registry
                    ↓
                 ACTIVE
```

これにより、本システムは固定機能の集合ではなく、

> **ユーザーの要求によって能力を増やしていくRuntime**

となる。

初期段階では、生成済みCapabilityを自動でACTIVEにしない。

```text
Build
 ↓
Validate
 ↓
STAGED
 ↓
User Review / Approval
 ↓
ACTIVE
```

Capabilityのinstallとactivateを分離し、Core更新や高権限Capabilityの暗黙有効化を禁止する。

---

## 8. Core Must Not Self-Modify

自己拡張は、

```text
core source codeを書き換える
```

ことを意味しない。

Core Runtimeは安定したKernelとして維持する。

```text
Stable Core
├─ Situation
├─ Policy
├─ Routing
├─ Capability Registry
├─ Capability Factory
├─ Presentation
└─ Security
```

新機能は、

```text
skills/
plugins/
adapters/
providers/
```

として追加する。

原則:

> **Kernelは小さく安定させ、能力は外付けする。**

---

## 9. Capability vs Skill

CapabilityとSkillは区別する。

## Capability

AIができることを表す論理的な能力。

例:

```text
mail.rewrite
meeting.translate
excel.detect_anomaly
browser.research
desktop.capture_screen
```

## Skill

Capabilityを実現する具体的な実装。

例:

```text
mail.rewrite
 ├─ local-llm-rewrite
 └─ cloud-rewrite-fallback
```

あるいは、

```text
mail.read_current
 ├─ outlook-addin
 ├─ microsoft-api
 ├─ accessibility
 └─ computer-use
```

Capability Routerは状況に応じて最適なSkillを選択する。

---

## 10. Capability Levels

新しい要求をすべてコード生成へ送らない。

最も軽い方法でCapabilityを実現する。

## Level 1 — Declarative Skill

コード不要。

```text
Existing Tool
+
Prompt
+
Procedure
+
Policy
```

## Level 2 — Composite Skill

既存Capabilityを組み合わせる。

```text
STT
+
Translation
+
contextStill Retrieval
+
Overlay
```

## Level 3 — Script Skill

TypeScript等による軽量実装。

NightWorkersへ実装を依頼する。

## Level 4 — Application Adapter

Outlook、Excel、Zoom、Browserなどへの専用統合。

## Level 5 — Native Capability

Rust / OS APIを必要とする。

```text
Audio Capture
Screen Capture
Accessibility
System Audio
Mouse / Keyboard
```

高い権限を必要とするため厳格に扱う。

---

## 11. Capability Definition

Capabilityには機械可読なManifestを持たせる。

例:

```yaml
id: meeting.translate
version: 1.0.0

description: >
  Translate detected foreign-language meeting speech
  into the user's preferred language.

inputs:
  - transcript_stream

outputs:
  - translated_stream

permissions:
  - audio.read
  - overlay.write

risk:
  level: 0

runtime:
  type: local

providers:
  preferred:
    - local-translation-model

presentation:
  - overlay

health_check:
  enabled: true
```

新規Capability生成時もこの形式のSpecを最初に作る。

---

## 12. Capability Factory

不足能力を生成するSubsystem。

```text
Capability Gap Analyzer
        ↓
Capability Spec Compiler
        ↓
Implementation Request
        ↓
NightWorkers
        ↓
Artifact Validator
        ↓
Staging
        ↓
Capability Installer
        ↓
Capability Registry
```

NightWorkersへ自然言語だけを投げない。

可能な限り、

```text
Goal
Inputs
Outputs
Permissions
Constraints
Integration points
Acceptance criteria
Tests
Presentation requirements
Local-first requirements
```

を含むCapability Specへ変換する。

---

## 13. Capability Installation Gate

NightWorkersのテスト成功だけで即インストールしない。

```text
Artifact
 ↓
Contract Test
 ↓
Permission Validation
 ↓
Security Validation
 ↓
Resource Limit Validation
 ↓
Integration Test
 ↓
STAGED
 ↓
Smoke Test
 ↓
User / Policy Approval
 ↓
ACTIVE
```

更新時にはRollback可能にする。

```text
Capability v1.3 ACTIVE
 ↓
v1.4 install
 ↓
failure
 ↓
rollback
 ↓
v1.3 ACTIVE
```

---

## 14. Situation Manager

最重要コンポーネント。

以下のシグナルを利用する。

```text
Foreground Application
Active Window
Running Processes
Audio State
Microphone Usage
Camera Usage
System Audio
Calendar
Keyboard / Mouse Activity
Screen Context
Conversation State
Agent Activity
Application-specific signals
```

Situation例:

```text
SOLO
MEETING
CONVERSATION
WRITING
CODING
FOCUS
MEDIA
UNKNOWN
```

ただし固定enumだけで世界を表現しようとしない。

状態は複数の属性を持てるようにする。

```ts
interface SituationState {
  scene: string;

  confidence: number;

  application?: string;

  userAttention:
    | "available"
    | "busy"
    | "unknown";

  audioEnvironment:
    | "silence"
    | "speech"
    | "multi_speaker"
    | "media";

  updatedAt: number;
}
```

---

## 15. Situation Classification

巨大LLMを24時間呼ばない。

優先順位:

```text
Hard Signal
 ↓
Deterministic Rule
 ↓
Statistical / Small Local Model
 ↓
Local LLM only when needed
```

例:

```text
Video meeting app active       +40
Microphone active              +20
Calendar event                 +15
Multiple speakers              +15
Meeting window foreground      +10
```

閾値以上:

```text
Situation = MEETING
```

状態遷移にはヒステリシスを導入する。

不明な場合:

> **Do nothing**

を基本とする。

---

## 16. Interaction Policy

Situationと「実行してよいこと」を分離する。

例:

```text
MEETING
```

なら、

```text
canSpeak             = false
canInterrupt         = false
canTranscribe        = true
canTranslate         = true
canRetrieveContext   = true
canNotifyVoice       = false
canActExternally     = restricted
```

モデルが発話を望んでも、

```text
canSpeak = false
```

ならTTSを実行してはいけない。

原則:

> **AIが状況を推定し、Policyが境界を強制する。**

---

## 17. Intervention Levels

Assist Plannerの判断を、一つのLevelへ押し込めない。

最低でも、次の3軸を分離する。

## Attention Decision

ユーザーとの関わり方。

```text
IGNORE
OBSERVE
SUGGEST
RESPOND
REQUEST_APPROVAL
```

## Execution Decision

処理をどこで行うか。

```text
NONE
LOCAL
EXTERNAL
DELEGATE
BUILD
```

## Presentation Decision

結果をどう提示するか。

```text
SILENT
INLINE
CHAT
OVERLAY
VOICE
NOTIFICATION
WINDOW
```

例:

```text
Meeting Translation

Attention     = OBSERVE
Execution     = LOCAL
Presentation  = OVERLAY
```

```text
User asks to fix a bug

Attention     = RESPOND
Execution     = DELEGATE to NightWorkers
Presentation  = CHAT + NOTIFICATION
```

判断不能な場合は、`IGNORE / NONE / SILENT`を基本とする。

---

## 18. Example — Meeting

会議開始:

```text
SOLO
 ↓
MEETING
```

Policy:

```text
TTS                OFF
STT                ON
Translation        AUTO
Notes              ON
Context Retrieval  ON
```

英語会議:

```text
System Audio
 ↓
Local Streaming STT
 ↓
Language Detection
 ↓
Local Translation
 ↓
Overlay
```

さらに、

```text
Topic / Entity Detection
 ↓
contextStill
 ↓
Relevant Documents
 ↓
Reference Cards
```

を表示できる。

会議終了後:

```text
Summary
Decisions
Action Items
Follow-ups
```

を生成。

---

## 19. Example — Writing / Email

メール作成中:

```text
Situation = WRITING
Application = Outlook
```

Capability:

```text
Proofread
Rewrite
Tone Adjustment
Fact Check
Reference Retrieval
Previous Context Retrieval
```

AIは原則として音声で割り込まない。

```text
Inline Suggestion
Host UI
Overlay
```

などを利用する。

---

## 20. Example — Coding

Coding Situationを検出しても、本体でCoding Agentを実装しない。

```text
IDE
 ↓
Current Context
 ↓
Ambient Runtime
 ↓
Coding Assistance Required
 ↓
NightWorkers
```

本体の役割:

```text
Current Application Detection
Repository Context Acquisition
User Intent Understanding
Delegation
Progress Presentation
Result Presentation
```

のみ。

---

## 21. Voice Architecture

Alexa型Wake Wordを必須にしない。

音声は常時利用可能なSensorとして扱う。

```text
Microphone
 ↓
Local VAD
 ↓
Local Streaming STT
 ↓
Utterance
 ↓
Situation / Intent
```

RAW AudioをCloudへ常時streamしない。

---

## 22. Local STT

原則ローカル。

Provider abstractionを利用する。

```ts
interface SpeechToTextProvider {
  id: string;
  location: "local" | "cloud";

  available(): Promise<boolean>;

  transcribe(
    audio: AsyncIterable<AudioChunk>
  ): AsyncIterable<TranscriptEvent>;
}
```

候補実装は交換可能にする。

例:

```text
sherpa-onnx
whisper.cpp
faster-whisper compatible runtime
other local STT
```

---

## 23. Local TTS

原則ローカル。

```text
Text
 ↓
Local TTS
 ↓
Audio
```

Provider abstraction:

```ts
interface TextToSpeechProvider {
  id: string;
  location: "local" | "cloud";

  synthesize(
    request: SpeechRequest
  ): AsyncIterable<AudioChunk>;
}
```

Cloud TTSはFallbackのみ。

---

## 24. Audio Lifecycle

常時音声処理は、

```text
fixed ring buffer
bounded queue
backpressure
explicit lifecycle
```

を必須とする。

禁止:

```ts
const chunks = [];

mic.on("data", chunk => {
  chunks.push(chunk);
});
```

RAW Audioは原則保存しない。

```text
RAW AUDIO
  数秒
 ↓
STT
 ↓
TRANSCRIPT
  短命
 ↓
Relevant?
 ├─ NO → DELETE
 └─ YES
      ↓
 Working Context
```

---

## 25. Context Broker

Application固有データをAgentへ直接渡さない。

共通形式へ正規化する。

```ts
interface ContextFrame {
  id: string;
  source: string;

  kind:
    | "conversation"
    | "document"
    | "code"
    | "application"
    | "system";

  application?: string;

  contentRef?: string;
  preview?: string;

  sensitivity:
    | "public"
    | "internal"
    | "personal"
    | "sensitive";

  expiresAt: number;

  availableActions: string[];

  timestamp: number;
}
```

本文の無条件コピーではなく、必要な時だけ解決できる短命な`contentRef`を基本とする。Contextには取得元、機微度、期限を持たせ、保存やCloud送信は別のPolicy判定を通す。

---

## 26. Application Adapters

アプリ統合をAdapter化する。

```text
OutlookAdapter
ZoomAdapter
TeamsAdapter
BrowserAdapter
VSCodeAdapter
TerminalAdapter
GenericDesktopAdapter
```

統合手段はアプリごとに異なってよい。

優先順位:

```text
1. Native Application API
2. MCP
3. Extension / Add-in
4. CLI
5. Playwright / CDP
6. Accessibility API
7. Computer Use
```

Computer UseはUniversal Fallbackにしない。

Native API、MCP、Extension、CLIなどの明示的な統合手段が使えない場合は、原則として`unsupported`またはユーザー確認へ移る。Computer Useは、対象Application、操作範囲、権限、停止条件が明示されたCapabilityとしてのみ利用する。

---

## 27. Presentation Manager

AIは、

> 何を伝えるか

だけでなく、

> どう伝えるか

を判断する。

Channels:

```text
VOICE
HOST_UI
OVERLAY
WINDOW
NOTIFICATION
SILENT
```

例:

```text
Simple question
→ Voice

Meeting translation
→ Overlay

Email typo
→ Host UI

Large document
→ Window

Background completion
→ Notification

Meeting中
→ Silent
```

---

## 28. UI Architecture

基本思想:

> **Voice-first + UI-on-demand**

これは長期方向である。最初のMVPでは、音声処理を確認・停止・修正できることを優先し、Chat UIを常設の操作面として使う。Voice-onlyにはしない。

常設Dashboardを中心にしない。

必要になったときのみSurfaceを出す。

React側は固定画面の集合ではなく、

> **Declarative Surface Renderer**

として扱う。

## First MVP UI

最初のMVPではGenerative Surface全体を先に作らず、ChatとSettingsの2 Surfaceに絞る。

```text
Chat
├─ Typed message
├─ Live / Final transcript
├─ Assistant response
├─ Conversation / Coding task mode
├─ Agent activity and cancellation
├─ Recording / Generating / Speaking status
├─ Cancel / Stop
└─ Minimal conversation history

Settings
├─ LLM Providers
├─ Agent Providers
├─ Models / Endpoints
├─ Task Routing
├─ Primary / Fallback
├─ Connection Test
└─ Effective Route
```

UIとDesign Systemは`../hono-standard`の`variant/rag`を参照し、次を再利用する。

```text
Chat layout and conversation presentation
Message composer patterns
Button / IconButton / Input / Select / TextArea
Color / spacing / radius tokens
Panel / status / loading / error presentation
Responsive layout and accessibility patterns
```

ただし、source codeを無条件にコピーして固定化しない。SAAAのConversation Event、音声状態、Task Route contractへ合わせて、必要なUI部品とtokenだけを移植または抽出する。

次は継承しない。

```text
Hono backend
RAG retrieval implementation
PostgreSQL / pgvector
Markdown source ingestion
RAG-specific authentication and routes
Agentic Search backend
```

`hono-standard`はUI / Design Systemの出発点であり、本プロジェクトのBackend Architectureではない。

RAG variantがPostgreSQL / pgvectorを利用していても、そのstorage構成は持ち込まない。将来Vector Searchが必要になった場合もSAAA所有のSQLite Vectorを使う。

---

## 29. Generative Surface

AgentにReactコードを書かせない。

安全なComponent Catalogを用意する。

```text
Text
Markdown
Transcript
Translation
ReferenceCard
DocumentCard
Code
Diff
Table
Chart
Timeline
TaskList
Progress
Approval
AgentStatus
```

Runtimeは宣言的なSurface Definitionを出す。

```json
{
  "surface": "meeting",
  "children": [
    {
      "component": "Transcript",
      "source": "meeting.transcript"
    },
    {
      "component": "Translation",
      "source": "meeting.translation"
    },
    {
      "component": "ReferenceCards",
      "source": "meeting.references"
    }
  ]
}
```

Reactはこれを描画するだけ。

A2UI / AG-UI的な設計思想を参考にしてよい。

---

## 30. Runtime Communication

REST API中心に設計しない。

中心概念:

```text
Command
Event
State Patch
Stream
UI Intent
```

イベント例:

```text
situation.changed

transcript.delta
transcript.final

translation.delta

context.found

capability.missing
capability.installing
capability.installed

agent.started
agent.completed

surface.create
surface.update
surface.close
```

最初のMVPでは`hono-standard`のHono backendを継承しない。

Tauri UIとRuntimeの通信は、version付きのCommand / Event / Stream contractとして新規に定義する。TransportはTauri IPC、local socket、WebSocketなどへ差し替え可能にし、Core Domainを特定のHTTP frameworkへ依存させない。

---

## 31. Technology Stack

初期案:

```text
Primary Language
  TypeScript

Runtime
  Bun

Desktop Shell
  Tauri 2

UI
  React
  Vite
  Tailwind
  SAAA Design System
    based on hono-standard variant/rag UI tokens and primitives

Native Runtime
  Rust

Schema
  Zod

Local State
  SQLite only

Optional Vector Search
  SQLite Vector
  enabled only when an Embedding model is configured

Browser Automation
  Playwright

Protocols
  MCP
  Event Stream

AI
  Local-first Model / Agent Provider Architecture
  Codex SDK for explicit coding.assist tasks
```

SAAAの永続化はSQLiteへ統一する。PostgreSQL / pgvectorは対象にしない。

SQLiteはSettings、Runtime State、必要最小限のConversation metadataに使用し、独自のLong-term Memory Engineとしては利用しない。

## SQLite and Vector Policy

Embeddingを使わない機能は、Vector extensionなしで成立させる。

Embedding Modelを利用する機能を追加する場合は、同じSQLite ownershipの中でSQLite Vectorを使用する。

```text
Embedding disabled / unavailable
  → Text / Voice Chatは継続
  → Vector Searchを使う機能だけdegradedまたはdisabled

Embedding enabled
  → Embedding dimensionとmodel identityを記録
  → SQLite Vector indexを使用
  → model / dimension変更時は明示的にrebuild
```

禁止:

```text
PostgreSQLをSAAAの必須dependencyにする
pgvectorをSAAAへ導入する
Embedding未設定をChat起動のblockerにする
異なるEmbedding dimensionを同じindexへ混在させる
contextStillのSQLite / Vector tableを直接共有する
```

## LLM / Agent Settings and Task Routing

最初のMVPでは、`../contextStill`のSettings画面とRuntime Settings contractを参照し、SAAA専用のLLM Settingsを実装する。

再利用する考え方:

```text
Provider Definition
├─ enabled
├─ endpoint
├─ model
├─ provider-specific options
└─ connection health

Task Route
├─ task id
├─ primary provider / model
├─ fallback providers
├─ timeout
└─ token / resource limits

Effective Route
└─ 実際に選択されるProvider / Modelを確認できる
```

最初に必要なLLM Task Route:

```text
conversation.respond
```

Codex SDKはChat Completions互換のLLM Providerとして扱わない。Coding Agent固有のthread、streaming event、cancel、sandboxを持つ`AgentProvider`として接続する。

最初のMVPで追加するAgent Task Route:

```text
coding.assist
  provider: codex-sdk
  selection: explicit
  sandbox: read-only
  approval: never
  network: disabled
  web search: disabled
```

`conversation.respond`と`coding.assist`は同じChat Surfaceへ結果を返してよいが、Route contractとProvider capabilityを混同しない。Codex SDK routeはユーザーが明示的に選択したときだけ実行し、MVPではrepositoryの変更、workspace-write、外部Network accessを許可しない。

Codex SDKのthread idはSAAAのSQLiteに保存し、同一Conversationを再開できるようにする。Codexの認証情報そのものはコピーせず、認証状態と参照方法だけを扱う。

Codex SDKはServer-side Node.jsを公式実行環境とするため、Bun互換性を実装開始時に検証する。互換性を確認できない場合は、Codex SDK部分だけを境界の明確なNode.js sidecarとして動かし、SAAA Runtime全体をNodeへ寄せない。

将来は同じRouting contractへ次を追加できる。

```text
situation.classify
conversation.summarize
context.compose
capability.plan
```

STT / TTSはLLM Task Routeと混同せず、それぞれ`SpeechToTextProvider`と`TextToSpeechProvider`として設定する。最初のMVPでは各1つのLocal Providerから開始してよい。

## Settings Ownership

SAAAの設定のSource of TruthはSAAA所有のSQLiteとする。

```text
SAAA Settings UI
 ↓
Validated Settings Command
 ↓
SAAA SQLite
 ↓
Runtime Settings Cache
 ↓
Model / Agent / STT / TTS Router
```

必須条件:

1. UIで保存したProvider、Model、Task Routeが再起動後も残る
2. 保存値ではなくEffective RouteをRuntimeが実際に使用する
3. 設定変更はschema validationを通る
4. API keyなどのsecretは画面やlogへ再表示しない
5. Provider接続テストと失敗理由をSettings UIで確認できる
6. contextStillのSQLite fileやprivate schemaを直接参照しない

contextStill側の設定実装は設計の参照元であり、SAAA Runtime設定の所有者ではない。

---

## 32. TypeScript Responsibility

TypeScript / Bun側:

```text
Situation Manager
Interaction Policy
Assist Planner
Capability Registry
Capability Router
Capability Factory
Model Router
Context Broker
Presentation Manager
Application Adapters
Runtime State
External Project Integration
```

---

## 33. Rust Responsibility

Native I/O中心。

```text
Audio Capture
System Audio
Screen Capture
Accessibility
Window Detection
Process Detection
Mouse
Keyboard
Global Hotkey
Camera
Native OS APIs
```

原則:

> **判断はTypeScript、Native I/OはRust。**

---

## 34. Daemon Architecture

UIとCore Runtimeを分離する。

```text
OS
 ↓
Ambient Agent Daemon
Bun / TypeScript
 ↓
24/7

├─ Tauri UI
├─ Native Runtime
├─ Local Model Runtime
├─ contextStill
├─ NightWorkers
└─ Application Adapters
```

必須:

```text
UI crash
→ Core survives

Model crash
→ Core survives

Native runtime crash
→ Core survives

NightWorkers crash
→ Core survives

contextStill unavailable
→ Core survives
```

Subsystem単位でrestart可能にする。

---

## 35. No Persona Names in Architecture

AI人格名とプロジェクト実装を完全に分離する。

ユーザーは好きな名前を設定できる。

```text
assistant.name = "Cipher"
```

ただし以下へ人格名を使用してはいけない。

```text
daemon name
process name
package name
database name
protocol name
class name
environment variables
IPC identifiers
directory names
```

内部名称は中立的な技術名称にする。

例:

```text
ambient-agent-core
ambient-agent-ui
native-runtime
audio-runtime
capability-runtime
```

正式なプロジェクト名決定後もPersonaとの分離を維持する。

---

## 36. Security & Permission Model

Capabilityには必ず必要権限を宣言させる。

権限を一つのLevelだけで表現しない。最低でも、次の軸を分離する。

```text
Data Sensitivity
  Public
  Internal
  Personal
  Sensitive

Data Destination
  Same Process
  Local Subsystem
  Local External Tool
  Cloud

Action Effect
  Observe
  Local Reversible
  External Reversible
  External Irreversible
  Destructive / Regulated

Attention Cost
  Silent
  Passive
  Interruptive
```

例:

```text
Read current document
→ Observeだが、文書内容に応じてPersonal / Sensitiveになり得る

Create local file
→ Local Reversible

Create email draft
→ External Reversible

Send email
→ External Irreversible

Delete data / payment / credentials
→ Destructive / Regulated
```

自己生成Capabilityにも同じPolicyを適用する。

新しいSkillの初回有効化、Cloud送信、External Action、Sensitive Data accessはユーザー承認を必要とする。

---

## 37. Privacy

Ambient RuntimeであるためPrivacyは最重要。

原則:

1. Raw audioを原則保存しない
2. Screen recordingを原則保存しない
3. 不要Contextは即時破棄
4. Local processingを第一選択
5. Cloud送信はFallback
6. Cloud送信前にContextを最小化
7. Sensitive Applicationを除外可能
8. Cloudを完全無効化可能
9. 現在何を観測しているか確認可能
10. どのProviderで処理したか監査可能

---

## 38. Observability

24/7 daemonでは自己監視を必須とする。

監視対象:

```text
RSS
JS Heap
Native Memory
VRAM
CPU Duty Cycle
Battery / Thermal Pressure
Model Processes
Audio Queue
Event Queue
WebSockets
File Descriptors
Active Sessions
Capability Processes
External Integration Health
Provider / Route / Model used per turn
Transcription / Generation / Speech latency
```

failureを通常状態として扱う。

```text
Local Model OOM
 ↓
Alternate Local Model
 ↓
Cloud fallback if allowed
```

Subsystem leak:

```text
Subsystem restart
```

Core全体の再起動を極力避ける。

---

## 39. Initial Repository Structure

初期案:

```text
apps/
  desktop/
    React + Tauri

  daemon/
    Bun entrypoint

packages/
  core/
  events/
  settings/
  conversation/
  speech/
  model-routing/
  ui-design-system/
  situation/
  policy/
  assist/
  capabilities/
  capability-factory/
  context/
  presentation/
  providers/
  applications/
  protocol/
  security/
  observability/

integrations/
  contextstill/
  nightworkers/

native/
  runtime/
    Rust

skills/
  builtin/

docs/
  architecture/
  concepts/
  adr/
```

Coreから外部製品固有実装へ直接依存させない。

---

## 40. MVP Roadmap

## MVP 0 — Configurable Local Voice Chat

最初のMVPは、Ambient Agent全体ではなく、将来のSituation-aware Runtimeを載せられる最小のConversation Runtimeを作る。

### Scope A — Settings and Routing

`../contextStill`のSettings画面と設定contractを参考に、SAAA専用のSettings UIとSQLite永続化を実装する。

```text
LLM Provider
Model / Endpoint
Agent Provider / Codex SDK status
Local / Cloud classification
Primary Route
Fallback Route
Timeout / Token limits
Provider Connection Test
Effective Route display
```

最初に有効化するTask Routeは`conversation.respond`と`coding.assist`とする。前者は通常のModel Provider、後者はCodex SDK Agent Providerへ接続する。

設定は保存できるだけでは不十分である。Chat Runtimeが実際に保存済みEffective Routeを使用することをMVPの条件とする。

### Scope B — Text and Voice Conversation

文字入力と音声入力を同じConversation Eventへ合流させる。

```text
Typed Text ───────────────────────────────┐
                                         ↓
Microphone                               Conversation
 ↓                                       ↓
Local Audio Capture                      Model Router
 ↓                                       ↓
Local STT                           Assistant Response
 ↓                                  ┌──────┴──────┐
transcript.delta / final            ↓             ↓
 ↓                               Chat UI       Local TTS
 └───────────────────────────────────┘
```

最初のMVPではPush-to-talkまたは明示的な録音開始でよい。常時音声監視、Wake Word、話者分離、会議検出は後続とする。

### Scope C — Simple Chat UI

`../hono-standard`の`variant/rag`から、Chat UIとDesign Systemの必要部分を持ってくる。

```text
Reuse
  Chat layout
  Message presentation
  Composer
  Form controls
  Theme tokens
  Status / loading / error patterns

Do not inherit
  Hono backend
  RAG backend
  PostgreSQL / pgvector
  Source ingestion
  Agentic Search
  RAG-specific authentication
```

Chat UIには最低限、次を表示する。

```text
User text message
Live transcript
Final transcript
Assistant response
Conversation / Coding task mode
Codex agent activity
Recording state
Generating state
Speaking state
Error / Retry
Stop
```

### Runtime Components

```text
SAAA Desktop UI
├─ Chat Surface
└─ Settings Surface

SAAA Runtime
├─ Settings Repository
├─ Runtime Settings Cache
├─ Conversation Service
├─ Model Router
├─ Agent Router
├─ Codex SDK Adapter
├─ Provider Session Repository
├─ STT Provider
├─ TTS Provider
├─ Event Stream
└─ SQLite

Native Runtime
├─ Microphone Capture
└─ Audio Playback
```

### Acceptance

1. LLM Provider、Model、Task RouteをUIで設定し、SQLiteへ保存できる
2. Application再起動後も設定が復元される
3. Provider接続テストの成功・失敗理由をUIで確認できる
4. `conversation.respond`が保存済みのPrimary / Fallback Routeを実際に使用する
5. 文字入力に対するLLM応答がChat UIへ表示される
6. 音声入力の途中結果と確定文字起こしが同じChat UIへ反映される
7. 確定文字起こしからLLM応答を生成し、文字と音声で返答できる
8. Recording、Generation、Speechをユーザーが停止できる
9. Local Route選択時はCloudへ送信せずに基本対話が成立する
10. `hono-standard`のHono / RAG backendへruntime依存しない
11. PostgreSQL / pgvectorを起動・設定しなくても全MVP Acceptanceを満たす
12. Codex SDKの利用可否と認証状態をSettings UIで確認できる
13. ユーザーが`coding.assist`を明示的に選び、read-onlyのCodex threadを開始できる
14. Codex SDKの応答と実行状態がChat UIへstreamされ、ユーザーが停止できる
15. Application再起動後、SQLiteに保存したthread idからCodex conversationを再開できる
16. Codex SDK routeからrepository変更、workspace-write、Network、Web Searchを実行できない

### Explicitly Out of Scope

```text
Automatic Situation Classification
Always-on Listening
Wake Word
Meeting Detection / Translation
Screen / Accessibility Observation
Calendar Integration
Application Automation
Long-term Personal Memory
PostgreSQL / pgvector support
RAG / Embedding ingestion
contextStill Runtime Integration
NightWorkers Integration
Codex SDKによるrepository変更 / workspace-write
Codex SDKへのwrite-capable MCP tools接続
Capability Factory
Computer Use
Generative Surface Catalog全体
```

---

## MVP 1 — Situation Shadow Mode

Foreground Application、Microphone、Audio、CalendarなどのHard SignalからSituation候補を生成する。

この段階では自動介入せず、`would observe / suggest / respond / stay silent`を記録して評価する。

---

## MVP 1.5 — Situation Calibration and Review

MVP 1のShadow Modeから得た状態遷移、根拠、ユーザーfeedbackを使い、SituationとPolicyの品質を校正する。

この段階でも自動実行は解禁しない。表示する場合もユーザーがSituation Surfaceを明示的に開いたreview-onlyな一覧に限定し、通知、TTS、Chatへの割り込み、Provider呼び出しは行わない。

```text
Situation ledger / feedback
        ↓
Replay and quality metrics
        ↓
Rule / threshold candidate
        ↓
User review and correction
        ↓
Versioned shadow policy
        ↓
Next MVPの明示開始Meeting Modeへ
```

詳細な実装順序、評価指標、Privacy境界は[`mvp-1.5-implementation-plan.html`](./.archived/mvp-1.5-implementation-plan.html)に定義する。

---

## MVP 2 — Meeting Mode

明示開始のMeeting Session、Microphone、Local STT、Transcript Surfaceを最初のCoreとして接続する。System Audio、Translation、Floating Overlayはplatform / providerのfeasibility gateを通過した機能だけを追加する。

Meeting中はPolicyによりTTSとInterruptive Notificationを禁止する。

Meeting ModeはMVP 1.5の評価済みPolicyを前提とするが、開始はユーザーの明示操作を必須とする。raw audioはmemory上のbounded bufferだけで扱い、既定ではtranscript / translationもsession終了時に破棄する。保存やCloud routeは個別の明示選択を必要とする。

詳細なSession lifecycle、Audio permission、STT / Translation / Overlayの分割、検証条件は[`mvp-2-implementation-plan.md`](./mvp-2-implementation-plan.md)に定義する。

---

## MVP 3 — contextStill Integration

SituationとTaskから必要なKnowledge Contextを問い合わせ、Relevant ContextをPresentation / Reasoningへ利用する。

保存候補はrule / procedure / lessonへ一般化できるものに限定する。

---

## MVP 4 — Application Adapters

Writing、Browser、VSCodeなど、明示的なApplication Adapterから段階的に追加する。

---

## MVP 5 — NightWorkers Integration

Coding要求をNightWorkersのTask / Run lifecycleへ委譲し、進捗、承認待ち、検証結果、closeoutをPresentation Managerへ反映する。

本プロジェクト内にはCoding EngineやMission Pilot相当機能を実装しない。

---

## MVP 6 — Capability Factory

不足CapabilityをContract化し、NightWorkersで実装・検証し、STAGED Artifactとして受け取る。

ユーザー承認、Runtime Validation、Rollbackを経てACTIVEにすることで、

> **「できない」から「安全に、できるようになる」**

という自己拡張ループを完成させる。

---

## 41. Explicit Non-Goals

初期段階では以下を目標にしない。

```text
AGIそのもの
Cloud-first architecture
24時間Cloud STT
全画面永久保存
全音声永久保存
独自Long-term Memory Engine
独自Coding Agent
Codex SDKを通常LLM Providerとして偽装すること
Codex SDKの認証情報をSQLiteへ複製すること
Codex SDKによる無承認のworkspace変更
NightWorkersの再実装
contextStillの再実装
contextStill private DBの直接共有
hono-standard backendの継承
RAGを最初のChatへ必須化
PostgreSQL / pgvectorの導入
巨大Knowledge Graph
全Application対応
無制限Computer Use
Agent生成Reactコード
無制限自己改変
Coreの自動書き換え
生成Capabilityの無承認ACTIVE化
大規模Multi-Agent Swarm
```

---

## 42. Engineering Principles

## Local-first

Cloudなしでも基本機能を成立させる。

## SQLite-first

SAAAの永続化はSQLiteへ統一する。Vector Searchが必要な場合もSQLite Vectorを使い、PostgreSQL / pgvectorを別経路として追加しない。

## Thin Core

Situation / Policy / Routing / Presentationへ集中する。

## Adapter-first

外部依存を抽象化する。

```text
ModelProvider
STTProvider
TTSProvider
TranslationProvider
ApplicationAdapter
AgentAdapter
ContextProvider
ComputerAdapter
```

## Event-driven

Streaming / long-running処理を前提とする。

## Bounded Everything

Queue、Buffer、Historyに上限を持たせる。

## Explicit Lifecycle

重要Subsystemには、

```text
start()
stop()
dispose()
restart()
health()
```

を持たせる。

## Safe Default

判断不能なら、

```text
IGNORE
```

または、

```text
SILENT
```

を選ぶ。

## Persona-independent

ユーザーが設定した人格名をArchitectureへ漏らさない。

## Self-extension via Plugins

Coreを自己書換えせず、Capabilityを追加する。

## Reuse Existing Projects

Reusable coding knowledgeとContext CompilationはcontextStill。

Software EngineeringはNightWorkers。

同じ問題を本プロジェクト内で再実装しない。

---

## 43. Architectural North Star

このプロジェクトの判断基準を以下とする。

> **A local-first, situation-aware ambient agent runtime that continuously understands the user's working context, decides whether and how to assist, routes work to the appropriate capabilities or external agents, and can acquire missing capabilities as tested, permission-scoped plugins without modifying its core.**

日本語:

> **ユーザーの作業状況を継続的に理解し、介入するべきか、何をするべきか、どの能力へ任せるべきか、どの方法で提示するべきかを判断し、不足する能力についてはCoreを書き換えることなく検証済みPluginとして獲得していく、Local-firstの常駐型AI Runtime。**

この定義に合わない機能は、Coreへ追加する前に本当に本プロジェクトの責務かを再検討すること。

## Current Milestone

現在の判断基準は次とする。

> **MVP 1のSituation StateとShadow Policyを、まずMVP 1.5で評価・校正し、明示開始のMeeting Modeへ接続する。Meeting中はLocal STTをCoreとし、Translation / Floating Overlayはfeasibility確認後に追加する。TTSとinterruptive presentationはPolicyで禁止する。**

MVP 1 Situation Shadow Modeは完了した。実装順序と検証結果は[`mvp-1-implementation-plan.md`](./mvp-1-implementation-plan.md)と[`mvp-1-release-evidence.md`](./mvp-1-release-evidence.md)に保存する。次のMVP 1.5では、ledgerとfeedbackを使った校正・replay・品質基準を固定する。MVP 2ではその後、明示開始のMicrophone Meeting SessionをCoreとして実装し、System Audio、Translation、Floating Overlayは個別のfeasibility gateを通過した場合だけ追加する。
