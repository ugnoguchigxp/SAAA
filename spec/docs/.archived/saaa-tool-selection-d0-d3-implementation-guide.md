# ツール選択 D0〜D3 — 担当AI向け詳細実装手順

作成日: 2026-09-20 / v1 / コード実装前

上位: [M2B v2](saaa-llang-dynamic-capability-m2b-plan.md)。この文書はD0〜D3の未決定部分を固定する。詳細が競合する場合はこの文書を優先する。D4/D5のMCP transport、D6の学習は実装しない。

## 0. 最初に守る作業手順

作業順は `T00 → T01 → T02 → T03 → T04 → T05 → T06 → T07 → T08 → T09 → T10 → T11 → T12 → R1 → R2`。各カードの合格条件を満たしてから進む。モデル配備待ちでもDB・fixture・純関数の独立作業は進めるが、mockだけでD2/D3完了とは言わない。

開始時のtracked/untrackedコード、HEAD、status、関連ファイルhashをrepository外へ保存する。別作業のworld-model変更を消さず、既存変更も自分の成果と数えない。今回は新規tableと内部APIの追加を許可する。既存conversation/public IPCの無関係な変更、別SQLite正本、新規外部サービスへの履歴送信は禁止。

### 固定する実装上の選択

| 項目 | 採用する方式 |
| --- | --- |
| 初期backend | L-Lang本番backend＋cfg(test)のfixture backend。fixtureは本番登録不可 |
| 認可 | 単一ローカルprofile主体を基準にした明示grant。group対応は後続 |
| project/task | ホストに確定IDがあれば利用。なければnull。LLMがIDを発明しない |
| task不明時 | 一時訂正はconversation限定＋24時間。run限定にして次の発言で失効させない |
| 検索cache | D0〜D3では作らない。vectorの読み込みcacheのみgeneration付きで可 |
| 参照 | メモリ内opaque UUID v4、run所有、10分TTL、64件/run。再起動後失効 |
| 用法ページ | SQLiteにsection/page単位で保存、1page最大8 KiB。schemaは分割しない |
| 語彙検索 | FTS5 trigram。3 Unicode文字未満は語彙検索を省略しembeddingへ |
| ML runtime | 固定ローカルPython worker一つ、stdin/stdout JSONL、Embeddingとrerankを担当 |
| 訂正抽出 | 既存設定で選択済み会話providerを使うtoolなしの構造化抽出。未設定なら未抽出状態を明示 |
| 新規設定 | SAAA_TOOL_SELECTION_CONFIG。未設定時は既存M2A directモード |

## 1. ファイルと責務

新規 `src-tauri/src/tool_selection/mod.rs` をlib.rsへ登録し、以下を小さいmoduleに分ける。既存ファイル名は作業時に存在を確認する。

| ファイル | 中に置くもの | 置かないもの |
| --- | --- | --- |
| config.rs / contracts.rs | 設定、型、limits、error code | DB処理 |
| schema.rs / repository/*.rs | migration、SQL、transaction | inference、HTTP await |
| catalog.rs / authorization.rs | 登録、更新、grant、適格tool列挙 | LLMによる権限付与 |
| scenario.rs / extraction.rs | scenario/feedback抽出と検証 | 直接rule書込み |
| inference.rs / worker.rs | worker framing、上限、起動・終了 | 台帳書込み |
| retrieval.rs / ranking.rs | cosine、BM25、RRF、補正、順序 | backend実行 |
| feedback.rs / rules.rs | 訂正イベント、scope、撤回、適用 | tool descriptionからの指示受付 |
| references.rs / gateway.rs | 3入口、参照寿命、結果サイズ | host直接spawn |
| backends/llang.rs | 現行CapabilityServiceへの変換 | 新Wasm runtime |
| service.rs | 上記を結ぶ短い処理 | 巨大な全機能実装 |
| tests/*.rs | DB、HTTP provider、実runtime、反例 | 本番のテスト分岐 |

Pythonは `scripts/tool-selection/worker.py`、依存lock、モデル取得用 `prepare_models.py`。固定workerはアプリ起動中に一度起動し、requestごとにmodelをloadしない。

接続点: `runtime/turns.rs`でinput messageとruntime_runsを永続化した後、最初のprovider requestを構築する前に抽出・訂正処理を呼ぶ。`runtime/start_turn.rs`の入力到着直後にはまだ永続message IDがないため、そこへ直接feedback insertしない。`providers/stream/dispatch.rs`と`chat_completions`へ3入口を接続する。`AppState`、`test_state.rs`、`test_support.rs`、quality harnessの初期化を全て追う。

## 2. 設定・主体・スコープ

```json
{
  "formatVersion": 1,
  "mode": "discovery",
  "pythonPath": "/absolute/venv/bin/python",
  "modelManifestPath": "/absolute/models/manifest.json",
  "extraction": "configured-conversation-provider"
}
```

modeはdisabled/direct/discovery。未設定はdirect。不正設定はdiscoveryを無効化し診断を残す。全ツールを代わりに提示してはいけない。既存M2A directの8件上限は保持する。discoveryでは既存recall等の固定ツールを維持し、生成tool部分を3入口に置換する。

DBにローカルprofile UUIDが既にあるなら利用する。なければtool_selection設定namespaceに生成して永続化し、毎起動で変えない。credentialやOSユーザー名から推測しない。テストではP1/P2を注入する。RequestContextはホストだけが構築する:

```text
principal_id, conversation_id, run_id, input_message_id,
project_id: Option<ID>, task_id: Option<ID>, cancellation, deadline
```

project/task IDが存在しない場合、「案件A」という語だけから永続project scopeを作らない。当該conversationで暫定適用し、永続保存には案件の対応付けが必要と返す。明示的な全般指示（「今後いつも」）はuser scopeにできるが、operation/object条件は保持する。

既存L-Lang公開allowlistの権限をそのまま全profileへコピーしない。既定profileに対して既存設定の有効IDだけを明示grantへ一度移行する。1,500件のcatalog登録自体はgrantではない。新grantは開発用import/管理repository APIからのみ行い、LLMに管理toolを公開しない。

## 3. DB定義の実装指示

全table名は `tool_selection_` prefixを付け、既存world/生成catalogと衝突させない。時刻はINTEGERのUTC unix milliseconds、JSONはTEXT＋CHECK(json_valid(...))。IDはTEXT、state/enumはCHECK。以下でPK/FK/UQと記したものはDB制約にする。全書込みは既存SqliteWriter。FKはON。

| table末尾 | 列（特記なしTEXT）・制約 |
| --- | --- |
| meta | singleton INTEGER PK CHECK=1、catalog_epoch/acl_epoch/rule_epoch INTEGER NOT NULL初期0 |
| sources | id PK、kind CHECK(llang)、owner_principal、enabled INTEGER CHECK 0/1 |
| catalog | id PK、source_id FK sources、backend_key、current_revision_id nullable、enabled INTEGER、UQ(source_id,backend_key) |
| revisions | id PK、tool_id FK catalog、schema_hash、description_hash、input_schema_json、output_schema_json nullable、search_text、operations_json、objects_json、effect CHECK(pure/read/write/unknown)、backend_binding_json、created_at INTEGER、UQ(tool_id,id) |
| usage_pages | revision_id FK revisions、section CHECK(usage/examples/troubleshooting)、page INTEGER >=0、text、PK(revision_id,section,page) |
| grants | principal_id、tool_id FK catalog、scope_kind CHECK(user/project)、scope_id NOT NULL、PK(principal_id,tool_id,scope_kind,scope_id) |
| embeddings | revision_id FK revisions、model_hash、dimension INTEGER、vector BLOB、PK(revision_id,model_hash)、CHECK(length(vector)=4*dimension) |
| decisions | id PK、principal_id、conversation_id、run_id、message_id、scenario_json、catalog_epoch/acl_epoch/rule_epoch INTEGER、model_hash、status、created_at INTEGER |
| candidates | decision_id FK decisions ON DELETE CASCADE、revision_id FK revisions、lex_rank/vec_rank nullable INTEGER、raw_score nullable REAL、base_score/final_score REAL、rule_ids_json、final_rank INTEGER、PK(decision_id,revision_id) |
| invocations | id PK、decision_id FK decisions、revision_id FK revisions、backend_call_id nullable、technical_status CHECK(running/succeeded/failed/cancelled/unknown/interrupted)、satisfaction CHECK(unknown/explicit_positive/explicit_negative) DEFAULT unknown、started_at/finished_at INTEGER、error_code nullable |
| feedback | id PK、principal_id、message_id、decision_id nullable FK decisions、kind、evidence_json、proposal_json、status CHECK(pending/ambiguous/applied/rejected/revoked)、idempotency_key UQ、created_at INTEGER |
| rules | id PK、feedback_id FK feedback、principal_id、scope_kind CHECK(user/project/task/conversation)、scope_id NOT NULL、operation、object_type、phase nullable、input_kind nullable、source_constraint nullable、target_tool_id FK catalog、target_revision_id nullable、preferred_tool_id nullable FK catalog、action CHECK(avoid/prefer/pairwise/forbid)、strength REAL、expires_at INTEGER nullable、state CHECK(active/revoked/superseded)、created_at INTEGER |

catalog.current_revision_idは複合FK `(id,current_revision_id) → revisions(tool_id,id)`、DEFERRABLE INITIALLY DEFERRED。catalogをnull pointerで作成→revision insert→pointer更新を一transactionで行う。revisionの所属が違うpointerを拒否する。循環FKを理由に検証をアプリだけへ移さない。

索引: catalog(source_id,enabled)、grants(principal_id,tool_id)、decisions(principal_id,conversation_id,created_at)、rules(principal_id,state,scope_kind,scope_id)、feedback(message_id)、invocations(decision_id,started_at)。FTSは `fts5(revision_id UNINDEXED, search_text, tokenize='trigram')` の通常table。revision文書更新とFTS更新を同一transactionにする。

message_idは実messagesのIDを参照する。既存message削除時にselectionのdecision/feedback/ruleと派生cacheを削除するフックを追加する。既存の削除transactionと一体化し、アクセス不能になった原文を派生ruleで復活させない。異なるconversationへruleを移動して消去を回避しない。

catalog/usage/grant/ruleの書込みと対応epoch増加は同じtransaction。通常の決定/実行履歴insertではepochを上げない。migrationのschema version番号は開始時の最新値から採番し、今の21を盲目的に22へ書換えない。空DB、既存DB、二回実行、失敗rollbackを試験する。

## 4. ML workerとモデル配備

初期Embeddingは `intfloat/multilingual-e5-small`、rerankerは `BAAI/bge-reranker-v2-m3` を採用する。用途の合格はSAAAの評価datasetで判定する。モデル候補を実装者が際限なく比較する工程は作らない。CPUメモリや速度が目標に届かない場合も、黙って別モデルに差し替えない。

モデルはprepare_modelsで取得し、その時点の実commit SHA、全使用ファイルhash、tokenizer、package versionsをmanifest/lockへ記録する。アプリ起動時はローカルfilesだけをloadし、missingならdegraded。動的remote codeを実行しない。取得手順と推論は別コマンド。pip依存は専用venvで固定しシステムPythonへ混ぜない。

EmbeddingはSentenceTransformers、queryに `query: `、tool検索文書に `passage: ` を付け、L2正規化、float32 little endianで保存する。次元は実出力とmanifestを比較し、不一致なら拒否する。rerankerはTransformers sequence classificationのlogitを使う。query+documentのtoken上限512、batch8、eval/no_grad。dropoutと乱数seedを固定する。scoreを確率と呼ばない。

JSONL protocol（version=1）:

```json
{"version":1,"id":"uuid","op":"embed","kind":"query","texts":["過去の判断を調べる"]}
{"version":1,"id":"uuid","ok":true,"vectors":[[0.1,0.2]],"dimension":2,"modelHash":"実際のhash"}
{"version":1,"id":"uuid","op":"rerank","query":"...","documents":[{"id":"rev1","text":"..."}]}
{"version":1,"id":"uuid","ok":true,"scores":[{"id":"rev1","value":1.2}],"modelHash":"実際のhash"}
```

数値は形状例であり実モデル次元ではない。id/version/modelHash、件数、順序、finite値をRustで検査する。NaN、重複/欠落ID、無関係出力は拒否。一line上限2 MiB、stderrは内容をLLMへ返さず64 KiBまで保持。stdoutへdebug print禁止。

同時worker requestは1、待機最大8、超過はdegraded/busy。推論deadline5秒、初期load60秒、起動失敗を無限再試行しない。timeout/cancel時はworkerの処理が後続requestへ混入しないようkill/reapして次requestで再起動。これはツールbackendの実行枠とは別。アプリ終了時もworkerを回収する。

検索文書は固定順 `title / purpose / operations / objects / suitable / unsuitable / required inputs`、各項目の上限をschemaで固定し全体4 KiB。マニュアル全文をembeddingしない。モデル選定の根拠は[E5 model card](https://huggingface.co/intfloat/multilingual-e5-small)、[BGE model card](https://huggingface.co/BAAI/bge-reranker-v2-m3)。上記の512tokenやtimeoutはSAAAの設計値。

## 5. scenario抽出と自由文訂正の契約

抽出は既存会話providerへの独立したtoolなしrequest一回にまとめる。元の会話を別の新規サービスへ送らない。抽出入力は現在のuser message最大8 KiB、直近8件までのdecision要約、選択tool名、ホスト確定scope。tool出力本文や使い方は含めない。合計16 KiB以内、temperature=0、出力4 KiB上限、deadline5秒、retryなし。

意味ラベルは初期固定集合:

- operation: search/read/extract/summarize/compare/create/update/delete/send/execute/unknown
- object_type: decision_record/current_information/document/table/code/message/calendar_item/file/unknown
- phase: discover/inspect/transform/commit/unknown
- input_kind: text/file/url/structured/none/unknown

LLMが未知enumを返したらunknownへ明示的に正規化し、scopeの拡張根拠にはしない。文字列intentは残すので固定分類にないtoolもsemantic検索できる。D3の永続ruleはoperation/objectがknownの時だけ自動適用し、それ以外はambiguous。

抽出出力を以下に固定する。追加キーは拒否。offsetはUTF-8 byteの半開区間で、ホストが元user messageとの完全一致と文字境界を検証する。

```json
{
  "scenario":{"intent":"案件の過去の判断を確認","operation":"search","objectType":"decision_record","phase":"discover","inputKind":"text"},
  "feedback":[{
    "kind":"tool_choice",
    "decisionId":"入力で与えたIDのみ",
    "rejectedToolId":"入力で与えたIDまたはnull",
    "preferredToolId":"入力で与えたIDまたはnull",
    "scope":"project",
    "duration":"persistent",
    "evidence":{"start":0,"end":0,"text":"実際の原文span"},
    "condition":{"operation":"search","objectType":"decision_record","phase":null,"inputKind":null}
  }]
}
```

feedbackは最大4件。kindは上位計画の分類、durationはonce/persistent/unspecified。LLMの自己申告confidenceだけで採否を決めない。返された候補を、対象IDの所有者、原発言、ホストscope、条件不足で検証する。preferredの名前が直近候補にない場合は、同じsource内のcatalog完全一致で一意に解決できる時だけ許可。曖昧なら未解決として確認する。

元messageがquote/外部tool出力でなくuser roleであることを確認する。抽出器への固定指示は「訂正がなければfeedback=[]。沈黙/通常依頼を肯定と解釈しない。指示されたscope以外へ広げない。引数/出力の否定をtool否定へ変えない」。原文根拠があることはscope正解の十分条件ではないためheld-outでscope抽出も評価する。

抽出失敗: 通常scenarioはintent=current user text＋全enum unknownとしてdegraded検索可能。feedbackはpendingにして永続ruleを作らない。ユーザーへ記憶済みと返さない。無関係な通常会話は止めない。明示取消/撤回が検出されたが対象不明の場合はその場で短い確認を返す。

抽出はuser messageごとに一回、再試行runでは既存eventを利用する。検索呼出しのintentが変わった場合はscene hashを比較して再抽出するか、元scenarioのenumをunknownに戻す。別目的のintentに元の「過去の判断」ラベルを使い回さない。

## 6. 訂正の適用範囲と保存transaction

scopeの優先順はtask > conversation > project > user（task/projectの関係は条件をANDで維持）。より広い禁止は狭いpreferで解除しない。禁止解除は明示revokeだけ。project ruleはprincipal一致かつproject完全一致。taskはprincipal＋task＋project（存在時）、conversationはprincipal＋conversation、userはprincipalだけがscope条件。

- once: task IDありならtask終了まで、なければconversation＋24時間。
- unspecified: conversation＋24時間。毎回runで消さない。
- persistent: 明示「この案件は今後」ならproject、明示「今後いつも」ならuser。それ以外はconversation＋期限なしとして範囲を広げない。
- project IDがないのにscope=project: conversationの暫定ruleに縮小して保存した旨を伝える。project文字列を新規IDにしない。
- operation/objectがunknown: 永続適用不可、feedback=ambiguous。原発言だけ保存する。

同じeventのsemantic signatureはSHA-256(canonical JSONのmessage ID、decision ID、kind、対象tool、正規化condition)。これをidempotency_keyに使う。抽出器の言い換えで同一eventを加算しない。

保存手順を固定する:

1. writer transactionで対象message/decisionの所有scopeを再読取。
2. feedbackを冪等insert。既存なら前結果を返しepochを増やさない。
3. 検証済みproposalだけruleへ変換。同一target/condition/scopeの旧soft ruleはsupersededへ。
4. revokeは対象ruleをrevokedへ。feedbackの原文は通常撤回では残す。
5. 適用変更があった時だけrule_epochを+1しcommit。
6. commit成功後だけ短い「この条件で記憶した」応答を作る。rollback時は記憶済みと言わない。

利用者の削除要求は通常revokeと区別し、原文由来の派生情報も除去する。rule再生成workerや再索引で削除を復活させない。

## 7. 順位計算を一意にする

全同点はtool ID→revision IDの昇順。BM25は昇順、cosine/raw logitは降順。NaN/Infは当該推論batch失敗。RRFはrankが1始まりで `1/(60+lex_rank)+1/(60+vec_rank)`、欠けた枝は0。上位30＋明示prefer対象最大8でrerank対象最大38。

FTS queryは意図文字列を引用エスケープした語句として束ね、ユーザー文字列をMATCH式として直結しない。空queryはinvalid-input。trigramを作れない短文はvecのみ。認可を適用した集合の中でtop-Kを取る。全体top50を取ってからACLをかけて候補を空にしない。

cross encoder後の候補数Nに対し `base=1-(rank-1)/max(1,N-1)`。N=1は1。soft補正はexact scenario一致でstrength=1。operation/objectは完全一致必須、ruleが指定したphase/input/sourceも一致必須。自由文の意味だけで条件を広げる補正は初版では実装しない。Embeddingは候補検索とrerankに使い、訂正scopeの判定に使わない。

各targetの有効soft ruleは同一conditionごとに一つ、重複加算なし。補正はavoid=-0.25、prefer=+0.25、異なる条件が同時一致した合計を[-0.5,+0.5]へclamp。final=base+補正。結果scoreは確率ではなく[-0.5,1.5]になり得る。

pairwiseはpreferred→rejectedの有向辺。soft順で優先queueを作ったstable topological sortで適用する。cycleならまず同scope/同条件の古い明示ruleをsupersededにできるか検査し、できなければcycle内の辺を今回適用せずambiguityを返す。無関係候補の順位は安定させる。

no_matchはlogitの固定閾値を使う。validation setで、no_matchの誤受入≤5%を満たす範囲の最大Recallとなる閾値を選びmodel manifestへ保存する。held-outで再調整しない。rerankerなしのdegraded時はno_matchの確信を偽装せずstatus=degradedにする。基礎関連性のないtoolをpreferだけで実行候補へ昇格させない。

数値golden test: 3候補A/B/Cのbase=1/0.5/0。A avoidとB preferが一致すればfinal=0.75/0.75/0、同点はID順。B>Aの明示pairwiseがあればB/A/C。同じruleが二重届いても変わらない。条件不一致ではA/B/C。

## 8. 検索と訂正の競合、参照発行

searchは短い読取でeligible revisions＋scope内rules＋3epochをsnapshotし、DB lockを離して推論する。発行前にepochを再読取し、違えば最大1回だけ再検索。再度変わったらretryable selection-changedを返して参照を発行しない。rule変更前の結果が変更後に公開されることを防ぐ。

decisionと候補scoreの保存時もepochを同じtransactionで照合する。candidateRefは保存済みdecisionからのみ発行し、途中の推論結果を先にユーザーへ返さない。

executionRefにrule_epochも保持する。describe後に訂正/撤回が入った場合、invokeはselection-changedで再検索を求める。受付済みの実行をrule変更だけで自動停止しない。ユーザーが明示取消した場合は既存RunCancellationへ伝える。

認可/catalog更新はinvoke受付で再検査する。これらのepochチェックだけでTOCTOUを解消したと考えず、L-Langの最終受付でも元ResolvedCapabilityを検査する。tool IDから新revisionを取り直さない。

## 9. gateway入出力とbackendの固定

会話providerには名前互換性のため常に `tools_search / tools_describe / tools_invoke` を公開する。内部名はtools.search等へ固定変換。将来MCPでも同aliasを使ってよく、provider別に探索規約を変えない。

初版の検索schema:

```json
{"type":"object","properties":{"intent":{"type":"string"},"limit":{"type":"integer","minimum":1,"maximum":8}},"required":["intent"],"additionalProperties":false}
```

describeはcandidateRef必須、section既定contract、cursor任意。invokeはexecutionRef＋arguments(object)必須。上限やUTF-8チェックはRust側でも検証する。profile/project等の引数は拒否。

全gateway応答は `{"ok":true,"data":...}` または `{"ok":false,"error":{"code":"...","message":"固定の安全文","retryable":false}}`。error codeはinvalid-input/not-found/not-authorized/stale-reference/selection-changed/capacity/unavailable/timeout/cancelled/integrity/storage。存在情報を漏らすnot-found/not-authorizedは外向きには同じ文面。内部error詳細をLLMへ返さない。

ToolSelectionServiceはsearch/describe/invoke/ingest_feedbackを持つ。backendは検証済みtool revisionとJSON objectを受けるasync trait、結果は構造化JSON＋technical status。L-Lang adapterは既存GeneratedToolSnapshotを単一選択対象から作り、既存tools::executeへ渡す。originはconversationを保持し、内部call IDとselection invocation IDを紐付ける。台帳から生成packageを作り直さない。

初版L-Lang入力16 KiB、gateway入力64 KiB、schema16 KiB以下。contractが大きい場合はunsupported扱いでexecutionRefを発行しない。使用方法ページはsection/pageで取得し、opaque cursorをrun/revisionへbind。UTF-8の途中で切らない。

D0〜D3では結果16 KiB超のbackendを本番登録しない。試験backendで超過した場合はbounded output-limitエラーにする。上位計画の大結果継続参照はD4へ延期し、今回未実装なのにcursorを返さない。この限定で汎用artifact保存を追加しない。

## 10. 手作業で判断できる必須fixture

以下は実モデル精度試験とは別の決定的fixture。ranker doubleは固定scoreを返し、抽出器doubleはschema準拠proposalを返す。実モデルlaneでも同じ意味の入力を試験する。

| ID | 入力・事前状態 | 期待結果 |
| --- | --- | --- |
| G01 | P1/project A/search/decision_record、webとminutesが適格 | 基本順位web→minutes |
| G02 | 「この案件の過去の判断はWebでなく議事録で」＋G01のdecision | project A限定、minutes>web。次searchで即適用 |
| G03 | G02後、P1/project A/search/current_information | 補正0、基本順位を維持 |
| G04 | G02後、P1/project BまたはP2/project A | 補正0、ruleも漏れない |
| G05 | G02後project不明またはobject_type unknown | project rule不適用 |
| G06 | 同じ訂正を同message IDで2回投入 | feedback/rule数・rule_epochは一回分 |
| G07 | 「フォルダが違う」 | tool_choiceルール0、arguments feedbackのみ |
| G08 | 「違う」＋直近2call | ambiguous、永続減点0、確認応答 |
| G09 | 100件API成功、ユーザー無反応 | satisfactionは全unknown、positive event=0 |
| G10 | 「今回だけWebを使わない」＋task不明 | conversation＋24h。別conversationへ不適用 |
| G11 | G02を撤回 | revoked、epoch増加、次回は基本順位。履歴は維持 |
| G12 | minutesが未認可/停止 | preferenceがあっても候補・実行に出ない |
| G13 | search推論中にG02をcommit | 再検索1回、古いrule_epochの参照を発行しない |
| G14 | describe後訂正→invoke | selection-changed、backend起動0 |
| G15 | providerの抽出出力に別user decision ID | rejected、rule0 |
| G16 | 日本語spanのbyte境界不正 | rejected、記憶済み表示なし |
| G17 | DB保存失敗 | rollback、rule_epoch不変、記憶済み表示なし |
| G18 | rule原文message削除後に再起動 | 派生rule・検索cacheから復活しない |
| G19 | revise toolのschema後、旧executionRefを使用 | stale-reference、最新版へ転送なし |
| G20 | worker timeout/異常line/NaN | 降格または安全エラー、別requestへ結果混入なし |

## 11. 実装カードと完了条件

| カード | 編集範囲と手順 | 必須チェック |
| --- | --- | --- |
| T00 | 開始snapshot、現M2Aテスト、schema/turn接続箇所を記録 | generated_capabilities/providerの既存試験。既存失敗の切分け |
| T01 | contracts/config/RequestContext/errorを実装 | mode、path、上限、偽造scope拒否。まだprovider接続しない |
| T02 | schema/repository/FTS/migration | 空DB・upgrade・冪等・FK・rollback・削除。DB version競合なし |
| T03 | catalog登録・grant・usage import | 1,500件、同名別source、未認可を除いたtop-K、revision固定 |
| T04 | prepare_models/worker/framing | 実model配備、hash/shape/異常応答/終了。配備不能は明示 |
| T05 | retrieval/ranking純関数 | cosine、BM25符号、RRF、数値golden、stable sort |
| T06 | scenario/extraction/schema検証 | G07/G08/G15/G16。user message roleと永続IDを利用 |
| T07 | feedback/rules/transaction | G02〜G12/G17/G18。沈黙unknown、重複、撤回、条件境界 |
| T08 | search service/epoch再検査 | G13、no_match、degraded、実推論score保存 |
| T09 | references/describe/usageページ | TTL、run分離、上限、schema完全性、G14/G19 |
| T10 | L-Lang backend/invoke記録 | 実fixture true/false、取消、DB終端、枠再利用 |
| T11 | turn hook/provider 3入口接続 | 自然文訂正→保存→次turn順位→実callのE2E |
| T12 | dataset/精度/性能評価CLI | 固定split、実モデル、条件外波及、1,500/10,000件の計測 |
| R1 | 初回実装snapshot保存、差分＋新規全文レビュー | 下記反例、指摘file:line、修正前失敗の再現 |
| R2 | 修正・再検証・検証報告 | 必須契約違反を未解決にして完了宣言しない |

T03のimportは開発用JSON fixture loaderと内部管理APIまで。実運用から勝手に1,500件を収集しない。T11の前にfixtureツールを本番discoveryへ混入しないことを検査する。

## 12. 評価datasetと検証コマンド

新規 `src-tauri/tests/fixtures/tool-selection/` に catalog.jsonl、scenarios.jsonl、corrections.jsonl、split.jsonを作る。toolごとの適否を100件以上個別確認、残りは規模専用と明記。scenario120件以上＋correction60組＋no_match20件を最低数とする。

splitは開発60%、validation20%、held-out20%。scenario familyを分割単位としてseed固定、言い換えを別splitへ入れない。ラベル確定後にhashを保存する。実装結果を見て正解toolを書換えない。精度は単一率だけでなく分母/分子を必ず記す。

Recall@30は正解集合との平均再現率、Hit@5は適格scenario中top5に正解が一つ以上ある割合。閾値は上位計画の95%/90%。no_match誤受入率、訂正scope誤拡張、抽出ambiguity率も報告。誤拡張ゼロを全入力への保証と呼ばず、評価集合内の件数を示す。

新規CLI `scripts/tool-selection/evaluate.py` は固定dataset/manifestを引数に受け、JSON結果を指定outputへ書く。mock/liveを出力ヘッダへ記す。pipelineのRust処理は新規Rust example/CLIを呼び、評価script内に別ランキング実装を複製しない。

```sh
cargo test --locked --manifest-path src-tauri/Cargo.toml --lib tool_selection
cargo test --locked --manifest-path src-tauri/Cargo.toml --lib generated_capabilities
cargo test --locked --manifest-path src-tauri/Cargo.toml --lib providers
cargo fmt --check --manifest-path src-tauri/Cargo.toml
cargo clippy --locked --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
bun run size:check
bun run check
```

実モデルlaneには専用venvとmanifestを明示してevaluate.pyを実行する。自然文抽出のlive laneは既存設定providerで実施し、利用不可ならmockの会話結線試験とは別に未達と報告する。使用済みprovider設定以外を自動導入しない。

## 13. 自己レビューと担当AIへの最終指示

必ず次の反例を追う: (1)課題とは無関係な訂正の適用、(2)scope不明なのにuser全体へ保存、(3)自然文抽出がないSQL直書きだけの完成、(4)rerankerがmockのまま、(5)検索中訂正が古い参照に負ける、(6)API成功からpositive生成、(7)describe後schema変更で最新版へ転送、(8)別作業の変更混入。

提出先は `spec/docs/verification/tool-selection-d1-d3.md`。各TカードとG fixtureの実テスト名、実行結果、初回/修正後snapshot、推論manifest/hash、未達、検証コマンドを記す。対象外のMCP公開がないことを失敗扱いせず、D0〜D3の未達をMCP未着手と混同しない。

> この詳細手順を正本としてT00〜R2を順番に実施してください。最初にコードを読むだけで終わらず、各カードを実装・試験してください。新規DB migrationと内部APIは許可範囲です。世界モデルやD4以降の実装へ広げないでください。自然文訂正、条件境界、実モデル順位付け、実L-Lang呼出しを別々に検証し、mockの成功を実機能の完成と数えないでください。曖昧な訂正は勝手に一般化せずambiguousで扱ってください。自己レビューで問題を再現・修正・再検証してから報告してください。
