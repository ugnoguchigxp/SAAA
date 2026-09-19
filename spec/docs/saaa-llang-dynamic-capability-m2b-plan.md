# SAAA × L-Lang — M2B 大規模ツール選択・訂正記憶・MCP接続計画

更新日: 2026-09-20 / v2 / 実装指示

この文書は旧M2B「少数の生成能力を直接MCP公開する計画」を置き換える。今回は計画の改訂であり、以下の新規機能は未実装。上位文書は[初期計画](saaa-llang-dynamic-capability-initial-plan.md)、既存基盤は[M2A](saaa-llang-dynamic-capability-m2a-plan.md)。旧HTTP案は[transport参考資料](saaa-llang-dynamic-capability-m2b-transport-reference.md)へ移した。

実装担当者は先に[D0〜D3詳細実装手順](saaa-tool-selection-d0-d3-implementation-guide.md)を読むこと。型・DB制約・推論backend・条件判定・カード順は詳細手順を正本とする。初版は検索cacheを作らず、大結果の継続取得はD4へ延期する。

## 1. 新しい到達点

1,000件を超えるツールをSQLite台帳で管理し、その場の目的に合う候補だけを検索・順位付けする。schemaと使い方は選択後に取得する。ユーザーの訂正は根拠と適用条件を持つ記憶として保存し、その条件でだけ次回の順位を補正する。無反応を成功・満足として学習しない。

SAAAは外部MCPツールを利用するクライアントであると同時に、選択済み能力への疎結合な入口をMCPとして提供する。L-Lang生成能力と外部MCPツールは共通台帳で検索できるが、実行backendを分ける。外部ツールへWasmの保証を流用しない。

完成例:

1. 1,500件の台帳に対し「案件Aの過去の意思決定を確認」と依頼する。
2. 検索・順位付けにより少数候補を提示し、必要なschema・使い方だけ取得して実行する。
3. ユーザーが「この案件の過去の判断はWebではなく社内議事録から探して」と訂正する。
4. 訂正を元の選択に結び付けて保存する。次の該当場面では議事録を優先する。
5. 同じ案件でも「競合の最新情報」、別案件、別ユーザーではこの補正を適用しない。
6. ユーザーが何も言わなかった実行はfeedback=unknownのままにする。

## 2. 旧計画から変更する契約

| 項目 | v2の指定 |
| --- | --- |
| 台帳総数 | 8件制限を撤廃。受入試験1,500件、負荷試験10,000件 |
| 8件の意味 | 一回の検索でモデルへ返す候補の最大数。登録・認可件数の上限ではない |
| 常時公開 | tools.search / tools.describe / tools.invokeの3入口だけ |
| SystemContext | 3入口の利用手順と制約だけ。全ツールの説明・schema・訂正履歴を入れない |
| SQLite | 台帳・使い方・索引・選択履歴・訂正記憶の正本 |
| ランキング | BM25＋Embedding＋RRF＋Cross-Encoder、その後に条件付き訂正補正 |
| 学習信号 | 明示訂正・明示肯定と、技術的実行結果を別々に保存。沈黙は未評価 |
| 実行 | 短命なrevision固定参照をbackend adapterが解決。名前から最新版へ転送しない |
| MCP tools/list | 3入口だけを返す。1,500件の直接定義を列挙しない |

M2Aの最大8件設定を単に1,500へ増やしてはいけない。既存の直接公開モードは互換用途として残し、新規discoveryモードでは3入口へ切り替える。同じrequestに両方式を混在させない。

## 3. 実装段階と最初の依頼範囲

今回は設計全体を固定するが、担当AIへの最初の実装依頼は **D0〜D3＋その自己レビュー** とする。1000件超の検索と訂正反映を会話内で実証してから、D4/D5のMCP接続へ進む。HTTP公開だけを先行させない。

| 段階 | 成果物 | 次へ進む条件 |
| --- | --- | --- |
| D0 | 開始snapshot、M2A回帰確認、モデル/索引契約、評価データ | 既存失敗と新規作業を区別できる |
| D1 | SQLite台帳、backend中立schema、使い方、参照管理 | 1,500件を永続化し3入口で必要情報だけ取得 |
| D2 | 状況表現、ハイブリッド検索、ML rerank | 実モデルで独立評価基準を満たす |
| D3 | 発言からの訂正抽出、条件付き記憶、即時補正・撤回 | 同条件で改善し、非該当条件へ波及しない |
| D4 | 外部MCP server登録・tools/list同期・呼出adapter | 名前衝突、revision変更、権限、切断を処理 |
| D5 | 3入口のローカルMCP公開 | 会話と同じ台帳・選択・実行経路を使用 |
| D6（後続） | 訂正履歴からの順位学習 | データと評価が整ってから着手。今回は実装しない |

D1〜D3ではL-Lang backendと試験用backendを使い、外部MCPとしての動作を完成と報告しない。D4で設定済み実MCP serverへの疎通を別途実施する。UI、L-Lang新ABI、LLMコード生成、world-modelの大改修は対象外。

## 4. 既存コードとの接続と変更範囲

新規モジュールは `src-tauri/src/tool_selection/` を推奨する。catalog、scenario、retrieval、ranking、feedback、references、gateway、backends、testsに責務を分ける。ファイル名は提案だが責務分離は必須。

- M2Aのpublication.rs/tools.rs、GeneratedToolSnapshot、CapabilityService、取消・DB終端処理を読む。L-Lang backendは既存invokeを再利用する。
- 会話接続は既存providers/stream/dispatch.rsとchat_completions経路を拡張し、別会話runtimeを作らない。
- SQLite書込みは既存SqliteWriterへ統合する。今回の新規table/索引migrationは範囲内。別の正本DBを作らない。
- personal-world-modelは別作業。必要なproject/task IDは既存で確定した値を受け取る。未完成の世界モデルを必須依存にしない。
- 未コミット・未追跡変更があるため開始コピーとhashをrepository外に保存する。reset/clean/stashで消さない。
- 新規推論依存はD0で必要性・version・license・ローカル実行可否を記録して選定する。モデルを勝手にruntime downloadする実装は作らない。

## 5. SQLiteデータ契約

下記は論理table。既存tableで同じ責務を満たす場合は再利用し、migrationとFK/unique/indexを明示する。

| table | 必須情報・不変条件 |
| --- | --- |
| tool_sources | source ID、backend種別、接続設定参照、owner、状態。credential本文を台帳へ入れない |
| tool_catalog | stable tool ID、source ID、backend名、current revision、enabled。source内名でunique |
| tool_revisions | immutable revision ID、schema/description hash、input/output schema、effect、更新時刻。L-Lang revisionへの参照も保持 |
| tool_usage | revision、locale、用途、適した条件、適さない条件、前提、引数例、失敗時対処、由来。raw外部説明と正規化結果を区別 |
| tool_access | principal/group/projectとtool/sourceの認可。検索結果と実行の両方で検査 |
| tool_search_index | FTS5検索文書とrevision、embedding model/version/dimension、vector、索引generation |
| tool_decisions | decision ID、principal、確定scope、scenario、候補revision/各段階score、選択、索引/model/rule version、時刻 |
| tool_invocations | invocation ID、decision ID、tool revision、backend call ID、technical status、時間、目的達成の観測を別フィールドで保存 |
| tool_feedback_events | user message ID、関連decision/invocation、発言の根拠範囲、種別、scope解釈、confidence、日時。原イベントはimmutable |
| tool_selection_rules | feedback由来、適用条件、対象tool/revision、avoid/prefer/pairwise、強度、scope、validity、active/revoked/superseded |

user satisfactionはunknown/explicit_positive/explicit_negativeで持つ。technical status=succeededでもunknownのまま。後日訂正を受けたらfeedbackイベントを追加し、元の選択履歴を書き換えない。同じmessage＋対象への抽出再試行は冪等にする。

vectorとFTS文書はrevisionに一致するものだけ検索する。tool更新では新revisionの索引を作成し、完成後に切替。不完全な索引で古い使い方を新revisionへ結び付けない。1,500件ではまず全候補vectorの厳密cosine検索を使い、ANN/HNSW導入は計測後とする。vectorはSQLite BLOBで永続化し、メモリcacheはgenerationで無効化する。

## 6. 3入口の具体契約

名前は会話/MCPとも `tools.search`、`tools.describe`、`tools.invoke`。使用providerが名前形式を制限する場合は固定aliasをadapterで用意し、機能を分岐しない。

### tools.search

入力: `{intent, limit?}`。limitは1〜8、既定5。intentはUTF-8で4 KiB以下。principal、project、conversation、task、権限はホストから注入し、モデル引数の自己申告を信用しない。

出力: `{decisionId, status, candidates:[{candidateRef, title, summary, reason}], continuation?}`。schema・長い使い方は含めない。候補全体12 KiB以内、summaryは候補ごと512バイト以内。statusはok/no_match/degraded。切詰め時は明示し、候補件数・順序を記録する。全候補の機密な名前や未認可件数を漏らさない。

検索は「このツールを必ず実行する」という決定ではない。候補が曖昧なら追加条件の取得、no_matchなら再検索またはユーザーへの確認を選べる。

### tools.describe

入力: `{candidateRef, section?, cursor?}`。sectionはcontract/usage/examples/troubleshooting、既定contract。SQLiteから選択revisionの必要sectionだけを返す。

出力: revision、使い方、完全なinput schema（contract時）、executionRef、続きのcursor。1応答16 KiB以内。schemaは途中切断せず、単体上限を超えるものはunsupported_contractとして実行不可にする。usageのページはrevision固定cursorで取得する。外部の使い方はデータとして扱い、system指示へ昇格しない。

### tools.invoke

入力: `{executionRef, arguments}`。gatewayのargumentsはJSON objectで、L-Lang専用boolean schemaには固定しない。実際のrevision schemaでbackend実行前に検証する。L-Langは従来のboolean subset制約を維持する。

candidateRef/executionRefはホストが生成したopaque参照で、principal＋会話runまたはMCP session＋decision＋tool revision＋contract hash＋認可versionへ紐付ける。有効期限10分、終了したrun/sessionの参照は失効。ユーザーが別の名前・revisionを注入して呼ぶ経路は作らない。invoke時に認可・enabled・revisionを再検査し、古ければ再検索を要求する。

入力はgatewayで64 KiB上限、backendによりさらに縮小。LLMへ返す結果は16 KiB上限とし、超過時は明示的なtruncated結果と制限内の継続参照を返す。schema validationを文字数切断で回避しない。外部実行の再試行は勝手に行わない。副作用のある不確定結果はunknownとして記録する。

検索参照は最大64件/runまたはsession。上限時は明示エラー。backend結果の保存上限・TTLをD1で固定し、無制限保存しない。権限変更時は参照・cacheを失効させる。

## 7. 状況表現と検索・ML順位付け

Scenarioは `{intent, operation, object_type, task_phase, available_input_kinds, source_constraints, project_id?, task_id?, confidence, provenance}`。principal/scopeはホスト確定値。意味的項目は会話から構造化抽出する。会話全文を毎回embeddingへ流さない。

D0で日本語・英語・混在入力を含むEmbedding/Cross-Encoderを選定する。固定model ID/revision/tokenizer/hash/dimension/最大長/前処理をmanifestへ記録し、実モデルで評価する。具体modelが未確定のままD2を完了にしない。mockスコアは配線試験専用で、MLの実証には数えない。推論はローカルを既定とし、認証情報や履歴の外部送信を新規に導入しない。

処理順:

1. 認可、source接続、enabled、入力種別・effect制約などのhard filter。
2. FTS5 BM25上位50件＋Embedding cosine上位50件を取得。
3. tool ID/revisionで重複除去し、RRF `sum(1/(60+rank))`で上位30件へ。
4. 該当する明示preferルールの対象が候補から落ちていれば、認可検査後最大8件まで追加。avoidは検索候補を消さず後段で補正。
5. Cross-Encoderでscenarioと各候補の用途・適否条件を比較する。入力の上限・切詰め方を固定する。
6. 条件付き訂正を適用し、最終上位5件（要求時最大8件）を返す。

日本語FTSは空白分割だけにしない。採用SQLiteのtrigram tokenizer、または同じ形態素処理を索引/queryの両側で使う。採用方式をD0で固定し、短い日本語queryはembedding側も使う。BM25は低い値が良いので符号・順位を取り違えない。

Cross-Encoderのraw scoreを確率と呼ばない。D0のvalidation setで固定した閾値によりno_matchを判断する。訂正補正用base scoreはrerank順位を0〜1へ正規化して使い、raw scoreの尺度差を持ち込まない。reranker停止時はhybrid順位＋訂正へ降格しstatus=degraded、Embedding停止時はFTS＋訂正へ降格。台帳や認可が読めない場合は実行候補を返さない。

検索cacheはprincipal/scope、scenario、索引generation、model version、認可version、rule versionをキーに含める。訂正・撤回は次の検索から必ず効く。検索失敗時に「前の別シナリオの結果」で代用しない。

## 8. ユーザー訂正の取り込み

通常のユーザー発言を既存会話入力経路で処理し、ランキング訂正か、引数訂正か、出力への不満かを分類する。feedback専用フォームを必須にしない。モデルが自分の判断をユーザーの訂正として登録できる公開toolは作らない。

分類: tool_choice / arguments / output_quality / source_scope / temporary_constraint / explicit_positive / revoke / ambiguous。自然文抽出器は根拠message IDと原文span、対象decision ID、適用条件を返す。ホストがその発言がユーザー由来であり、対象decisionが同じprincipal/scopeに属することを検証する。

複数toolを実行していた場合は名前・目的・結果への言及から対象を絞る。対象が特定できない「違う」はambiguousとして保存し、永続減点を作らない。必要時は「ツール自体と検索条件のどちらが違いましたか」と短く確認する。無関係な独立作業は続けられる。

| 発言 | 保存・反映 |
| --- | --- |
| 「この案件の過去の判断は議事録から」 | project＋operation/object条件付きで議事録prefer、必要なら元tool avoid |
| 「フォルダが違う」 | arguments/source_scope。ツール全体の順位は下げない |
| 「今回は送信しない」 | 当該taskだけの操作制約。恒久的な全案件ルールにしない |
| 「AではなくB」 | 条件付きB>A。Bが未認可・停止なら実行せず、他候補または確認へ |
| 「違う」だけ | ambiguous。原発言保存、適用範囲は未確定 |
| 無反応 | unknown。肯定イベントも報酬+1も作らない |
| 「さっきの指定は撤回」 | 元ruleをrevokedにしcacheを失効。イベントは履歴として残す |

明示された継続指示は1回で記憶する。同じ指摘を複数回要求しない。「今回」「この案件」「いつも」を区別する。期間が不明な訂正は当該task内へ限定し、勝手にユーザー全体へ一般化しない。継続scopeが明確なら再確認不要。「案件Aの過去の意思決定では議事録を優先します」のように適用範囲を短く返す。

明示訂正は撤回・上書き・条件消滅まで保持し、時が経っただけで忘れて同じ誤りを繰り返さない。推定ルールはtask終了または24時間で失効し、永続昇格には明示根拠を必要とする。削除要求では原文・派生索引・cacheも既存削除方針に従って除去する。

## 9. 条件一致とランキング補正

scope一致を先に判定する。principal、project、task、指定source、入力種別等の明示条件はSQL/構造化述語で一致させる。必須条件がunknownなら適用しない。その内側でoperation/object/phaseの意味類似を使う。Embedding類似だけで別案件・別ユーザーへルールを適用しない。

初期方式は検索可能な訂正事例に基づく局所補正（case-based reranking）。モデル重みをその場で更新しない。判断理由、撤回、revision追跡を可能にする。

- soft avoid/preferは `base_score ± weight × match_strength`。初期weight=0.25、同一対象へのsoft補正合計は±0.5でcapする。値は評価前に固定する。
- 明示されたB>Aは両方が適格候補にある場合の相対順序制約とする。候補にないBの注入は7節で行う。
- 明示的な禁止・source限定はsoft減点でなくhard filterへ渡す。
- 同じeventの重複、言い換えによる重複ルールを回数として加算しない。
- 矛盾時は狭いscope、明示根拠、同じ条件内の新しい訂正を優先する。矛盾が残る場合は適用を保留し確認する。pairwise cycleも黙って並べ替えない。
- 好みはstable tool IDへ、特定versionの不具合はrevisionへ結び付ける。revision更新だけで継続的な好みを失わせず、不具合ペナルティを無条件に新revisionへ継承しない。

最終履歴にはbase順位、適用rule ID、match条件、補正値、最終順位を残す。ユーザーは「なぜこれを選んだか」を後から説明してもらえる。内部の長い推論文を保存する必要はない。

将来D6では「同じ条件ならB>A」の明示訂正をpairwise学習に使う。Aだけが否定された場合に、未選択Bを正解として捏造しない。無反応や未選択候補にreward=0/1を付けない。訂正が来るケース自体にも偏りがあるため、このデータだけから全体満足率を推定しない。Banditを導入する場合は観測報酬と欠測を分け、選択確率を記録し、過去の決定的ログから不偏評価できると主張しない。

## 10. 外部MCPと公開gateway（D4/D5）

MCP serverのtools/listを全ページ取得して台帳へ同期する。source ID＋tool名で名前衝突を避ける。schema/使い方変更は新revision、一覧から消えたtoolはtombstoneにする。部分失敗時は同期generationを切り替えず、古い台帳のfreshnessを表示する。接続不能toolを選んで初めて判明する状態を減らす。

登録・認可はホスト管理操作。modelのsearchや外部tool説明が勝手に接続先・権限を追加できない。外部backendはMCP固有input schema、content、取消、timeoutを扱う。network切断時の結果不明をsucceeded扱いせず、書込toolを自動再試行しない。

公開gatewayは2025-06-18のローカルStreamable HTTP/JSON応答、Bearer認証、127.0.0.1限定、Origin拒否を旧案から引き継ぐ。tools/listは3入口だけ。search→describe→invokeの参照がMCP sessionに結び付く。セッションで1,500件のofferを保持しない。

HTTP切断だけを明示取消として扱わない。gateway taskがdeadlineまで所有し、cancel通知/DELETE/終了で明示取消・回収する。会話の呼出元abortは既存adapterの取消規約に従う。transport参考資料のHTTP status/初期化/セッション上限を採用するが、直接gc_名の公開契約は採用しない。

## 11. 必須試験と評価基準

D0で1,500件の再現可能な台帳を用意する。少なくとも100件は用途・引数・適否が異なる手作業確認済みtool cardとし、残りは規模試験用に区別する。単なる同名連番だけで検索精度を評価しない。日本語/英語/混在・似た名前・同名別serverを含める。

正解tool集合を持つシナリオ120件以上、訂正と境界反例の組60組以上、no_match20件以上を用意する。調整用とheld-outを分け、同じtemplate/projectの言い換えを両方へ漏らさない。生成した正解ラベルは人手または独立レビューで確認する。

| ID | 必須試験 |
| --- | --- |
| C01 | 1,500件登録後も常時tool定義は3件。SystemContextに台帳本文がない |
| C02 | describeで選択revisionのschema/usageだけ取得。paginationと上限を検査 |
| C03 | 権限外toolは検索・describe・invokeの全経路で拒否。偽造scope/参照も拒否 |
| C04 | 同名別source、更新、停止、期限切れ、schema変更で誤backend実行なし |
| R01 | BM25/embeddingの候補統合、RRF、Cross-Encoderが実モデルで動く。mockのみは不可 |
| R02 | held-out Recall@30 ≥95%、適格シナリオの正解Hit@5 ≥90%。達しなければ原因と未達を報告 |
| R03 | no_matchと曖昧な入力で無関係toolを強制実行しない |
| F01 | 明示訂正1回で次の一致scenarioの順位が変わる |
| F02 | 同一projectでも別operation/objectでは適用せず、別project/userへも漏れない |
| F03 | 必須条件unknownならrule不適用。単なる意味類似でscopeを越えない |
| F04 | 無反応100回でもpositive feedback件数・満足度ラベルは増えない |
| F05 | 引数ミス・出力不満をtool選択の否定へ誤変換しない |
| F06 | 複数call後の訂正が正しいdecisionに結び付く。不明ならambiguous |
| F07 | 今回だけの指定、永続指定、撤回、反対の訂正、重複イベントを処理 |
| F08 | correction直後のcacheが失効。再起動後もrule・scopeが復元される |
| F09 | 明示B>AでBを候補へ回収。ただし認可・停止を越えない |
| F10 | 自然なユーザー発言→抽出→検証→保存→次の検索までの統合試験。SQL直insertだけでは不可 |
| E01 | 実L-Lang invokeでrevision固定、取消、DB終端、枠再利用を維持 |
| E02 | 外部MCPの複数page同期・部分失敗・tool消失・同名衝突・不確定結果を確認 |
| E03 | MCP越しの3入口、session分離、HTTP切断、明示cancelと終了を実HTTPで検証 |

訂正境界の決定的fixtureでは条件外の順位変化ゼロを必須とする。抽出器を含むheld-out評価では訂正適用精度・適用漏れ・範囲誤拡張率を別々に報告し、scope誤拡張が残るケースはambiguous扱いへ戻す。raw再ランキングと訂正適用後の評価を分ける。

性能は1,500/10,000件でp50/p95、cold/warm、hardware、model、LLM提示bytesを記録する。初期目標はwarm検索・rerank p95≤1秒（scenario抽出のLLM時間は別記）、describe p95≤50ms。機種依存目標を満たせない場合はbackend別内訳と改善案を示し、実測を捏造しない。正確性を落として速度合格にしない。

## 12. 自己レビュー、検証、提出

各段階で開始時→初回実装→自己修正後のsnapshotを分ける。Markdownを品質点の代用にしない。未追跡fileを含む差分と新規全文を読む。

追跡する経路は2本: (a) search→describe→認可再検査→backend→終端、(b) user訂正→対象decision→scope→rule→cache失効→次回順位。特に「無反応の正解化」「一回の訂正の全体化」「API成功の満足度化」「使い方のsystem指示化」を反例で確認する。

既存M2Aと新規tool_selectionのtargeted試験、cargo fmt、clippy、size、bun run checkを実行する。実ML評価は固定manifestと評価datasetで別コマンドを用意し、seed・version・分割・結果を保存する。モデル未配備なら配線試験と区別し、D2完了を宣言しない。

報告は `spec/docs/verification/tool-selection-d1-d3.md`、D4/D5は別報告。完了段階、DB migration、公開契約、fixture/実モデルの区別、精度/速度、訂正の境界事例、自己レビューと修正前後、未達を記す。点数を自己申告して保証しない。

## 13. 最初に担当AIへ渡す指示

> 本計画v2のD0〜D3を実装してください。M2Aの8件直接公開を大量化するのではなく、1000件超のSQLite台帳と3入口を作り、状況別検索・ML順位付け・条件付き訂正記憶を会話経路で実証してください。通常のユーザー訂正から対象decisionとscopeを特定し、一致条件でだけ順位へ反映してください。沈黙を成功ラベルにせず、引数ミスとtool選択ミスを分けてください。初回実装snapshotを保存して自己レビューし、反例を再現・修正・再検証してください。既存の別作業を保護し、D4/D5やD6へ無断で進まず、今回の完了基準と未達を正確に報告してください。

## 14. 技術的な参照資料

検索・再ランキングの構成は[Sentence Transformers](https://www.sbert.net/examples/sentence_transformer/applications/retrieve_rerank/README.html)、全文検索は[SQLite FTS5](https://www.sqlite.org/fts5.html)を参照する。観測された反応の偏りは[Microsoft Research](https://www.microsoft.com/en-us/research/publication/unbiased-learning-rank-biased-feedback/)、Banditの部分観測は[Vowpal Wabbit](https://vowpalwabbit.org/docs/vowpal_wabbit/python/9.0.1/tutorials/python_Contextual_bandits_and_Vowpal_Wabbit.html)を参照する。本書の数値上限・段階・訂正保存規約はSAAAの設計指定であり、これらの資料の推奨値ではない。
