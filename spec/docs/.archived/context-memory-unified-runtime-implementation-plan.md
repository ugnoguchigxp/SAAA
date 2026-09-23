# ContextWindow / Personal State 共通Runtime統合実装計画

状態: 実装完了。2026-09-17時点。外部LARM実機認定は本計画の対象外として継続する。

実装結果（2026-09-17）: DB v20でgeneration manifest、Scope Registry、Scope解決・epoch、message/run scope、Personal State source scopeを追加した。通常会話はMemory ON/OFFにかかわらず単一Turn Orchestratorを通り、Personal Stateはtyped Context Sourceとして共通Brokerへ入る。OpenAI互換・Dynamic LAN・Agent Session・Reasoning MCPの各Provider requestとTool follow-upは別generationになり、選択・省略source、Tool offer、Policy、Scope、現在入力のdigestを記録する。Context本文や完成Envelopeは保存しない。REDはProvider送信前に停止し、YELLOWは省略理由を残して縮退する。

旧`personal_state::conversation`、専用Tool loop、lineage補助経路は削除した。LARMのSource登録・canonical measurement・one-shot View作成は`personal_state::materializer`へ分離し、モデル推論を開始しない境界にした。Scope/Policy/Personal State assertion/未処理sourceはdispatch時と完了時に再検証する。Context組み立ての`contextInputsLoad`、`contextBrokerCompose`、`contextAssemblyTotal`はcontent-freeなp50/p95 telemetryへ追加した。

コードレビュー追補（2026-09-17）: Personal State jobのScope fenceをglobal input epochから対象Scope epochへ変更し、無関係Scopeの更新後はledger revisionだけを安全にrebaseする。Brokerにも許可Scopeの交差検証を追加した。Agent SessionのTool follow-upは元の会話と構造化Tool結果を含む自己完結Envelopeにし、実際のHTTP bodyで予算・digestを計測する。両Providerの選択source、省略source、静的Tool offer記録は共通実装へ統合した。置換されたcoding workspaceのresource scopeはrevokeし、選択されなかったPersonal State sourceは完了時依存検証の対象外にした。

検証結果: Rust全体554件成功・13件ignore、Frontend 322件成功、Personal State core 17件成功。Clippy `-D warnings`、TypeScript、IPC binding、Spec、module-size gateを通過した。ignore対象の外部サービス・実機canaryは成功件数へ含めない。総合check中に既存Codex app-server fixtureが一度timeoutしたが、単独再実行と続く全Rust再実行では成功し、再現しなかった。

関連文書:

- [Personal AI Concept](saaa-personal-ai-concept.md)
- [Capability / Tool Runtime Concept](saaa-capability-tool-runtime-concept.md)
- [Adaptive Learning and Selective Memory Concept](saaa-adaptive-learning-memory-concept.html)
- [Personal State Architecture Roadmap](personal-state-architecture-roadmap.md)
- [Personal State P1](personal-state-phase-1-plan.md)

## 1. 結論

実装開始時は、ContextWindowとPersonal Stateの個々の安全機構は強い一方、会話Runtimeが二系統あり、4文書で定義した「単一Runtime、毎generationで新しいContextEnvelope、MemoryはContext Source」という構造を満たしていなかった。

最適な改善は、Personal Stateを作り直すことではない。immutable ledger、SourceRef、projection、抽出worker、忘却、LARM Source/View、generation fenceは維持し、Personal Stateが所有している会話生成・Provider呼出し・Tool loopを共通Turn Orchestratorへ戻す。その上で、現行`context_window`を、現在入力・会話履歴・Personal State・Task参照・Tool定義を同じ予算で扱うContext Brokerへ発展させる。

本計画の依存順序は次で固定する。

```text
共通Turn Orchestrator
  -> 明示Scope解決
  -> Context Source候補収集
  -> 共通Context Broker
  -> generation単位のContextEnvelope
  -> 既存Provider routing / fallback / Tool loop
  -> 出力保存とPersonal State非同期抽出
```

Memory ON/OFFはPersonal State sourceを候補に含めるかだけを変える。Runtime所有者、Provider route、Identity、recent history、Tool loop、出力保存は変えない。

## 2. 対象範囲

本計画はロードマップのP1.5を実装し、P1の資産を共通Runtimeへ移す。

含むもの:

- 通常会話の単一Turn Orchestrator
- `user / project / task / resource / request`の明示Scope解決
- generationごとに破棄するContextEnvelope
- 会話履歴とPersonal Stateを一つのContext Brokerで選択する仕組み
- system、現在入力、Tool schema、状態、履歴、LARM Viewを含む共有予算
- Personal StateのContext Source化
- LARM Active ViewをContextEnvelope全体ではなくsource materialization方式として扱う分離
- Scope・Source依存単位の失効
- GREEN / YELLOW / REDのContext Health
- Memory ON/OFF、Provider fallback、Tool往復、再起動を含む統合受入

含まないもの:

- Capability Registryと学習型Tool Routerの本実装（P1.6）
- World State（P2）
- 複数Scopeの自動分類、話題推定、User Core（P3）
- sqlite-vec、embedding、ANN、reranker
- ContextStill、VoiceMem、L-Langの新規統合
- Personal State P1で未完了の外部LARM実機認定を、共通Runtime統合だけで完了扱いすること

P1.6に備え、既存の静的Tool定義もContext Brokerの予算対象にはする。ただし本計画中にRegistryや学習順位付けへ置き換えない。

## 3. 実装開始時の評価

### 3.1 維持する実装

| 領域 | 現在の実装 | 評価と扱い |
| --- | --- | --- |
| Raw会話正本 | `conversation_messages` | 維持。ContextEnvelopeや要約を第二の本文正本にしない |
| ContextWindow安全境界 | `memory/context_window.rs` | 現在入力1回、履歴の非命令化、byte上限、最小再構成の考え方を継承する |
| 入力snapshot | `runtime/conversation_inputs.rs` | Identity、route、security、regional、履歴を同じSQLite read snapshotから読む原則を維持する |
| Provider経路 | `runtime/turns.rs`と`providers/*` | route、fallback、provider session、stream、cancel、output persistenceを共通Runtimeの正本として維持する |
| Tool loop | `providers/chat_completions`と`providers/stream/dispatch.rs` | 提示Tool検証、実行後のretry禁止、call上限、取消を維持する |
| Personal State core | `crates/personal-state-core` | AccessScope、SourceRef、Assertion、Transition、StatePatch、依存・忘却検証を維持する |
| Personal State永続層 | `memory/personal_state/*` | ledger、projection、coverage、worker、outbox、cleanup、restoreを維持する |
| LARM接続 | `personal_state/product.rs`、`inference.rs` | provision、canonical measurement、one-shot View、binding検証、取消確認を維持し、推論呼出しから分離する |

2026-09-17の再確認では、次の既存テストが成功した。

- `memory::context_window::tests`: 18件
- `memory::personal_state::tests`: 16件
- `crates/personal-state-core`: 17件

これは土台の削除理由がないことを示す。一方、共通Runtime統合やScope切替を検証するテストではないため、P1.5完了の証拠にはしない。

### 3.2 解消する構造上の問題

| ギャップ | 実装開始時 | 目標 |
| --- | --- | --- |
| Runtime所有者 | `SAAA_MEMORY_ENABLED=1`で`personal_state::conversation`へ早期return | flagに関係なく共通Turn Orchestratorが所有 |
| Personal State | 独自system prompt、独自Provider request、独自Tool loopを持つ | Context SourceとLARM materializerに限定 |
| Provider route | Memory ON時は通常のroute/fallbackを迂回 | ON/OFFで同一routeとfallback |
| 履歴 | 通常経路はrecent/continuity、Personal State経路は別構成 | 同じBrokerで競合・重複を除いて選択 |
| Tool | 通常経路とPersonal State経路で提示集合が異なる | 同じTool offer snapshotを使用 |
| Context予算 | 固定byte配分で、Tool schemaやLARM materializationと別管理 | Provider契約ごとの共有予算 |
| Scope | 通常履歴に明示Scopeがなく、Personal Stateは`primary`とsource message ID中心 | principalと適用Scopeを分離し、明示Scopeで選択 |
| 失効 | `personal_scope.input_epoch`更新で無関係なgenerationも失効 | 実際に依存したScope・Source・Policyだけで失効 |
| Health | GREEN/YELLOWのみ。重大失敗はreportを作る前にError | REDを永続・通知し、Provider送信を停止 |
| generation | 初回request内でTool往復を継続し、Contextを再計画しない | Provider requestごとに新Envelopeとmanifest |
| 旧Memory | `control_plane` projectionとPersonal Stateが併存 | Personal Stateを意味状態の正本とし、旧projectionをread pathから外す |

特に`runtime/turns.rs`ではMemory ON時に専用会話経路へreturnするため、その後にある通常ContextWindowへのmemory projection記録は本番のMemory ON経路では到達しない。現状を「ContextWindowにPersonal Stateが統合済み」とは評価しない。

## 4. 目標アーキテクチャ

### 4.1 共通Turn Orchestrator

共通Turn Orchestratorは次だけを所有する。

1. 現在入力のdurable保存とrun作成
2. Scope解決と保存
3. 同一read snapshotからの設定・Policy・候補入力取得
4. Provider attemptごとのContext予算決定
5. Context BrokerによるEnvelope作成
6. Provider routing、fallback、Tool loop、取消
7. 出力保存、Runtime状態、監査event

Context SourceはProviderを呼ばず、Toolを実行せず、現在入力を再挿入しない。候補、依存、必要度、render方式、source healthだけを返す。

### 4.2 generationの定義

Providerへ1回requestを送る単位をgenerationとする。

- 初回回答request
- Tool結果を受けた後のrequest
- output開始前のProvider fallback
- reasoning providerが行う追加request

これらはすべて別generationであり、毎回新しいContextEnvelopeとmanifestを作る。Tool transcriptは前generationの結果として次Envelopeへ入るが、現在のユーザー入力は命令位置に一度だけ存在する。

### 4.3 ContextEnvelope

ContextEnvelopeは永続的なMemoryではなく、1 generationだけ有効な投影である。最低限次を持つ。

```text
ContextEnvelope
  generation_id
  scope_snapshot
  trusted_policy_and_identity
  current_instruction            exactly one
  task_and_runtime_snapshot
  selected_context_items[]       untrusted data
  tool_offer_snapshot[]
  provider_materialization       base / LARM View
  budget_report
  health_report
  dependency_manifest
```

Envelope本文自体はDBへ保存しない。再現・失効・監査に必要なdigest、SourceRef、版、Scope、選択・省略理由、予算、healthだけを保存する。

### 4.4 Context Source契約

候補型は少なくとも次を持つ。

| 項目 | 意味 |
| --- | --- |
| `candidate_id` | source内で安定した候補ID |
| `source_kind` | policy、task、recent、personal-state、raw-source、tool-schema等 |
| `scope_refs` | 適用できる明示Scope |
| `requirement` | `must / should / may` |
| `authority` | instruction / trusted-policy / untrusted-data |
| `source_refs` | 正本のID、version、digest、範囲 |
| `render_mode` | base message、LARM View、Tool schema、参照のみ |
| `cost` | canonical token、または保守的byte見積り |
| `utility` | 学習を使わない決定的優先度 |
| `freshness` | 評価時刻、有効期限、未処理訂正の有無 |
| `failure_mode` | 欠落時に省略できるか、generationを止めるか |

P1.5のsourceは次とする。

- `TrustedRuntimeSource`: policy、Identity、regional、presentation mode
- `CurrentInstructionSource`: 現在入力。常に1件の`must`
- `TaskRuntimeSource`: 明示Task、run、委任、取消、成果物参照
- `RecentConversationSource`: Scope一致した最近の対話
- `PersonalStateSource`: 有効なassertion、競合、未反映訂正、必要なRaw SourceRef
- `StaticToolOfferSource`: 現在の静的Policyで提示可能なTool schema

World Stateと学習rerankerはこのinterfaceへ後から追加する。

## 5. Scope契約

### 5.1 P1.5で扱うScope

principalは認可主体であり、作業Scopeの代用にしない。P1.5では次を明示的に扱う。

- `user:<principal>`: 利用者全体へ明示的に共有された条件
- `project:<id>`
- `task:<id>`
- `resource:<id>`
- `request:<input_message_id>`: 今回だけの条件

1 turnには現在requestが必ずあり、加えて最大1本のfocus chainを持つ。例は`user -> project -> task -> request`である。無関係な二つのtaskが同時にfocusとして渡された場合は推測せず`ambiguous`にする。

解決順序は次で固定する。

1. `StartTurnInput.scopeRefs`の明示指定
2. 既存Task・resource台帳に保存済みの明示link
3. 指定なしの通常会話は`user` scope

自然文、embedding、最近使ったTaskからScopeを推定しない。対象が必要なのに不明、または競合する場合はREDとしてProvider送信を止め、確認応答または保留状態を返す。

Scope IDと親子関係は共通Scope Registryで検証する。これはWorld Stateではなく、opaque ID、kind、状態、明示的な親子linkだけを持つ境界台帳である。現行コードには汎用project/task registryがないため、存在しないIDを文字列だけで受理しない。既存のcoding workspace/job等をScopeとして使う場合も、作成時にRegistryへ対応付ける。

### 5.2 過去データ

既存のscope情報を持たない会話messageは`legacy:unscoped`として扱う。局所project/taskのEnvelopeへ自動混入させない。focusが`user`だけの自由会話では、現在と同じrecent history候補として利用できる。

既存Personal State assertionの`task_request=<message_id>`は`request:<message_id>`へ移行する。`task_request=null`は明示sharedのまま維持する。P1.5では既存core fieldを削除せず、canonical scope keyを格納する。複数Scopeを一つのassertionへ直接付ける拡張はP3へ送る。

### 5.3 選択規則

局所focusがある場合、Personal Stateは次だけを候補化する。

- `user` scopeで明示共有された状態
- focusと完全一致する状態
- 保存済み親linkで結ばれたproject/resourceの状態
- 現在requestの状態

単に同じprincipal、同じ会話timeline、時刻が近いという理由では選ばない。

局所focusを持つsourceから抽出した状態は、既定で最も具体的なproject/task/resource/requestへ属する。モデル出力だけで`user`共有へ昇格させない。共有化には、入力と共に渡された明示Scope、利用者確認、または検証可能な既存共有規則のいずれかを要求する。条件を満たさない`task_request=null`候補はactivateせず、candidateとして保留または拒否する。

## 6. Context Broker戦略

### 6.1 予算

Brokerはrequest全体を一つの予算として扱う。system、現在入力、Tool schema、Tool transcript、Memory、会話履歴、LARMの包装、出力予約、安全余白を別会計にしない。

Provider Adapterは`ProviderContextContract`を返す。

- 認定されたcanonical tokenizer / measurementがあるProviderはtoken予算を正本にする
- LARMは既存のcanonical measurementとcapability上限を使う
- 入力上限を認定できないOpenAI互換Providerは、現行96,000 bytesを超えない保守的fallback profileを使い、healthへ測定方式を記録する
- schemaが大きな値を許すことを利用可能容量の証拠にしない

選択順は決定的にする。

1. trusted policy、Identity、現在入力
2. 認可・委任・取消・必須Task状態
3. 明示制約、訂正、競合、未反映入力
4. 現在generationで提示するTool schema
5. focusに一致するrecent dialogue
6. objective、decision、open loop等のPersonal State
7. 明示参照されたRaw source
8. 古いcontinuity詳細

`must`が予算に入らない場合は黙って落とさずREDにする。`should`のsource障害・予算省略はYELLOW、`may`の通常の低順位省略はmanifestへ理由を残しGREENを維持できる。

### 6.2 決定的選択

P1.5では学習を使わない。優先度は必要度、Scope一致、状態kind、明示参照、未反映訂正、鮮度、安定IDの順で決める。同じsnapshotとProvider契約からは同じEnvelopeを得る。

semantic continuityはPersonal Stateの構造化状態が担当する。Raw会話の古い部分をembedding検索して補うことはP1.5の前提にしない。必要性が評価で確認された後に、Context Source内部の候補生成手段としてsqlite-vec等を追加できるが、認可、Scope、must判定、最終予算決定をVector Searchへ委ねない。

### 6.3 重複排除

同じSourceRef範囲をbase messageとLARM Viewへ二重投入しない。Personal Stateの短いstate itemはbase、長い安定sourceはViewを基本とする。pending correctionは必須であり、baseにもViewにも入れられない場合はREDにする。

現行`RECENT_DIALOGUE_HISTORY`と`CONTINUITY_GROUPS`は、`RecentConversationSource`の候補生成ロジックとして移す。固定の「Memory 8KB、recent 32KB、continuity 12KB」配分は廃止し、共有予算内の上限としてのみ扱う。

## 7. Context Health

| 状態 | 条件 | 動作 |
| --- | --- | --- |
| GREEN | invariant成立、全must選択、Scope確定、依存再検証成功 | Providerへ送信 |
| YELLOW | should sourceの一時障害、省略、保守的budget profile、optional materialization縮退 | 同じRuntimeで最小安全Contextを送信。理由をeventとmanifestへ保存 |
| RED | 現在入力が0/複数、Scope競合、must欠落、権限不明、予算超過、source版不一致、Policy/委任失効 | Providerへ送信しない。runを保留または安全な失敗にする |

最終Envelope作成後に現在入力数、命令権限、Scope、総予算、Tool offer、SourceRef版を再検証する。現在のように不正Envelopeを「最小再構成」で隠して送るのではなく、候補選択段階でoptionalを落とし、最終invariant違反はREDとして閉じる。

Context Healthは`RuntimeEvent::Activity`だけでなくgeneration manifestへ保存する。本文、credential、個人情報はhealthやtelemetryへ入れない。

## 8. Personal Stateの統合方法

### 8.1 維持する責務

Personal Stateは次を引き続き所有する。

- immutable assertion / transition ledger
- current projection
- SourceRef、coverage、依存、tombstone
- background extraction worker
- 忘却・remote cleanup・restore
- LARM source registrationとView bindingのAdapter実装

### 8.2 手放す責務

`memory/personal_state/conversation.rs`から次を除去する。

- 会話用system promptの組み立て
- Provider選択とchat request
- 独自Tool定義と最大12回のTool loop
- assistant回答の保存
- 通常会話runの完了処理

移行期間は未使用コードとして残して比較できるが、P1.5完了時にはcallerとmoduleを削除する。障害時にこの専用経路へfallbackしてはならない。

### 8.3 typed PersonalStateSource

`projection::compose`はJSON文字列を直接system messageへ入れる関数から、typed候補を返す関数へ分割する。

- active / candidate / disputedを区別する
- evidence、input dependencies、scope、期限、未処理sourceを保持する
- pending sourceを無条件に全件materializeせず、must候補とSourceRefにする
- instruction authorityは常にnone
- Brokerがrender先と予算を決める

foreground回答をPersonal State専用JSON形式へ強制しない。P1.5ではdurableな状態抽出は既存workerを正本とし、未処理の現在入力を次generationでmust sourceとして扱うことで遅延中の訂正を失わない。`task_bundle::adopt`は共通Response Contractへ移せるまで最適化扱いとし、専用Runtimeを残す理由にしない。

### 8.4 LARM分離

現在の`product::infer` / `inference::infer`は「source materialization」と「model inference」を同時に行う。次へ分割する。

1. `LarmContextMaterializer::prepare`
   - source revalidate
   - provision / register
   - canonical measurement
   - one-shot View作成とbinding検証
   - `view_id`、digest、expiry、依存を返す
2. 共通LARM Provider Adapter
   - 共通Provider attemptとしてrouteから選ばれた時だけchatを実行
   - Orchestrator発行のgeneration / attemptへbind
3. cleanup / cancellation
   - 既存outboxとremote operationを維持

選択ProviderがContext View capabilityを持たない場合、短いPersonal State projectionはbaseへ入れられる。長いoptional sourceは省略してYELLOW、必須sourceを安全に渡せない場合はREDとする。MemoryがProvider routeを勝手にLARMへ変更してはならない。

## 9. Tool loopとの接続

P1.5では既存`available_agent_tools`のPolicyを維持するが、返されたTool定義を`StaticToolOfferSource`としてBrokerへ渡す。Brokerが予算へ計上した同じsnapshotだけをProviderへ提示し、実行時も同じsnapshotで`tool_was_offered`を検証する。

Tool実行後は新generationを開始する。

1. Tool結果を正本または一時transcriptとして記録
2. 委任、取消、SourceRef、Scope epochを再検証
3. call上限に応じてTool候補を再生成
4. 新Envelopeを作成
5. Providerへ再送

これによりP1.6では候補生成元をTool Routerへ置き換えるだけで、Provider clientやContext予算を再設計しなくてよい。Selection Snapshotの永続化はP1.6で追加し、本計画ではgeneration input manifestが静的Tool提示集合を記録する。

## 10. 永続化とIPC変更

計画作成時のDB versionは18であり、Phase 1でadditiveなversion 19を追加した。以後も実装開始時のHEADを再確認し、次の空きversionを使う。過去migrationは編集しない。

### 10.1 追加table

| table | 用途 |
| --- | --- |
| `context_scopes` | user/project/task/resource/requestのopaque identityと有効状態。意味状態は持たない |
| `context_scope_links` | 明示的な親子・所有関係。自動推定した関係は保存しない |
| `runtime_scope_resolutions` | runごとのresolved / ambiguous / missing、focus、scope digest |
| `runtime_run_scopes` | runに明示されたuser/project/task/resource/requestとrelation、source |
| `conversation_message_scopes` | messageに確定したScope。過去会話選択と再起動後の再現に使う |
| `context_scope_epochs` | Scopeごとの入力変更sequence。無関係なScopeを失効させない |
| `context_generations` | generation、run、ordinal、provider attempt、Envelope digest、health、status |
| `context_generation_inputs` | 選択・省略したsource/tool、版、digest、必要度、配置、理由 |

`context_generations`にRaw本文や完成Envelopeを保存しない。`context_generation_inputs`はSourceRefと非機密metadataだけを持つ。

### 10.2 Personal Stateのadditive変更

- `personal_sources`と確定Scopeを対応付ける`personal_source_scope_refs`を追加
- `personal_jobs`に対象scope keyとclaim時scope epochを保持
- `personal_generations`から共通`context_generation_id`を参照可能にする
- generation validityをglobal input epoch一致だけでなく、実際のsource版、scope epoch、policy revision、tombstone、委任で判定する
- global `input_epoch`は監査用の単調増加値として残せるが、単独で全runを失効させるfenceにはしない

message保存triggerだけでScopeが分からないため、入力message、run、Scope、scope epoch、Personal State source/jobの作成を`prepare_runtime_run`の同一transactionへまとめる。既存triggerは移行後に二重enqueueしない形へ置き換える。

### 10.3 IPC

`StartTurnInput`へdefault空の`scopeRefs`を追加する。

```json
{
  "scopeRefs": [
    { "kind": "project", "id": "project-saaa", "relation": "parent" },
    { "kind": "task", "id": "task-p1-5", "relation": "focus" }
  ]
}
```

IDは既存台帳で実在と関係を検証する。自由な自然文labelをScope IDとして採用しない。既存clientは空配列で互換にし、`user` focusへ解決する。

## 11. コード変更単位

### 新設

- `runtime/context/mod.rs`: 共通Runtime境界
- `runtime/context/scope.rs`: Scope Registry、解決、検証、epoch snapshot
- `runtime/context/source.rs`: typed Context候補
- `runtime/context/broker.rs`: 決定的選択、重複排除、共有予算、最終render
- `runtime/context/generation.rs`: generation metadata保存と依存再検証
- `runtime/context/schema.rs`: DB v19/v20のadditive schema
- `runtime/context/health.rs`: GREEN / YELLOW / RED
- `memory/personal_state/materializer.rs`: LARM Source/View準備。推論は開始しない
- `providers/chat_completions/generation.rs`: request/Tool follow-up manifest
- `providers/agent_session/sse/generation.rs`: request/Tool follow-up manifest

### 主な変更

- `runtime/turns.rs`: Memory分岐を削除し、共通Orchestratorを呼ぶ
- `runtime/conversation_inputs.rs`: Scope snapshotとsource候補metadataを同じread snapshotで取得
- `runtime/conversation_context.rs`: Broker済みmessageをProvider historyへrender
- `memory/context_window.rs`: recent/continuity候補生成へ縮小し、最終予算所有をBrokerへ移す
- `providers/chat_completions/mod.rs`: requestごとにEnvelopeを取得し、Tool提示snapshotを外から受け取る
- `providers/chat_completions/mod.rs`: 提示済みTool snapshotを実行時にも使用し、再計算で置換しない
- `memory/personal_state/projection.rs`: typed ContextCandidateを返す
- `memory/personal_state/product.rs`: materializer結果をbindしてからchatを実行
- `memory/personal_state/generation.rs`: 共通generationとdependency manifestへbind
- `memory/personal_state/worker.rs`: message IDではなくresolved scope keyでrequest-local状態を扱う
- `memory/personal_state/schema.sql`: additive scope / dependency migration
- `memory/control_plane/*`: 旧projectionの本番read pathを停止。tableはrollbackと移行確認のため当面削除しない

## 12. 実装フェーズ

2026-09-17のコード切替ではPhase 1〜6を一つのmigration seriesとして完了した。Phase 0の比較fixtureは既存Provider fixture、Personal State 64シナリオ、会話品質60シナリオ、Scope/Broker/generationの追加回帰テストを正本とする。ネットワークを含む外部LARM実機認定は対象範囲の定義どおり別gateである。

### Phase 0 — Baselineと契約固定

- 4 Concept文書と本計画を受入の正本にする
- 現行通常会話、Memory ON、Provider fallback、Tool callのrequest構造をfixture化する
- A/B Scope統合シナリオとsource ablation corpusを先に作る
- Context assemblyの現行p50/p95、request bytes、first requestまでの時間を記録する
- P1未完了の外部LARM項目をP1.5と混同しないdependency表にする

完了条件: 比較可能なfixture、corpus、性能baselineが結果を見る前に固定されている。

### Phase 1 — 共通Turn Orchestratorとgeneration ledger

実装済み。DB v19のledgerをv20のScope/source manifestへ拡張し、全会話Provider経路へ接続した。

- 共通generation型、manifest、health型を追加
- 現行ContextWindowを一度adapterで包み、Memory OFFの動作を変えず共通Orchestratorから呼ぶ
- Provider attemptとTool follow-upごとにgeneration IDを発行
- 現在入力1回、Provider fallback前再検証、Tool提示snapshot固定のテストを追加

完了条件: Memory OFFで既存route、fallback、Tool、stream、保存が回帰せず、全Provider requestにmanifestがある。

### Phase 2 — 明示Scope解決

実装済み。未登録・競合Scopeは推定せずREDになり、coding workspace/jobはresource/task ScopeとしてRegistryへ登録される。

- IPC、Scope table、message scope、scope epochを追加
- `user`既定、project/task/resourceの明示link、ambiguous holdを実装
- recent historyをScopeでfilter
- legacy unscopedの保守的規則を適用

完了条件: project Aの局所履歴がBへ入らず、Scope不明・競合時にProvider requestが0件である。

### Phase 3 — 共通Context Broker

実装済み。96,000 bytesの保守的Provider契約を最終requestにも適用し、must超過はRED、should省略はYELLOW、may省略はmanifest記録とした。

- Context Source契約とBrokerを実装
- policy/current/task/recent/static toolsをsource化
- ProviderContextContract、共有予算、重複排除、health REDを実装
- 旧`context_window::compose`を互換shimにし、shadow比較後に最終所有権をBrokerへ移す

完了条件: 全requestが予算内、現在入力が1件、Tool schemaを含む総入力が計測され、source単位のablationができる。

### Phase 4 — Personal StateのContext Source化

実装済み。Memory flagが変えるのはPersonal State候補の有無だけであり、route、Identity、recent history、Tool loop、保存経路は共通である。

- typed PersonalStateSourceを実装
- assertion、pending correction、Raw SourceRefをBroker候補へ変換
- Memory OFFは候補なし、ONは候補ありとし、Orchestratorとrouteを共通化
- foreground専用JSON回答への依存を外し、workerとpending sourceで整合性を維持
- 旧control-plane projectionを本番選択から外す

完了条件: ON/OFFでRuntime owner、Provider route、Identity、recent history、Tool loopが同一で、Personal State sourceだけがablationされる。

### Phase 5 — LARM materializer分離と依存単位失効

実装済み。LARM materializerは推論を開始せず、View準備結果だけを返す。Scope、Policy、assertion transition、pending source availabilityをgeneration fenceで再検証する。

- provision / View作成をProvider inferenceから分離
- 共通generationとLARM attempt / Viewをbind
- Scope epoch、SourceRef、Policy、委任のdependency vectorを実装
- 無関係なScope更新では継続、関連source編集・忘却・委任撤回ではdispatch / save / publishを拒否
- Tool往復ごとに新EnvelopeとViewを作る

完了条件: Bの新入力でAのrunが無効化されず、Aの訂正・削除・撤回では遅延結果と新規副作用が止まる。

### Phase 6 — 切替、削除、受入

実装済み。専用会話moduleとfallbackを削除し、additive schemaと旧tableをrollback資産として残した。

- shadow差分と性能を評価
- unified runtimeを既定化
- `personal_state::conversation`のcallerとmoduleを削除
- 旧Runtimeへのfallbackを削除
- 回帰、再起動、snapshot miss、release変更、forget race、A/Bシナリオを実行
- P1受入資産を共通Runtimeで再実行

完了条件: 13章のgateをすべて満たし、専用会話経路が存在しない。

## 13. 受入Gate

### 13.1 整合性・安全性（許容0件）

- Provider requestごとの現在入力が命令位置にちょうど1件
- Raw history、Memory、Tool結果が命令権限を持たない
- principal / Scope / purpose / classification漏洩
- 未提示Tool、版不一致Tool、撤回後の新規dispatch
- forget後の本文保存・再投影・snapshot再利用
- must sourceの無通知省略
- REDでのProvider送信
- baseとViewへの同一source範囲の重複

### 13.2 共通Runtime

- Memory ON/OFFで`execute_conversation_turn`以下の所有経路が同じ
- Provider route、fallback順、Identity、regional、presentation、recent history、Tool loopが同じ
- Personal Stateと各Context Sourceを個別にablationできる
- optional source障害は同じRuntimeでYELLOW縮退
- mandatory source、権限、Scope不足はRED停止

### 13.3 Scope / invalidation

- A開始 → B切替 → A完了通知 → A委任撤回 → 再起動 → A再開
- Aの局所条件がBへ0件
- Aの完了通知をBのTask結果として扱わない
- 再起動後の重複副作用0件
- 撤回後の新規実行0件
- Bの無関係入力によるAのgeneration失効0件
- Aの関連source編集・忘却による旧generation許可0件

### 13.4 品質

- 明示制約、訂正、撤回、未決、open loopのmust保持率100%
- 固定日本語corpusで局所条件のuser-wide昇格0件
- current stateが未処理訂正より古い場合に確定値として回答0件
- baselineより不要な再説明・確認が悪化しない

### 13.5 性能・資源

Phase 0で測定環境を固定し、少なくとも次を継続測定する。

- Scope解決、source収集、Broker、renderのp50/p95
- Provider送信直前のcanonical tokensまたは保守的bytes
- first requestまでの追加時間
- Tool follow-upごとの再構成時間
- LARM measurement / View作成 / cleanup
- SQLite read/write時間とtransaction保持時間

暫定gateは、ネットワークを除くローカルContext組み立てp95 50ms以下、通常Memory OFFのfirst request準備時間をbaseline比10%以上悪化させない、全処理でDB transactionをnetwork await越しに保持しない、とする。Phase 0の実測で非現実的と判明した場合は、実装結果を見る前に理由と新閾値を本書へ記録する。

## 14. Rolloutとrollback

実装は二重Runtimeを残すfeature flag方式ではなく、回帰fixtureを通した単一Runtimeへの直接切替を採用した。`SAAA_CONTEXT_BROKER_V2`と`SAAA_UNIFIED_CONTEXT_RUNTIME`は導入していない。切替後も`SAAA_MEMORY_ENABLED`が制御するのはPersonal State sourceの有無だけであり、旧専用会話Runtimeへ戻る経路はない。

schemaはadditiveにし、旧tableを直ちに削除しない。rollbackは以前のapp binaryと旧read pathで可能にするが、新旧両Runtimeを本番fallbackとして同時稼働させない。切替後の障害は共通Runtime内でMemory sourceを無効化して縮退し、専用Personal State会話Runtimeへ戻さない。

実施した削除順は次のとおり。

1. unified runtime既定化
2. ローカル受入corpusとProvider fixture通過
3. `personal_state::conversation`削除
4. 旧control-plane projection read削除
5. 2 release以上のrollback期間後に旧table削除を別計画で判断（未実施）

外部LARM実機canaryはPersonal State P1の未完了項目として残り、本計画のコード完成や旧会話Runtimeの復活条件にはしない。

## 15. 実装開始判断

最初に着手する変更はPersonal State内部の大改造ではなく、Phase 1の共通generation ledgerとTurn Orchestrator境界である。これがない状態でScope、Vector Search、Tool Router、World Stateを追加すると、別々のRuntime所有者と予算管理が増え、Conceptとのギャップが広がる。

本計画の終了時点では、SAAAはSessionを保存単位にせず、Raw Eventと意味状態を継続保持しながら、推論Contextはgenerationごとに作って捨てる。Tool、Memory、会話履歴、将来のWorld Stateは同じScope・Policy・予算・失効契約に従う。この共通境界をP1.6以降の前提とする。
