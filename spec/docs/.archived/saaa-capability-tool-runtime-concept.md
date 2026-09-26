# SAAA Capability / Tool Runtime Concept

## 必要な能力だけを見つけ、安全に実行し、結果から選択を改善する

作成日: 2026-09-16
状態: コンセプト案 v0.3 / 統合契約改訂
改訂日: 2026-09-17

## 1. この文書の位置付け

この文書は、SAAAが利用できるCapability、Skill、Toolを登録し、ユーザーの要求に合う能力だけを選択してLLMへ提示し、安全に実行するための基本構想を定義する。

対象は、SAAA組込みの機能だけではない。L-Langで追加される小さな機能、外部サービスAdapter、ローカルScript、将来のComposite Skillも、同じ発見・選択・実行・評価の流れへ載せる。

上位方針は[SAAA Personal AI Concept](saaa-personal-ai-concept.md)と[Project Concept & Direction](plan.html)に従う。この文書は、そこに記載されているCapability Model、Capability Registry、Capability Router、Capability Factoryを、Tool Runtimeの観点から具体化する。

Capability Routerは独立した会話Runtimeではない。[Adaptive Learning and Selective Memory Concept](saaa-adaptive-learning-memory-concept.html)のMemory Policyと同様に、共通Context Brokerへ候補と参照を渡す選択機構である。Memoryの有効・無効、Toolの有無、Providerの違いによって、Turnの保存、Scope解決、ContextEnvelope、Tool loop、応答確定の所有者を切り替えない。

本文の「SAAAは〜する」は目標とする設計を表し、すべてが実装済みという意味ではない。設計原則、初期採用案、将来の改善候補を区別して記載する。最終的なSQLite schema、IPC、公開API、画面仕様は、実装計画で別に確定する。

## 2. 背景と解決したい問題

何でも扱えるAgentを目指すほど、利用可能なToolは増える。すべてのTool名、説明、引数schema、利用条件を毎回LLMへ渡すと、次の問題が生じる。

- Tool説明がContextを消費し、ユーザーの要求や作業状態へ割ける量が減る。
- 似たToolが増えるほど、LLMが名前や説明のわずかな差から誤選択しやすくなる。
- Tool追加のたびに固定promptやRuntimeの分岐を増やす必要がある。
- 利用されなかったToolも毎回推論対象となり、遅延と費用が増える。
- ユーザーが繰り返し訂正しても、次回の選択に反映されない。

SAAAは、Toolの保管とToolの提示を分離する。利用可能な能力はSQLiteへ登録し、各要求に対して必要な候補だけを検索する。LLMへ渡すのは、選択された少数のTool定義だけとする。

```text
すべてのToolを毎回LLMへ渡す
                ↓
SQLiteに全Toolを登録する
                ↓
要求ごとにCapabilityを検索する
                ↓
適格なSkill / Toolへ絞り込む
                ↓
少数のTool定義だけをLLMへ渡す
```

## 3. 用語と責務

### Capability

ユーザーに提供できる論理的な能力。「何ができるか」を表す。

例:

```text
mail.rewrite
meeting.translate
browser.research
desktop.capture_screen
code.implement
```

Capabilityは特定の実行方式に依存しない。検索、計画、Capability不足の判定は、原則としてこの単位で行う。

### Skill

Capabilityを実現する具体的な方法。「どう実現するか」を表す。一つのCapabilityに複数のSkillを関連付けられる。

```text
mail.rewrite
  ├─ local-llm-rewrite
  ├─ cloud-rewrite
  └─ llang-mail-rewriter
```

Skillには、実行方式、必要資源、対応環境、費用、レイテンシ、実績、healthを持たせる。

### Tool

LLMまたはRuntimeからSkillを呼び出すための型付きインターフェース。Tool名、説明、入力schema、出力契約、実行制約を持つ。

一つのSkillが複数のToolを持つ場合がある。例えば長時間のCoding Skillは、開始、状態確認、追加依頼、取消を別Toolとして公開できる。ただし、すべてを常時提示する必要はない。

### Artifact

Skillを実行する配布物。L-Lang package、Wasm、Script、Native実装、Application Adapterなどを含む。Artifactは検証対象であり、Capabilityそのものではない。

### Capability Registry

Capability、Skill、Tool、Artifact、状態、版、利用条件、検索情報を保持する。永続状態の正本はSQLiteとする。

### Capability Router

現在の要求と状況に対して、利用候補となるCapabilityを発見し、適格なSkillとToolを選ぶ。権限を付与する機構ではない。

### Selection Snapshot

一回のgenerationに対し、実際に提示するTool、Skill版、Artifact hash、選択理由、Router版を固定した記録。Brokerの予算調整とRuntimeの適格性検査後に共通Runtimeが確定し、Tool Callと実行をこの記録へ結び付ける。選択後のCatalog更新によって、実行対象が暗黙に差し替わることを防ぐ。

## 4. 設計原則

### 4.1 SQLiteを正本にする

Capability、Skill、Tool、版、状態、利用実績、ユーザーフィードバックはSQLiteへ保存する。フロントエンドやLLM Contextに独立した正本を作らない。

検索用Embeddingとインメモリ索引は再構築できる派生物とする。キャッシュを失っても、SQLiteに残るManifestと版情報から復元できなければならない。

### 4.2 発見と実行を分ける

意味的に近いことと、実行してよいことは別である。

Capability Routerは候補を発見する。Policy、Delegation、Permission、Risk Gateは実行可否を決める。検索スコア、利用回数、学習結果によって権限を拡大しない。

### 4.3 LLMへ渡すToolを絞る

LLMには、要求に関係するToolだけをContext予算内で渡す。候補数だけでなく、Tool説明とschemaの合計byte数またはtoken数にも上限を置く。

### 4.4 Tool成功と目的達成を分ける

Toolが正常終了しても、ユーザーの要求を満たしたとは限らない。Tool Runtimeは実行結果を記録し、Task Runtimeまたは会話Runtimeは目的への適合を評価する。

### 4.5 新しい能力は明示的なLifecycleを通す

生成・取得したArtifactを、検索で見つかったという理由だけで実行可能にしない。インストール、検証、承認、有効化を分離する。

### 4.6 頻度だけで最適化しない

よく使われるToolが、すべての要求で適切とは限らない。利用頻度は、意味的に適合した候補間の小さな優先度として用いる。選択適合度、実行信頼性、ユーザー選好を分離する。

### 4.7 Local-firstを維持する

Registry、検索、利用実績、フィードバックは端末内で完結できることを基本とする。外部EmbeddingやCloud Skillを使う場合は、送信範囲と許可を別に検証する。

### 4.8 SessionlessだがScopedな共通Runtimeへ接続する

Capability選択は会話Sessionへ紐付けない。現在の`project`、`task`、`resource`、`request`などのScopeと、共通Turn Orchestratorが所有する現在入力を使う。Scopeは権限そのものではなく、候補の適用範囲と誤混入防止に使う。

Capability Routerが返すのは候補、順位、除外理由、固定したSkill版とArtifact hashである。候補の記録と、最終提示集合のSelection Snapshotを区別する。Provider requestを直接組み立てず、別のTool専用会話Runtimeも持たない。Context BrokerがMemory、World State、Task状態と同じ予算の中でTool定義を選び、共通Runtimeが実行前検査とTool loopを所有する。

## 5. 全体構成

```text
Current Instruction
  + Scope Refs
  + Active Goal / Task
  + Situation
  + Available Inputs
  + Delegation / Policy
            ↓
      Routing Request
            ↓
   Deterministic Eligibility Filter
   - ACTIVEか
   - 権限と委任の範囲内か
   - 必要な入力があるか
   - 対応OS / Runtimeか
   - healthと期限を満たすか
            ↓
      Capability Retrieval
   - SQLite FTS5 / BM25
   - Embedding / cosine similarity
            ↓
          Fusion
   - exact match
   - lexical rank
   - semantic rank
            ↓
       Skill Re-ranking
   - user preference
   - verified reliability
   - latency / cost / locality
   - current resource availability
            ↓
      Bounded Candidate Plan
            ↓
 Capability Plan + version refs
            ↓
       Common Context Broker
   - Memory / World / Taskと予算調整
   - Toolと必須情報を共通予算へ収容
            ↓
       Common Turn Runtime
   - 適格性検査後にSelection Snapshotを確定
            ↓
      Tool Call Validation
            ↓
        Skill Execution
            ↓
 Raw Event / Outcome / Feedback / Verification
            ↓
 Reducer / Learning Ledger
            ↓
     次回の選択へ反映
```

## 6. Registryが保持する情報

以下は論理的なデータ群であり、最終的なテーブル名ではない。

| データ群 | 主な内容 | 正本か |
| --- | --- | --- |
| Capability Catalog | ID、説明、用途、入出力、正例、負例、分類 | 正本 |
| Skill Catalog | Capabilityとの対応、executor、費用、実行条件 | 正本 |
| Tool Definitions | Tool名、入力schema、出力契約、Context用説明 | 正本 |
| Artifact Versions | package hash、版、provenance、検証結果 | 正本 |
| Lifecycle State | discovered / staged / active / suspended / retired | 正本 |
| Search Documents | 検索用に正規化した説明とタグ | 派生可能 |
| Embeddings | model ID、次元、content hash、vector | 派生可能 |
| Selection Events | 候補、順位、提示、選択、Router版 | 観測記録 |
| Execution Outcomes | 成否、失敗分類、レイテンシ、検証結果 | 観測記録 |
| User Feedback | 適合、不適合、訂正先、不明 | ユーザー記録 |
| Aggregate Statistics | 時間減衰した適合度、信頼性、選好 | 派生可能 |

Capabilityの検索文書には、少なくとも次を含める。

- 人が読める短い説明
- `use_when`: 適する要求と状況
- `avoid_when`: 適さない要求と状況
- 必要な入力と生成する出力
- 正例となるユーザー要求
- 負例となるユーザー要求
- 対応するApplication、言語、データ種別
- Capability分類とタグ

JSON Schema全文をEmbedding用文章としてそのまま使わない。引数名や構造上の語が意味検索へ不要な影響を与えるため、検索文書と実行schemaを分離する。

`avoid_when`と負例は、正例と同じEmbeddingへ連結しない。負例に近い要求を減点できるよう、別の特徴または別Embeddingとして扱う。

認証情報、短時間credential、秘密値はRegistry、検索文書、Embedding、選択履歴へ保存しない。

## 7. CapabilityとArtifactのLifecycle

初期Lifecycleは次を基本とする。

```text
DISCOVERED
    ↓ inspect
STAGED
    ↓ verify / acceptance tests
VALIDATED
    ↓ user or trusted policy activation
ACTIVE
    ↓ suspend / health failure / policy change
SUSPENDED
    ↓ retire
RETIRED
```

初期実装で状態を簡略化する場合も、次の区別は失わない。

- 登録されているが実行できない。
- 検証済みだが、まだユーザーへ公開しない。
- 現在の選択候補として利用できる。
- 一時的に選択から外れている。
- 過去の実行証跡のため版情報だけ残している。

L-Lang Artifactは、[L-Lang Wasm実行キット接続PoC](verification/llang-wasm-host-poc-20260916.md)で確認したinspect、verify、invokeの契約を利用候補とする。ただし、PoC合格を通常会話への公開承認とみなさない。

```text
L-Lang Build
  ↓
Artifact + Manifest + packageHash
  ↓
SAAA inspect
  ↓
SAAA verify / acceptance gate
  ↓
RegistryへSTAGED登録
  ↓
明示的なACTIVE化
  ↓
検索文書とEmbedding生成
  ↓
Capability Routerの候補になる
```

候補生成時にSkill版とArtifact hashを固定し、提示直前に有効性を検査してSelection Snapshotへ記録する。呼び出しまでの間に新しい版がACTIVEになっても、同じ実行の途中で暗黙に切り替えない。固定版が利用不能になった場合は、別版へ黙ってfallbackせず、再選択または安全な失敗とする。

## 8. Routing Request

Capability Routerへ渡す入力は、ユーザー発話だけに限定しない。ただし、会話履歴全体を無条件に結合もしない。

Routing Requestは、今回の選択に必要な情報を小さく構成する。

- 最新のユーザー要求
- 現在のGoalまたはTask
- 操作対象となるApplication、File、Conversation、Device
- 利用可能な入力型
- 求める出力型
- 現在のSituation
- Local / Cloud利用条件
- 委任、権限、リスク上限
- 継続中のTool jobまたはProvider execution handle

権限や委任は、Embeddingへ混ぜて類似度で判断しない。構造化された条件として、候補検索の前後と実行直前に検査する。

Routing RequestはContextEnvelopeそのものではない。本文を複製せず、現在入力への参照、必要なbounded feature、Scope、Goal / Task参照を基本とする。RouterはMemory本文やWorld State全体を読むのではなく、候補選択に必要なmetadataだけを受け取る。

Toolの実行結果を受けた後は、最初の要求だけでなく、未解決の目的と現在の実行状態からRouting Requestを更新する。複数手順の仕事で、最初にすべてのToolを提示し続ける必要はない。

## 9. Capability選択

### 9.1 決定的な適格性フィルター

意味検索より先に、実行候補になり得ないSkillを除外する。

- Lifecycleが`ACTIVE`である。
- 現在のOS、Application、Runtimeで利用できる。
- 必要な入力と依存資源がそろっている。
- healthが許容状態である。
- ユーザーの委任、Privacy、送信条件を満たす。
- リスク上限を超えない。
- 現在のTask状態で呼べるToolである。

利用頻度、Embedding類似度、モデルの希望によって、このフィルターを迂回しない。

### 9.2 Hybrid Retrieval

初期の検索は、SQLite FTS5とEmbedding検索を併用する。

FTS5は、Capability ID、Tool名、Application名、固有語、略語、エラーコードなど、文字として一致する要求に使う。SAAAの会話検索ですでに採用しているtrigram tokenizerとBM25の実装方式を再利用候補とする。

Embedding検索は、異なる言い回しから近いCapabilityを探すために使う。例えば「メールを少し柔らかくして」と`mail.rewrite`を結び付ける。

両者の候補集合を統合し、Reciprocal Rank Fusionなど、異なるスコア尺度を直接比較しなくてよい方式で初期順位を作る。完全一致や構造化タグ一致は、独立した強い特徴として扱う。

### 9.3 初期のVector Search方針

想定するCatalogは最大でも数千件である。初期はSQLite用Vector Search拡張を導入しない。

- EmbeddingはSQLiteへfloat32 BLOBとして保存する。
- ACTIVEなEmbeddingをRustのインメモリ派生キャッシュへ読む。
- L2正規化済みEmbeddingのdot productでcosine類似度を求める。
- Catalog revisionが変わったときだけキャッシュを更新する。
- FTS5検索結果と統合する。

数千件の総当たりは小さく、ANN indexの訓練、再構築、配布、互換性、SQLite拡張の安全な組込みを増やす合理性がまだない。件数だけでなく、実測したp95遅延、メモリ量、更新頻度が許容値を超えた場合に、SQLite Vec1などの固定・検証済み拡張を再評価する。

### 9.4 Skillの再順位付け

Capabilityが見つかった後、そのCapabilityを実現するSkillを選ぶ。考慮する特徴は次の通り。

- Local / Cloud設定
- 入力と出力の互換性
- Application固有の精度
- 検証済みの実行信頼性
- ユーザーの過去の選好
- レイテンシ、費用、資源使用量
- healthと現在の混雑
- 版の安定性

検索順位が高いことだけで、最終Skillを決めない。意味的な適合はCapability発見に使い、Skill選択では実行条件と実績を重ねる。

### 9.5 Context予算への収容

最終的に提示するToolは、上位N件という件数制限と、schemaを含む合計Context予算の両方で制限する。初期値は実測で決めるが、概念上は4〜8件程度の少数を想定する。

似たSkillが複数ある場合、同じCapabilityの実装だけで提示枠を埋めない。原則としてCapability単位で多様性を確保し、必要ならRouterが代表Skillを一つ選ぶ。

長時間jobの状態確認、継続、取消などは、対象jobが存在するときだけ提示する。開始前からLifecycle Tool一式を常時渡さない。

各候補は、必要な対象参照、制約、Procedure prerequisiteと推定Context costをBrokerへ返す。Brokerは必須情報との依存関係を満たす集合を選び、収容できなければ再計画または保留する。Router内の候補数上限は検索コストの制限であり、最終提示予算の決定ではない。

### 9.6 該当なしを返せる

スコアが低い、必要な入力がない、安全なSkillがない、候補が競合している場合は、無理にToolを選ばない。

結果は少なくとも次を区別する。

```text
MATCHED             適格な候補がある
AMBIGUOUS           候補を決めるための確認が必要
UNAVAILABLE         Capabilityはあるが現在利用できない
CAPABILITY_MISSING  必要なCapabilityが登録されていない
NO_TOOL_NEEDED      Toolを使わず応答できる
```

適格な候補が0件でもCatalogに能力がないとは限らない。実行不可のCatalog metadataと除外理由を使ってUNAVAILABLEを区別し、検索が不完全な場合は不足を断定せず追加検索または確認へ進む。実行不可のTool定義をProviderへ公開する必要はない。

`CAPABILITY_MISSING`はCapability Factoryへの入力候補にできるが、自動的な生成・インストール・ACTIVE化を意味しない。

## 10. 利用実績とフィードバック

### 10.1 三つの評価を分離する

一つの総合成功回数だけでは、改善理由を説明できない。次を別に扱う。

| 評価 | 問い | 主な根拠 |
| --- | --- | --- |
| Selection Fit | この要求にこのCapability / Toolで合っていたか | ユーザー評価、訂正、Task評価 |
| Execution Reliability | Toolは契約通り正常に動いたか | exit、schema、timeout、検証 |
| User Preference | 複数の適切なSkillのうち何を好むか | 明示選択、採用履歴、設定 |

Toolが正常終了しただけではSelection Fitを正としない。逆に、適切なToolが一時的な障害で失敗した場合、意味的な適合まで負にしない。

### 10.2 記録するイベント

選ばれたToolだけでなく、比較対象となった候補も記録する。

- Routing Requestのdigestと参照
- Router、Embedding、Catalogの版
- 適格性フィルターを通過した候補
- lexical rankとsemantic rank
- 再順位付けに使った特徴
- LLMへ提示したTool
- LLMが選択したTool
- 実際に実行したSkillと版
- 実行結果と検証結果
- ユーザーの評価と訂正先

候補提示の記録がなければ、「利用されなかった」のか「候補にすら出なかった」のかを区別できない。学習と評価ではこの違いを保持する。

これらは共通Raw Eventまたは既存runを参照するLearning Ledgerとして保存する。ユーザー要求、Tool出力、Memory本文をLearning Ledgerへ複製しない。削除時は対象sourceに依存する特徴、集計値、checkpointも利用停止・再構築の対象にする。削除対象を含まないcheckpointを確認できなければ初期状態から再構築し、完了までは影響のない固定Policyを使う。保持期限で証拠が欠ける場合も旧重みを無条件に延命せず、残存証拠から再構築する。

学習器が使う集計値やcheckpointは派生物とし、元のSelection Event、Outcome、Feedbackと版から再構築できるようにする。

### 10.3 フィードバックの強さ

初期の優先度は次の順とする。

1. 「違う。こちらのToolを使って」という明示訂正。
2. 選択が合っている／合っていないという明示評価。
3. 受入条件を使ったTask結果の検証。
4. ユーザーが結果を採用したことを示す行動。
5. Toolが技術的に正常終了したこと。
6. 単に呼び出された回数。

暗黙シグナルを、明示的な訂正より強く扱わない。自然言語から評価を抽出する場合、曖昧な発言を負例または正例へ自動確定せず、`unsure`として保持できるようにする。

### 10.4 初期の学習方式

初期Routerは学習なしで成立させる。Phase 4で最初に検証する再順位付けは、大きな学習基盤を導入せず、時間減衰付きの統計とBeta-Bernoulli事後分布を使う。

```text
fit         = Beta(correct + prior, incorrect + prior)
reliability = Beta(success + prior, failure + prior)
preference  = capped(log1p(decayed accepted uses))
```

新しいSkillは、意味検索によって履歴なしでも候補になれる。利用実績は、意味的に近い候補間の再順位付けに限定する。古い履歴は時間とともに影響を減らし、以前の好みにユーザーを固定しない。

### 10.5 将来のContextual Bandit

十分なSelection EventとOutcomeが蓄積した後、Linear Thompson SamplingまたはLinUCBなどのContextual Banditを検討する。

特徴量の候補は、semantic similarity、lexical rank、exact match、入力互換性、Application、Situation、ユーザー選好、信頼性、レイテンシである。新しいToolにも一般化できる共有特徴を使い、Toolごとに独立した利用回数だけで学習しない。

探索は、低リスクかつ意味的な差が小さい候補間に限定する。外部送信、削除、購入、権限変更、公開操作などで、学習目的の探索を行わない。

## 11. Toolの提示と実行

Brokerが選びRuntimeが検査したTool定義だけをProviderへ渡す。共通Runtimeは同じgenerationのSelection Snapshotを実行検査用metadataとして保持する。LLMからTool Callを受け取ったら、少なくとも次を検査する。

- そのToolが今回提示済みである。
- Tool名と引数がschemaへ適合する。
- Selection Snapshotが指すSkill版とArtifact hashが有効である。
- 対象Task、Scope、共有resourceの関連revisionと認可が有効である。無関係なTaskの更新では失効させない。
- 実行直前にも権限、委任、health、リスク条件を満たす。
- 呼出回数、時間、出力量、再試行の上限内である。

組込みToolはNative executor、L-Langは検証済みhost protocol、外部Applicationは専用Adapterというように、executorを明示する。Tool名の文字列だけで任意の実行先へdispatchしない。

Tool出力は、実行した操作の観測または外部データである。Tool出力中の文章を、新しい権限や現在のユーザー指示として扱わない。

Tool実行には、可能な範囲でidempotency key、timeout、cancel、出力量上限、構造化failure codeを持たせる。副作用のある操作は、再起動や応答喪失後に状態確認なしで再実行しない。

## 12. Contextと再検索

Tool Contextは、一度決めた一覧を会話終了まで固定しない。目的、Scope、Task状態、World Stateが変わったら再検索する。再検索結果は共通Context Brokerへ戻し、次のgeneration用ContextEnvelopeとして再構成する。

```text
ユーザー要求
  ↓ Tool Aを提示・実行
Tool Aの結果
  ↓ 目的と未解決状態を更新
次に必要なCapabilityを再検索
  ↓ Tool Bだけを追加提示
```

これにより、複数手順の仕事でも、将来使うかもしれないToolを最初からすべて入れずに済む。

Routerが見落とした場合の回復手段として、小さな`search_capabilities`相当のMeta Toolを常設する案を残す。ただし、Meta Toolが返した名前をそのまま実行せず、Runtimeが候補を再検証して次のProvider requestへ正式なTool定義を追加する。

Selection SnapshotはContextEnvelopeと同じTurn / generationへ結び付ける。ContextEnvelopeに提示されていないTool、Snapshotと版が異なるTool、ScopeまたはTaskの関連状態が変わり再検証されていないToolは実行しない。Tool resultは許可された範囲で先にRaw Eventまたは実行台帳へ記録し、同じTurn内の一時Contextへ追加する。継続に必要な状態はTask Runtimeへ、世界理解の更新はWorld State候補へ渡し、Contextだけに保持しない。

## 13. Privacyと安全性

- Capability検索のために、ユーザー要求を新しい長期ログへ重複保存しない。
- Selection Eventは既存のrunと入力messageを参照し、必要に応じてdigestと数値特徴だけを持つ。
- Embeddingも内容を推測できる派生データとして扱い、元データの削除・失効に追従させる。
- Tool利用履歴から、秘密値、本文、ローカルpath、credentialを診断出力へ漏らさない。
- ユーザーがSkillを無効化、削除、訂正した場合、検索cacheと集計値へ反映する。
- 利用実績が高くても、SUSPENDED、RETIRED、権限外のSkillを候補へ戻さない。
- 外部由来のTool説明、Artifact metadata、Tool出力は信頼済み命令として扱わない。
- Artifactの署名、hash、provenance、検証版を保持し、同名Artifactのすり替えを防ぐ。

## 14. 評価

検索品質は、印象ではなく再現可能なCorpusで評価する。

### Offline評価

日本語を中心に、次を含む要求と期待Capabilityの組を作る。

- 明示的なTool名を含む要求
- Tool名を含まない言い換え
- 複数のCapabilityが必要な要求
- 似ているが別のToolを使う要求
- Toolを使わない要求
- Capabilityはあるが権限や入力が不足する要求
- 誤選択すると影響の大きい要求
- 新しく追加されたL-Lang Skillのcold start

主要指標:

- Capability Recall@K
- Mean Reciprocal RankまたはnDCG
- Wrong Tool Rate
- No Tool Neededのprecision / recall
- 適格性フィルター違反件数
- Tool schemaのContext byte数またはtoken数
- Routingのp50 / p95遅延
- 新規Skill追加後の再index時間

### Online評価

- 明示的な適合／不適合率
- Tool訂正率
- 提示されたが選ばれなかった割合
- 技術的成功率とTask達成率の差
- 同じ要求に対する再試行・Tool切替率
- Context削減量
- ユーザーがTool選択を取り消した割合

学習済み再順位付けを有効にする前にshadow modeで候補と順位だけを記録し、現在の静的提示結果と比較する。重みやmodelは版管理し、問題があれば削除・権限撤回を反映した有効な以前の版へ戻せるようにする。

安全条件の違反は平均精度で相殺しない。権限外Toolの提示・実行、SUSPENDED版の実行、Selection Snapshotと異なるArtifactの実行は0件を必須とする。

## 15. 段階導入

### Phase 0 — Contractと評価Corpus

- 共通Turn Orchestrator、Context Broker、ContextEnvelope、Scope Refとの接続境界を確定する。
- Personal State有効時を含め、別の会話Runtimeを増やさない移行方針を確定する。
- Capability、Skill、Tool、Artifactの型を確定する。
- 既存組込みToolを同じ論理Catalogへ写像する。
- Offline評価Corpusとbaselineを作る。

### Phase 1 — SQLite Registry

- Catalog、Lifecycle、Artifact版をSQLiteへ保存する。
- 既存の単一SqliteWriterを通す。
- Registryから現在の静的Tool一覧を再現し、挙動を変えない。

### Phase 2 — Hybrid Routerのshadow運用

- FTS5とEmbedding総当たり検索を実装する。
- 実際の提示内容は変えず、候補と順位だけを記録する。
- Recall@K、誤選択、安全フィルター、遅延を評価する。

### Phase 3 — Bounded Tool Context

- 低リスクのTool群から、上位候補だけをLLMへ渡す。
- confidenceが低い場合は共通Broker内の検証済み静的候補Policy、追加検索、または確認へ戻す。別の会話Runtimeへ切り替えない。
- Tool数とContext量の削減効果を測る。

Phase 3の共通経路接続gateはPersonal StateロードマップのP1.6に対応する。低リスクの既存Toolで接続を成立させてからWorld Stateへ進み、学習による再順位付けはWorld Stateの初期gate後に独立評価する。L-Lang接続はWorld Stateの前提にしない。

### Phase 4 — Feedback Re-ranking

- 明示的フィードバックと実行結果を保存する。
- Selection Fit、Reliability、Preferenceを分離して再順位付けする。
- shadow、candidate、active、rollbackの手順を持つ。

### Phase 5 — L-Lang Runtime接続

- 検証済みArtifactをSTAGED登録する。
- 明示的にACTIVE化された版だけをRouterへ公開する。
- Selection SnapshotからL-Lang invokeまでを通す。

### Phase 6 — 必要が確認された最適化

- Contextual Bandit
- SQLite Vector拡張またはANN
- Composite Skillの自動候補化
- Capability Gapからの実装提案

データ量や品質上の問題が確認される前に、Phase 6を必須化しない。

## 16. 初期スコープ外

- 生成したCapabilityの自動ACTIVE化。
- Tool利用頻度だけによる自動権限拡大。
- Deep Reinforcement LearningによるEnd-to-End Tool選択。
- 外部Vector Databaseの導入。
- 数千件規模でのANN index導入。
- SQLiteへ実行コードそのものを無検証で保存・実行する仕組み。
- Toolが成功したという理由だけでTaskやGoalを完了にすること。
- すべてのToolを一度にLLMへ渡す互換経路の恒久維持。
- Capability FactoryによるCore Runtimeの自己書換え。
- Memory有効時、Tool利用時、L-Lang利用時だけ別の会話Runtimeへ切り替えること。
- Capability RouterがProvider request、会話履歴、Memory projectionの正本を所有すること。

## 17. 現在の実装との接続

現在の会話Runtimeは、組込みToolの定義と実行先を静的に構成している。初期移行では、既存のTool定義と制限をそのままRegistryへ写像し、Registry経由でも同じ一覧と実行結果を再現する。実装着手時には実際のmodule配置を再調査し、このConcept内のpath名を移行契約として固定しない。

現在はPersonal State有効時に専用会話経路へ分岐し、通常のContext Window、Provider routing、会話Contextとは別にTool定義を構成する。この状態を目標構成とはみなさない。Capability Registryを両経路へ個別接続せず、先に共通Turn OrchestratorとContext Brokerの入力契約を確定し、Personal StateをContext Sourceとして接続する。Tool Routerはその共通経路だけへ接続する。

SQLiteはRust Runtimeの単一Writerが書込みを所有し、Readerはread-only接続を使う。この所有構造を維持する。Catalog登録、Lifecycle変更、Selection Event、Outcome、Feedbackは既存Writerを通す。

会話検索ではFTS5 trigramとBM25を利用している。Capabilityの字句検索は、この実績のある構成を再利用候補とする。

LARMのEmbedding接続では、queryとpassageの384次元・L2正規化済みVectorが実機確認されている。ただし意味検索品質は未評価であるため、Capability Corpusによる評価を経て採用する。

L-Lang Wasm hostは開発用integration testでinspect、verify、invokeを確認済みだが、通常会話Tool、Registry、配布アプリ、受入gateには未接続である。本構想は、その接続先となる。

## 18. 未決事項

- Capability IDとTool名のnamespace規則。
- Built-in ToolとL-Lang Toolの衝突回避方法。
- 一つのCapabilityに複数Skillがある場合の既定選択とユーザー設定。
- Embedding providerが利用できない場合のFTS-only fallback条件。
- Feedback UIを毎回表示せず、必要な場面だけ集める条件。
- 複数Capabilityが必要な要求の分解をRouterとPlannerのどちらが担うか。
- Capability Gapを、提案、実装依頼、単なる不足報告のどこまで自動化するか。
- Tool Contextの件数上限とbyte / token予算。
- Selection EventとEmbeddingの保持期間、削除連動。
- L-Lang Artifactの署名、配布、bundle同梱、rollback手順。
- Contextual Banditを有効化するために必要な最低データ量と安全な探索範囲。

## 19. この構想の完了条件

この構想が実装されたと言えるのは、少なくとも次を満たしたときである。

- 組込みToolとL-Lang Toolを、同じCapability / Skill / Toolモデルで扱える。
- SQLiteのRegistryから、現在利用可能なToolを再構築できる。
- ユーザー要求から関連Capabilityを検索し、少数のToolだけをLLMへ提示できる。
- Toolを提示した理由と、除外した決定的理由を追跡できる。
- ユーザーの明示訂正が、次回の同種要求での順位へ反映される。
- 選択適合度、実行信頼性、ユーザー選好を別々に確認できる。
- Tool追加後も、全Tool説明をSystem Contextへ常時追加する必要がない。
- Memory、World State、Toolの有効状態にかかわらず、同じTurn OrchestratorとContext Brokerを通る。
- Sessionlessな会話を維持しながら、Capabilityの適用範囲をScopeで分離できる。
- 検索や学習結果が、権限、委任、Lifecycle、実行時検査を迂回しない。
- Toolの正常終了とユーザー目的の達成を区別できる。
- 問題のあるRouter、Embedding、重み、Artifact版を切り戻せる。
