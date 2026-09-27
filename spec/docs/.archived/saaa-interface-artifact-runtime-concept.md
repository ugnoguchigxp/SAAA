# SAAA Interface / Artifact Runtime Concept

## 会話から画面と成果物を作り、必要なファイルを再発見できるPersonal Workspace

作成日: 2026-09-17  
状態: コンセプト案 v0.1

## 1. この文書の位置付け

この文書は、SAAAにおける動的UI、Markdown、Office文書、添付ファイル、会話との関係、保存、検索、再利用の基本構想を定義する。

SAAAは会話だけを保存するChatアプリではなく、会話を入口として画面、文書、表、プレゼンテーション、作業成果物を作り、後から意味と出典を保ったまま再利用できるPersonal Workspaceを目指す。

上位方針は[SAAA Personal AI Concept](saaa-personal-ai-concept.md)に従う。Toolの発見と実行は[Capability / Tool Runtime Concept](saaa-capability-tool-runtime-concept.md)、MemoryとContextへの選択は[Adaptive Learning and Selective Memory Concept](saaa-adaptive-learning-memory-concept.html)および[ContextWindow / Personal State共通Runtime統合実装計画](context-memory-unified-runtime-implementation-plan.md)に従う。

この文書のArtifactは、Capability文書にある実行可能packageとしてのArtifactより広い。混同を避けるため、本文ではユーザーの文書や成果物を`Content Artifact`、ToolやSkillの配布物を`Executable Artifact`と呼ぶ。

本文の「SAAAは〜する」は目標設計を表し、すべてが実装済みという意味ではない。現行実装、採用方針、将来候補を区別する。最終的なSQLite schema、ファイル形式、IPC、画面仕様は実装計画で確定する。

## 2. 現在の実装評価

### 2.1 動的UI

現在のGenerative Inline UIには、次の基盤が実装されている。

- LLMがHTML、CSS、JavaScriptを直接生成しない、検証済みSemantic UI JSON
- `Grid`、`Stack`、`Cell`、`Text`、`Metric`、`Table`、`Chart`、`Status`、`Actions`、`ModelStatus`の限定Component Library
- 12列layout、Component数・深さ・文字数・保存サイズの上限
- View、追記型Revision、Instance、live/snapshot、公開、検索、archive
- 会話messageからUI instanceへの参照
- 許可されたData Sourceだけを参照する取得境界
- 未知Component、実行式、URL、任意HTML、任意Tool実行の拒否

これは安全なSemantic Component Libraryである。加えて、Design Token、基本Primitive、application rootのProvider、SAAA UI Adapterが実装され、Semantic RendererはAdapter経由でDesign Systemを利用する。保存済みSemantic UIにはDesign System固有のComponent名、props、CSS class、token名を含めない。

Design Systemの初期統合は完了したが、アプリ全体への展開は完了していない。現状は次が不足している。

- 通常画面に残るraw color、余白、角丸、文字、影、motionのToken移行
- light themeを含む複数Theme契約とユーザー選択
- Componentの密度、サイズ、variant、用途の網羅的な標準
- Typography、icon、form、feedback、empty/error/loadingの統一規則
- WCAGを基準にしたcontrast、keyboard、screen readerの受入条件
- Component Library版とToken版の互換契約
- Figma等の設計資産とcodeの対応付け

Generative UIのlayout、Metric、Table、Chart、ActionはAdapterへ接続済みであり、未知Component拒否と保存形式の互換性も維持している。一方、通常画面の多くには色値や寸法の直接記述が残る。したがって結論は「Design Systemの基礎とSemantic Renderer統合は実装済み。アプリ全体の移行と受入基準の完成は継続課題」である。

### 2.2 ファイルと検索

現在は、会話本文を`conversation_messages`へ保存し、SQLite FTS5/BM25で会話を検索できる。Generative UIはUI定義、Revision、Instance、SnapshotをSQLiteへ保存できる。

一方、一般ファイルについては、次を一貫して扱うContent Artifact基盤がない。

- Markdown、PDF、Word、Excel、PowerPoint、画像等の登録
- 原本、版、抽出結果、preview、出典の管理
- 会話message、Task、Project、UIとの参照関係
- ファイル横断の全文検索と意味検索
- 同名ファイル、更新、移動、削除、外部変更の扱い
- LLMが検索結果を引用し、必要な範囲だけContextへ入れる契約

設定を保存する`settings_documents`はアプリ設定用であり、ユーザー文書の保存基盤ではない。coding workspaceのpath登録も、一般Artifact Catalogではない。

## 3. Vision

> SAAAは、会話を入口に必要な画面と成果物を作り、すべてのContent Artifactを出典・版・Scope付きで保持し、ユーザーの言葉から該当するものを安全に再発見できる。

目指す体験は次の通りである。

- 「先週作った見積もりを開いて」で、会話、ファイル名、内容、Taskとの関係から候補を特定する。
- 「この表を経営会議向けに見せて」で、同じデータを表、Chart、PowerPointなど適切な表現へ投影する。
- 「この会話を議事録にして保存して」で、会話を根拠にMarkdownまたはOffice文書を新しいArtifactとして作る。
- 「さっきの資料の3ページ目」で、曖昧な参照を現在のTaskとActive Referentから解決する。
- ファイルが更新された場合、古い内容を現在版として答えず、参照した版と更新状態を示す。
- 見つからない場合、存在を捏造せず、検索範囲や候補をユーザーへ確認する。

## 4. 中核原則

### 4.1 原本、抽出、索引、表示を分ける

一つのファイルを、そのままSQLite、Embedding、LLM Context、画面表示へ複製しない。

```text
Original Content
  ├─ Version metadata and digest
  ├─ Normalized semantic representation
  ├─ Search chunks and indexes
  ├─ Preview / thumbnail / rendered pages
  └─ UI or Context projection
```

原本だけが文書内容の正本である。抽出text、chunk、Embedding、previewは原本版から再構築できる派生物とする。

### 4.2 SQLiteは台帳、ファイルシステムはpayload store

SQLiteにはArtifact ID、種類、版、digest、Scope、関係、抽出状態、検索索引、操作履歴を保存する。大きなbinary payloadを無条件にSQLite BLOBへ入れない。

管理対象ファイルは、アプリ専用directoryのcontent-addressed storeへ保存する。payload pathはユーザー入力を直接連結せず、hashと内部IDから決定する。

外部workspaceのファイルを参照する場合は、管理copyと外部参照を区別する。外部pathだけを永続的なidentityにせず、platform file identity、digest、更新時刻、size、workspace scopeを組み合わせ、利用時に再検証する。

### 4.3 会話とファイルを同じ本文テーブルへ押し込まない

会話messageとContent Artifactは別の正本を持つ。関係は型付きlinkで表す。

```text
Conversation Message ── mentions / attaches / creates / cites ── Content Artifact Revision
Task                 ── input / output / deliverable           ── Content Artifact
UI View              ── visualizes / edits / exports            ── Content Artifact Revision
```

会話を削除しただけで、ユーザーが明示保存した成果物を削除しない。逆に、添付だけの一時Artifactは、参照がなくなったときのretention policyに従う。

### 4.4 ファイル内容は命令ではなくdata

文書中の文章、macro、外部link、埋め込みobject、コメントは、LLMやRuntimeへの命令権限を持たない。Content Artifactから抽出したtextは、現在のユーザー命令と別のdata領域へ置く。

文書に「このToolを実行せよ」と書かれていても、それだけで権限、委任、Tool提示、外部送信を成立させない。

### 4.5 検索と認可を分ける

意味的に近いArtifactを見つけることと、内容を読んでよいことは別である。検索前にprincipal、purpose、Scope、classificationで候補範囲を絞り、検索結果を返す直前にも再検証する。

Embedding similarity、利用回数、最近開いた回数によって、別Projectや別Userの内容を混入させない。

### 4.6 SessionlessだがScopedにする

Artifactは会話Sessionへ所有させない。`user`、`project`、`task`、`resource`、`request`のScopeと、明示linkへ属する。

会話はArtifactを発見・作成・更新する入口の一つであり、唯一のfolderではない。同じArtifactを別の会話から参照できるが、ScopeとPolicyを満たす場合に限る。

### 4.7 自動保存と明示保存を区別する

会話への添付、途中生成、preview、export済み成果物を同じ「保存済み」と表示しない。

初期Lifecycleは次を基本とする。

```text
STAGED -> INDEXING -> READY -> SUPERSEDED -> ARCHIVED
   |          |
   |          └─ DEGRADED / FAILED
   └─ EPHEMERAL -> EXPIRED
```

ユーザーが明示保存したArtifact、Taskの成果物、一時的な中間生成物を区別する。

## 5. SAAA Design System

### 5.1 Design Systemの責務

Design Systemは見た目のcatalogだけではない。SAAAでは、視覚表現とUI部品を提供する基盤と、LLMが安全にUIを構成するための公開語彙を分ける。Design Systemは前者を担い、SAAAのSemantic UI契約は後者を担う。

```text
Design Tokens
    ↓
Primitive Components
    ↓
Semantic Components
    ↓
Patterns / Layouts
    ↓
Semantic UI Definition
    ↓ validate
Renderer + Data Source + Action Policy
```

LLMへCSS propertyや任意Component APIを渡さない。LLMは「何を伝える画面か」をSemantic Componentで指定し、色、余白、breakpoint、focus、loading、errorはDesign Systemが決める。

### 5.2 Design Systemの組み入れ方

Design SystemをLLMへ直接公開せず、SAAA内に統合境界を置く。依存方向は常にSemantic UIからDesign Systemへ向け、Design System固有のComponent名、props、CSS class、token名を会話、保存UI、Tool schemaへ漏らさない。

```text
LLM
  ↓ Semantic UI Definition
Schema Validator ─ Data Source ─ Action Policy
  ↓ stable SAAA component IDs
Semantic Renderer
  ↓ SAAA UI Adapter
Design System Components + Tokens
  ↓
Browser / Desktop UI
```

責務は次のように分ける。

| 層 | 所有するもの | 所有しないもの |
| --- | --- | --- |
| Design System | theme、token、primitive、interaction state、motion、accessibility基準 | LLM schema、業務data、Tool実行、保存UIの互換性 |
| SAAA UI Adapter | 安定したprops、状態変換、token alias、fallback、Design System差し替え境界 | user intent、検索、権限判断 |
| Semantic Renderer | Semantic Componentの検証と描画、Data Sourceの値の受け渡し | raw CSS、任意Component生成 |
| Data Source / Action Policy | 読取範囲、更新権限、確認、監査 | layoutと見た目 |

#### 統合規則

1. Design Systemはversionを固定したdependencyとして導入し、branchや移動する参照を実行時依存にしない。
2. SAAAはDesign SystemのComponentを画面から直接importせず、原則として`SAAA UI Adapter`を経由する。
3. tokenはSAAAの意味名へaliasする。保存データには`surface.raised`等の意味を保存し、特定libraryのCSS variable名や色値を保存しない。
4. Theme Providerはapplication rootに一度だけ置き、通常画面、会話内UI、Artifact preview、dialog、portalで同じtheme contractを使う。
5. layout、data、actionを分離する。Design System ComponentへSQL、file path、Tool arguments、credentialを渡さない。
6. Design SystemにないChart、Citation、Document Preview等は、同じtokenとprimitiveを使うSAAA Componentとして実装する。
7. Component追加は先にSAAAの用途とschemaを定義し、その後で既存Primitiveへの写像または新規実装を決める。
8. dependency更新時はtype-checkだけでなく、visual regression、keyboard、screen reader、contrast、compact layout、saved UI replayを検証する。

#### Componentの写像

| Semantic Component | Adapterで組み立てる視覚要素 | SAAAが保持する振る舞い |
| --- | --- | --- |
| `Metric` | Card、Text、Badge | 値の型、単位、source、更新時刻 |
| `Status` | Badge、Icon、Tooltip | status vocabulary、severity、説明 |
| `Table` | Table、Input、Pagination、Empty State | query、sorting、filter、列schema、上限 |
| `Chart` | Panel、Legend、Tooltipと描画層 | chart schema、系列、data上限、fallback table |
| `Actions` | Button、Menu、Dialog | capability、confirmation、delegation、audit |
| `ArtifactList` | List、File Icon、Badge、Skeleton | Artifact query、Scope、revision、selection |
| `DocumentPreview` | Panel、Tabs、Toolbar、Loading/Error | representation選択、locator、引用、degraded state |

通常画面とGenerative UIは別々のComponent群を持たない。同じAdapterを使い、LLMに公開する範囲だけをComponent Manifestで制限する。これにより見た目は共有しつつ、LLMが低水準APIへ依存することを防ぐ。

導入は一括置換にしない。最初にthemeとtoken aliasを接続し、次にButton、Input、Badge、Card、Loading、Error等の基本PrimitiveをAdapterへ移す。その後、通常画面とSemantic Rendererを機能単位で切り替え、最後に重複CSSと旧Componentを削除する。移行中は旧実装と新実装を同じSemantic Component IDの別Rendererとして比較できるようにする。

### 5.3 Token層

初期Tokenは次の意味階層を持つ。

| Token群 | 例 | 方針 |
| --- | --- | --- |
| Color | `surface.canvas`、`surface.raised`、`text.primary`、`status.danger` | raw color名ではなく用途で参照 |
| Typography | `body.md`、`label.sm`、`title.lg`、`data.numeric` | font、size、weight、line-heightを一体化 |
| Space | `space.1`〜`space.8` | 任意pixelを禁止 |
| Radius | `radius.control`、`radius.panel`、`radius.pill` | Component用途に対応 |
| Border | `border.subtle`、`border.focus`、`border.danger` | contrastをThemeごとに保証 |
| Elevation | `elevation.overlay`等 | stackingとshadowを一体管理 |
| Motion | `motion.fast`、`motion.enter` | reduced-motionを尊重 |
| Layout | `content.readable`、`grid.columns`、`breakpoint.compact` | 通常画面とinline UIで共有 |

Tokenの正本は型付きsourceとし、CSS custom properties、TypeScript型、必要ならFigma variablesを生成する。CSS内の色値を別々に更新しない。

### 5.4 Component層

現在のSemantic Componentを維持しつつ、次の層へ整理する。

- Primitive: `Button`、`Input`、`Select`、`Badge`、`Panel`、`Table`、`Dialog`
- Data display: `Metric`、`Status`、`Chart`、`Timeline`、`KeyValue`、`DocumentPreview`
- Content: `Text`、`Markdown`、`Citation`、`FileCard`、`ArtifactList`
- Layout: `Stack`、`Grid`、`Cell`、`Tabs`、`Section`
- Feedback: `Loading`、`Empty`、`Warning`、`Error`、`Progress`
- Action: policyで許可された型付きActionだけ

LLM公開Componentと、アプリ内部だけで使うPrimitiveを分ける。すべてのPrimitiveをLLMへ公開しない。

### 5.5 Component Manifest

各LLM公開Componentは次を持つ。

- 安定IDとlibrary version
- 用途、適する場合、避ける場合
- 厳格なprops schema
- 対応するData Source型
- interactionとAction分類
- live/snapshot/export対応
- accessibility contract
- responsive behavior
- fallback representation
- migration adapterまたは利用可能期間

古い会話内UIは保存時のlibrary versionで再現する。互換Rendererがない場合は、壊れた画面ではなく要約、data table、またはsnapshot previewへ縮退する。

### 5.6 UI、Data、Actionの分離

Semantic UI definitionはlayoutと表現だけを持つ。実データはData Source、操作はAction Policyが提供する。

```text
UI Definition: Metric(source="task.summary", field="open")
Data Source:   scope検証済みのtyped query
Action:        capability + delegation + confirmation
```

UI定義にSQL、URL、file path、Tool arguments、credentialを埋め込まない。保存UIを別の会話で開いても、新しいScopeでData SourceとActionを再認可する。

### 5.7 Artifactとの接続

保存済みUI ViewもContent Artifactの一種としてCatalogへ登録できる。ただしUI revisionの正本は既存のUI台帳に置き、Artifact Catalogには種類、名称、Scope、最新版、検索文書、関係だけを持たせる。

UIはArtifactを表示できるが、表示しただけで内容を複製しない。固定表示が必要な場合はsnapshot revisionを明示的に作る。

## 6. Content Artifact Model

### 6.1 ArtifactとRevision

論理Artifactと不変Revisionを分ける。

| 概念 | 主な内容 |
| --- | --- |
| Artifact | 安定ID、kind、title、lifecycle、owner、Scope、latest revision |
| Revision | revision ID、親revision、content digest、size、media type、作成理由、作成者、時刻 |
| Payload | managed object、external reference、または両方 |
| Representation | plain text、structured blocks、sheet cells、slides、page images、thumbnail |
| Chunk | 検索とContext投入のための、位置情報付きの小区間 |
| Relation | message、task、project、UI、他Artifactとの型付きedge |
| Index State | pending、ready、degraded、failed、extractor version |

Revisionは書き換えない。編集、importし直し、外部変更の取込みは新しいRevisionを作る。

### 6.2 種類

初期kindは、拡張子ではなく扱い方で分類する。

- `text.markdown`
- `document.word`
- `sheet.workbook`
- `presentation.deck`
- `document.pdf`
- `image.raster`
- `ui.view`
- `conversation.export`
- `generic.file`

拡張子だけを信用せず、media type、magic bytes、package構造、parser結果を照合する。macro付きOffice文書や未知形式は、通常文書と同じ信頼レベルで自動処理しない。

### 6.3 Formatごとの正本と派生表現

| Format | 正本 | 主な派生表現 |
| --- | --- | --- |
| Markdown | UTF-8 source | heading tree、paragraph、code block、link、plain text、rendered preview |
| Word | 元のOpen XML package | paragraph、heading、table、comment、page preview、plain text |
| Excel | 元のworkbook | sheet metadata、cell range、formula/value、table、chart metadata、preview |
| PowerPoint | 元のpresentation | slide、shape text、speaker notes、table、image relation、slide preview |
| PDF | 元のPDF | page text、layout block、image、page preview、OCR result |
| Image | 元画像 | metadata、thumbnail、OCR、caption候補 |

派生表現は元の位置を失わない。Wordの段落、Excelのsheet/range、PowerPointのslide、PDFのpageと座標へ戻れるlocatorを持つ。

### 6.4 Conversationとの関係

messageは複数のContent Partを持てる構造へ発展させる。

```text
Message
  ├─ Text Part
  ├─ UI Part
  ├─ Artifact Reference Part
  └─ Citation Part
```

大きな文書本文をmessage contentへ埋め込まない。messageにはArtifact revision ID、表示名、種類、短い要約、relationを保存し、本文はArtifact Runtimeから解決する。

主なrelationは次とする。

- `attached`: ユーザーが会話へ添付した
- `created`: このrunが新しいArtifactを作った
- `derived_from`: 別Artifactまたは会話から派生した
- `cites`: 回答が特定範囲を根拠にした
- `edits`: 既存Artifactの新Revisionを作った
- `visualizes`: UIがArtifactを表示する
- `deliverable_of`: Taskの成果物である

## 7. 取込みPipeline

取込みは一つのtransactionで完了したように見せず、段階を明示する。

```text
Select / Drop / Tool output / Workspace discovery
                    ↓
          Stage and validate source
                    ↓
       Sniff type, size and safety limits
                    ↓
      Create Artifact + immutable Revision
                    ↓
     Store managed payload or external reference
                    ↓
        Extract normalized representations
                    ↓
         Chunk with source locators
                    ↓
      FTS index / optional vector index
                    ↓
          Preview and metadata ready
```

取込み中でもArtifact IDは返せるが、検索可能、preview可能、LLM利用可能を別の状態で示す。parserが失敗した場合も原本を失わず、metadataだけの`DEGRADED`として扱える。

受入上限はformatごとに定める。zip bomb、path traversal、過大な展開、破損package、macro、外部link、埋め込みfileを検査する。Office macroを実行しない。

## 8. LLMが該当ファイルを見つける仕組み

### 8.1 検索前の参照解決

すべてをVector Searchへ投げる前に、明示参照を解決する。

1. 現在入力にArtifact ID、添付、file名、UI選択があるか
2. 現在のrequest、task、projectに明示linkがあるか
3. 「このファイル」「さっきの表」にActive Referent候補があるか
4. 完全一致、prefix、既知のaliasがあるか
5. それでも不足する場合に検索する

明示参照は意味類似度より優先する。ただしScopeと認可は省略しない。

### 8.2 Hybrid Retrieval

初期検索は次を組み合わせる。

```text
Scope / Policy filter
        ↓
Exact metadata match
        +
SQLite FTS5 / BM25
        +
Optional sqlite-vec semantic search
        ↓
Deterministic fusion and reranking
        ↓
Bounded Artifact candidates
```

FTS5は名称、tag、heading、本文chunk、sheet名、slide title、会話上の説明を対象にする。日本語tokenizationの品質は別途評価し、単純な空白分割を前提にしない。

sqlite-vecは必須の正本ではない。数千Artifactまたは数万chunk規模では運用しやすい候補だが、まずFTS5、metadata、Scope、固定corpusのbaselineを作る。語彙差による取りこぼしが確認できた場合に、version付きEmbedding indexとして追加する。

Embeddingは次を守る。

- 原本ではなく再生成可能な索引
- embedding model ID、次元、chunk digestを記録
- model変更時は新indexを並行作成
- restricted contentを許可なく外部Embedding APIへ送らない
- embedding scoreで認可、現在版、削除、忘却を上書きしない

### 8.3 Reranking

順位付けは一つの不透明なscoreへ早期統合しない。少なくとも次を特徴として保持する。

- 明示file名、Artifact ID、現在選択との一致
- Scope距離
- lexical score
- semantic score
- title、heading、tagの一致
- current revisionか
- Task input/output/deliverable関係
- recent useとlast opened
- userの適合／不適合feedback
- extractor healthとcontent availability

利用頻度は意味的に適合した候補間の補助に使う。よく開くファイルという理由だけで、異なる要求へ常に提示しない。

### 8.4 二段階取得

LLMへ最初から全文を渡さない。

1. Discovery: title、kind、Scope、短いsummary、matched locationだけを返す。
2. Read: LLMまたはユーザーが選んだArtifactの必要範囲だけを取得する。

同名または近い候補が複数あり、Task関係でも決められない場合は、候補を示して確認する。検索上位を黙って正解扱いしない。

### 8.5 Context Brokerへの接続

Artifact Retrievalは独立した会話Runtimeを持たない。候補を共通Context Brokerへ渡し、現在入力、Memory、Task、Tool定義と同じ予算で選択する。

Context manifestには、利用したArtifact revision、chunk locator、digest、選択・省略理由を記録する。原本本文や完成Contextは監査台帳へ複製しない。

Artifactが更新、削除、認可変更された場合、依存generationは完了時に再検証する。無関係なArtifact更新で全generationを失効させない。

## 9. 編集、保存、Export

LLMの編集は原本のin-place上書きを基本にしない。

```text
Read selected revision
        ↓
Create typed edit plan
        ↓
Validate format and user intent
        ↓
Create new immutable revision
        ↓
Render / recalculate / inspect
        ↓
Publish latest revision atomically
```

Markdownはtext diffを利用できる。Officeはformat別Adapterが構造を理解し、Word paragraph、Excel range、PowerPoint slide等の型付き操作を行う。生成後は再度開いて構造とpreviewを検証する。

`Save`、`Save As`、`Export`を区別する。

- Save: 管理Artifactの新Revisionを作る
- Save As: 新しい論理Artifactを作る
- Export: 管理store外へcopyを作る。以後の同期を保証しない

外部ファイルの上書き、macro保持、既存書式の破壊可能性がある操作は、Capability Policyと確認境界に従う。

## 10. UIとArtifactを結ぶ利用体験

UIはArtifactの代替ではなく、現在の目的に合わせたViewである。

例:

- Excel workbookを検索し、対象tableをinline `Table`と`Chart`で表示する。
- 会話からMarkdown議事録を作り、`DocumentPreview`として表示する。
- 複数資料を比較し、比較UIから採用案を選び、新しいWord文書へ保存する。
- PowerPointの各slideを一覧表示し、選択したslideだけを修正する。

UIの状態変更とArtifact編集は分ける。tableのsort、filter、pageはUI instance stateであり、workbookの変更ではない。Artifactを変更するActionは、明示されたtyped editとして別に実行する。

## 11. World Model、Memory、Taskとの境界

| 領域 | 保持するもの | 保持しないもの |
| --- | --- | --- |
| Artifact Runtime | 原本、版、構造、索引、関係 | ユーザーの恒久的な好み |
| Personal Memory | ユーザーの好み、経験、継続条件 | Office binaryや全文copy |
| World Model | 現在のProject、Task、選択中Artifact、関係、状態 | 原本本文の第二正本 |
| Task Runtime | 入力、成果物、進捗、委任、検証結果 | 一般検索索引 |
| Context Broker | 今回使う限定投影 | 永続的なファイルstore |

「ユーザーは週次報告をPowerPointで好む」はMemory候補になり得る。「2026-Q3-review.pptxの5枚目に売上Chartがある」はArtifact indexとWorld/Task上の参照である。本文全体をMemoryへ移さない。

## 12. Privacy、削除、Backup

削除は次を区別する。

- 会話から参照を外す
- Artifactをarchiveする
- managed payloadと派生表現を削除する
- external referenceだけを解除する
- 検索indexとEmbeddingを削除する
- export済みcopyはSAAAの管理外であると示す

忘却要求では、link、chunk、FTS、vector、preview、cache、未完了jobを対象revisionへ追跡する。DB rowだけを消してpayloadを残さない。payloadだけを消して検索snippetを残さない。

BackupはSQLite台帳とmanaged payload storeを同じsnapshot generationとして扱う。片方だけ復元した場合は整合性検査を行い、欠落payloadを利用可能と表示しない。

## 13. 観測と学習

次のeventを本文なしで記録できる。

- queryと候補のdigest
- 候補、順位、選択理由、除外理由
- open、preview、read、cite、edit、export
- userの「これ」「違う」「次から優先」
- extractor、index、render、edit検証の結果

検索改善では、ユーザーが開いたことを即「正解」としない。誤って開いて戻った場合、候補を確認しただけの場合、内容を引用して目的を達成した場合を分ける。

学習前に決定的baselineを持つ。評価指標は、Top-1/Top-5 retrieval、誤Scope混入、古いrevision選択、確認回数、目的達成、検索遅延とする。認可違反と削除済み内容の再提示は許容0件とする。

## 14. 初期schemaの論理案

最終名ではないが、次の責務を分離する。

| データ群 | 主な内容 |
| --- | --- |
| `artifacts` | 安定ID、kind、title、lifecycle、latest revision |
| `artifact_revisions` | 親版、digest、media type、size、payload mode、作成理由 |
| `artifact_scopes` | ArtifactとScopeのrelation |
| `artifact_relations` | message、task、UI、他Artifactとの型付きedge |
| `artifact_representations` | representation kind、extractor version、digest、state |
| `artifact_chunks` | revision、locator、ordinal、text digest、classification |
| `artifact_fts` | title、tag、heading、chunk textのFTS5派生index |
| `artifact_embeddings` | chunk、model、dimension、vector、content digest |
| `artifact_jobs` | import、extract、OCR、render、index、deleteの有限job |
| `artifact_events` | revision、publish、archive、delete、verificationの台帳 |
| `message_parts` | text、UI、Artifact reference、citationの順序付きpart |

FTSとEmbeddingは再構築可能にする。relation、Scope、Revision、Eventは正本である。

## 15. 段階導入

### Phase 0: Design System基礎

- Design System dependencyと互換versionを固定
- peer dependency、style scope、package export、browser testの採用条件を確認
- Design System tokenとSAAA semantic tokenの対応表を作成
- application rootへTheme Providerを接続し、portalを含むtheme contractを統一
- SAAA UI Adapterを追加し、基本primitiveから段階移行
- 現行Semantic Componentをmanifest化し、Adapterへの写像を記録
- loading、empty、error、focus、compact layoutの受入fixture
- visual regression、keyboard、contrast、saved UI replayのCI検査
- Design System、Adapter、Semantic UIそれぞれのversionとmigration契約

完了条件: 固定したDesign System packageで依存関係とbrowser testが成立し、通常UIとGenerative UIが同じAdapter、Theme、contrast、focus contractを使う。動的UIと保存データにはraw color、任意spacing、任意CSS、Design System固有propsを公開しない。

### Phase 1: Markdown Artifact

- Artifact/Revision/Relation/Job台帳
- managed content-addressed store
- Markdown import、作成、編集、preview
- messageのArtifact Reference Part
- title/tag/heading/bodyのFTS5検索
- Context Brokerへのchunk候補接続

完了条件: 会話からMarkdownを作成・再発見・引用でき、別Scope混入、古い版のcurrent扱い、本文の命令化が0件である。

### Phase 2: Office read path

- Word、Excel、PowerPointの原本保存
- format別の構造抽出とlocator
- preview生成
- file名、metadata、本文、sheet、slideのHybrid Retrieval
- degraded/unsupported表示

完了条件: 固定corpusで正しいArtifact Top-5率95%以上、Scope漏洩0件、引用箇所を原本位置へ戻せる。

### Phase 3: Office create/edit path

- 型付きedit plan
- 新Revision作成
- render、reopen、formula、layout検証
- Save、Save As、Export
- Task deliverableとの接続

完了条件: 原本の暗黙上書き0件、検証前publish 0件、代表文書で内容・構造・表示の受入を満たす。

### Phase 4: Semantic retrieval

- FTS baselineと失敗corpus
- local embedding modelの認定
- sqlite-vec index
- lexical/semantic fusion
- feedbackとablation評価

完了条件: FTS baselineに対して事前固定したretrieval指標を改善し、誤Scope、古いrevision、削除済み候補を増やさない。改善が確認できなければFTSのみを維持する。

### Phase 5: UI / Artifact統合

- `DocumentPreview`、`ArtifactList`、`Citation`等をSemantic Libraryへ追加
- Artifact Data Sourceとtyped Action
- UIからcreate、compare、edit、exportをTaskとして開始
- saved UI viewのArtifact Catalog登録

完了条件: UI stateとArtifact mutationが混同されず、保存UIを別Scopeで開いてもdataとactionが再認可される。

## 16. 評価と非目標

初期評価では次を測る。

- Exact name、内容語、言い換え、会話参照、Task参照ごとのTop-1/Top-5
- Markdown、Word、Excel、PowerPointごとの抽出率とlocator精度
- current revisionの選択率
- Scope外候補、削除済み候補、壊れたpreviewの発生数
- import、検索、preview、部分読取のp50/p95
- Contextへ投入したbyte/tokenと引用根拠率
- UI生成成功率、repair回数、fallback率、アクセシビリティ違反

初期段階では次を約束しない。

- 任意Webページや任意Cloud Driveの自動同期
- Office macroの実行
- すべてのOffice機能の完全なround-trip
- Vector Searchだけによる正答保証
- ファイル内容からの自動権限付与
- すべての会話とファイルを無制限にLLMへ渡すこと
- export先を含む物理secure erase

## 17. 判断

SAAAへDesign Systemを組み入れるが、LLM向けschemaや保存形式として直接露出させない。検証済みversionを固定してSAAA UI Adapterの背後で利用し、Design Systemはtoken、theme、primitive、accessibilityを担う。SAAAのSemantic Component LibraryはLLM向けの安定契約として維持し、任意UI codeやDesign System固有propsを生成させず、Component Manifest、Data Source、Action Policyを接続する。

ファイル保存はMemoryの一部として実装しない。Content Artifact Runtimeを独立した正本として設け、会話、Task、UI、Memory、World Modelとは型付き参照で接続する。

検索の初期正本はSQLite metadataとFTS5/BM25とする。sqlite-vecは、Officeを含む固定corpusで語彙差の取りこぼしが確認された後に、再構築可能な派生indexとして追加する。最終選択は明示参照、Scope、版、lexical、semantic、Task関係を統合し、共通Context Brokerが予算内で行う。
