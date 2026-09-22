# ContextWindow・入力記録・メモリーの統合方針

状態: 設計案。2026-09-22。外部レビューへの回答版に実装計画の確定事項を追記。実装済みの仕様ではない。実装手順は [実装計画](context-window-records-memory-implementation-plan.md)、担当AI向けの前提は [引き継ぎ文書](context-window-records-memory-handoff.md) を参照。本書と実装計画が矛盾する場合は実装計画の §0-2/§0-3 を優先する。

目的は、現在の情報で何が判断でき、何が不足し、どの記録・メモリー・Worldを参照すべきか判断できるContextWindowを作ることである。入力キャッシュはこの能力を維持しながら費用と待ち時間を抑える手段とする。特定モデルやAPIの採用は前提としない。

一度取り込んだ入力はSQLiteへ保存する。Contextからの除外と原典の削除を別操作にする。Memory OFFでも原典保存・活動参照・Context継続は動作する。OFFはPersonal State/Worldの自動抽出・注入を制御し、記録機能を無効にしない。

今回確定するのは設計契約と初期設定値であり、DB migration、Provider変更、外部API呼出しは行わない。既存の原文正本、Scope、依存検証、忘却journalは継承する。

## レビューの採否と補正

三領域、Segment、共通記録ID、記憶参照の四つの論理操作、usage計測、忘却の波及を採用する。一般Toolの発見・詳細取得・実行は既存のSQLite Tool Selectionを使用する。メモリー/Worldの高頻度参照入口は常設してよいが、四つの論理操作すべてを新しい常設Toolとして追加する決定ではない。以下は補正事項。

- 再レンダリングというCPU処理そのものではなく、送信prefixの内容・順序・フレーミングの変化が問題。同じ表現を決定的に生成してもよいが、本案では固定部分の保存済みrenderingを再利用して保証を単純化する。digestだけでは内容を復元できない。
- TTL経過は再構成の強制条件にしない。Providerの保持保証・更新条件は異なり、cache missも時刻だけでは確定しない。再構成には読取・要約・再書込の費用があるため「無料」と扱わない。
- 末尾の置換と、Providerの会話全体を厳密なappend-onlyにすることは別。思考blockやsessionが履歴へ拘束される経路は専用契約が必要。
- 引き継ぎをPersonal State ledgerだけから生成しない。Memory OFFでも、入力記録・Runtime状態・明示的な決定を引き継げるようにする。
- 同一検索の再利用にはScope・鮮度・snapshot・失効検証が必要。再検索を無条件に禁止しない。
- 64 KiB超という理由だけで抽出推論を自動実行しない。読取目的、原典の形式、許可された費用予算で選ぶ。
- 日本語がunicode61で一切検索できないという断定は採用しない。現行会話検索は既にtrigram。新しい記録検索にもtrigramを用い、3文字未満は別経路とする。
- Scopeは単一列だけに縮約しない。複数Scopeの関係を保持し、認可条件を件数制限より前にSQLへ適用する。
- 原典へ既存のredact_runtime_textを適用しない。同関数は診断用に2,000文字へ制限するため、全入力保存を破壊する。

### Toolシステムの既存設計を前提にする

1,000件超のToolのschema/description/使い方を全件Contextへ載せない。現行のdiscovery経路はSQLiteのcatalog/revisions/usage_pages/grants/embeddings/decisions/rulesを用い、LLMには `tools_search → tools_describe → tools_invoke` を提示する。

- tools_searchはintentから少数の候補とcandidateRefを返す。既定5件、最大8件。検索はFTS・embeddingの候補統合、reranker、条件付きのユーザー訂正ルールを利用し、利用不能時には既存の縮退規則を使う。
- tools_describeは候補のcontract/usage/examples/troubleshootingを必要な分だけ返し、executionRefを発行する。個別schemaと使い方はこの応答で取得し、Providerのトップレベルtoolsへ全件展開しない。
- tools_invokeはexecutionRefとargumentsを受け、Scope、認可、revision/schema、epoch等を再検証して実行する。検索順位も保存済みの実行経験も、実行権限にはならない。
- 大きいMCP結果には既にresultRefとtools_describe(resultRef,page)による続きの読取がある。原典読取を設計する際にこの経路を重複実装しない。

現行available_agent_toolsはdiscovery時にgeneratedの直接定義を3入口へ置き換える一方、会話recall・ContextStill・voice・coding等の直接Toolも別途提示する。この例外集合を確認せず「現在すべてのToolが3入口だけ」とは扱わない。legacy direct経路も独立に扱う。

Contextに必要なのは、目的・対象・制約・不足情報から適切なTool検索intentを作れること、候補を比較できること、選んだToolの契約と使い方を理解できることである。未選択の全Tool知識を常駐させることではない。

### 利用判断から検索へ進む契約

メモリー/Worldを参照するための小さな高頻度入口は、discoveryの3入口とともに常設可能とする。そこから得た情報、現在の依頼、作業状態を用いて、LLMが一般Toolの必要性を判断する。必要と判断した時点で必ずtools_searchを呼び、候補を選び、tools_describeで必要な契約/使い方を確認してtools_invokeへ進む。

過去の成功例、覚えているTool名、以前のschema、古いexecutionRefを根拠に検索を省略しない。現在の操作のためにsearch/describe済みで、同じ実行系列を継続する場合に限り、有効な参照を既存規則の範囲で再利用できる。新しい操作の選択や失効後の再選択では再びsearchから始める。

固定policyには「常設のメモリー/World参照を使って必要情報を確認し、一般Toolが必要ならtools_searchで利用可能な候補と使い方を探す」という判断規則を置く。これは毎ターン無条件に検索する規則でも、メモリー/Worldの全内容を常駐させる規則でもない。実行側は引き続きsearch→describeで発行された参照を検証する。

## A. Contextの領域・予算・権威

配置は `固定prefix F → 追記領域 L → 当generationの動態領域 D → 現在の依頼 U → Providerが要求するTool往復` を基本とする。Tool follow-upの具体的な並びはadapterがProvider契約に従って生成する。

FとLは一定区間で不変・追記とし、Dは毎generationで更新する。L内の過去発言・Web情報は現在の命令へ昇格させない。外部本文やLLMの自己生成仮説をsystem authorityへ配置しない。

### 初期予算

`B`はadapterが送信可能と判断した、最終直列化リクエストのbyte予算。JSONのescape、Tool schema、wrapper、全入力を含む。出力枠と安全余裕はB決定前に確保し、二重控除しない。現行互換経路の開始値は64,000 bytes。token上限が分かる経路はbyte検査とtoken検査の両方を満たすこと。byte上限だけで任意モデルの容量を保証しない。

次表は初期の領域上限。合算して全枠を予約するものではなく、最終合計は常にB以下とする。小さいBでは下記割合上限も適用する。

| 内容 | 領域 | 必要度 | 生成主体 | 初期最大bytes |
| --- | --- | --- | --- | --- |
| 固定policy・identity・3つのdiscovery入口・高頻度のメモリー/World参照入口・最小bootstrap定義 | F | Must | Runtime、入口の版固定。個別業務Toolの全catalogは含めない | min(12,288, B×25%) |
| 再構成時の引き継ぎ | L先頭 | 適用制約・未完了状態はMust、その他Should | Runtime。派生要約を含む場合は明示 | min(8,192, B×15%) |
| 過去の会話、完了したTool往復の簡潔な記録、根拠ref | L | 原則Should。継続中の実行はMust | 保存済み記録のrenderer | BからF/D/U/一時読取/wrapperを引いた残量 |
| 時刻・Scope/epoch・現在の制約・実行状態・Tool残回数・失効・省略通知 | D | Must | Runtime | 合計min(8,192, B×20%) |
| inputOrigin/presentationMode | D | Must | Runtime | 上記D枠に含む |
| 作業仮説・未確認・矛盾・直近活動要約 | D | Should。ただし判断対象の不確実性はMust | Runtimeまたは根拠付き派生記録 | 合計2,048、D全体の追加枠はmin(4,096, B×10%) |
| 選択したPersonal State/World | D | 適用制約はMust、関連事実・関係はShould/May | 確定snapshotのprojection | 上記D枠内。全件注入禁止 |
| 現在の依頼U | U | Must | 入力記録 | B−F−他のMust−wrapper。無断切り詰め禁止 |
| 大きな原典の一時読取 | DまたはToolの一時本文 | 通常Should、必要範囲指定時は当回Must | 原典の範囲読取 | min(8,192, B×20%)/generation |
| Tool検索候補・選択した契約/使い方 | Tool応答、必要期間のみL/D | 候補はShould、実行予定の契約は当回Must | 既存Tool Selectionの保存済みrevision | searchは既存12,288、describeは既存16,384 bytes以下。B内に別途収容 |

F/D/引き継ぎのMustが上限を超えた場合は黙って落とさず、安定したreason codeで再構成または停止する。不要な履歴・Shouldを減らした後でもMust総量がBへ入らなければRED。単にActive/Candidateという状態で全項目をMustへ昇格させない。

Scope適用はRuntimeが判定する。DのScope/epochはモデルの説明補助であり、認可の根拠ではない。現在時刻はUTCと表示timezoneをgeneration snapshotから提示し、期限切れ自体の排除もRuntimeが行う。

各事実は `user_statement / external_observation / runtime_state / derived_claim` を区別する。external_observationは外部にそう書かれていたという意味であり、内容の真実性を保証しない。LLMが出した未確認事項はderived_claimとして引き継ぐ。

省略通知は `reason, omitted_count|null, available_via, earliest_ref|null, index_coverage` を持つ。権限外情報の存在・件数は通知しない。Fにはdiscovery入口と常設する高頻度メモリー/World参照入口の短い定義を置き、その他の個別Toolのschema/利用手順/例はcatalogとusage_pagesからdescribeで取得する。system側には信頼境界と必須の選択規則だけ残す。取得した契約がBへ収まらない場合は任意履歴を整理するか再計画し、schemaを切り詰めたまま実行させない。

## B. Segmentと再構成

SegmentはSAAAが管理する固定prefixと追記記録の世代であり、Provider側のcacheの存在を意味しない。

### Segment内の追記

追記単位はimmutableな `context_entry`。順序番号、record_id、役割、rendered_blob_id、serializer_version、content digest、依存refを持つ。既存entryを再レンダリング・編集しない。

次ターンには前ターンの会話と完了したTool往復を追加するが、D全体をLへコピーしない。採用した事実・決定・必要な応答・活動refだけを記録から追加する。大きな一時本文はLに入れない。部分応答・取消・Tool errorも状態を付け、成功した結果に変換しない。

この方式は `F+L` のprefixを維持する。前回のD/Uまで含むリクエスト全体のprefix一致は保証しない。その差を計測に残す。

### Provider整合

- `rebuildable`経路: F/L/Dを独立に再構成可能。一時本文を次回除外し、F/Lを維持する。
- `history_bound`経路: 思考block・Tool block・remote sessionが以前の本文へ依存する場合、勝手に本文を削除・置換しない。Providerが検証済みの編集/compactionを提供するならそれを用いる。未対応なら、新Segmentと新session/適法な引き継ぎを開始する。
- Tool callとresultの対応を壊さない。大きな結果を直接返した経路で、後から結果をrefだけに差し替えられるとは仮定しない。原則として最初のTool結果をref/概要にし、本文は専用読取で取り込む。
- 認可や情報鮮度の再検証は、prefixを固定してもgenerationごとに続ける。Worldが失効したらDを再作成する。history-boundでそれが許されなければ新Segmentへ移る。安全検査自体は撤去しない。

Segment内で安定させるのはdiscovery入口と最小bootstrapのトップレベル定義であり、検索対象のTool集合ではない。catalog更新、候補の切替、追加describeは通常の検索・Tool応答として扱い、全catalogを凍結しない。回数上限はDで知らせ、実行側でも拒否する。個別Toolの失効・認可変更は既存の実行時検証で拒否し、未参照Toolの更新だけではSegmentを再構成しない。F/Lに失効対象の情報が含まれる場合やbootstrap自体の変更は再構成する。legacy/direct提示を残す経路は別契約として実際のschema変化を計測する。

### 再構成トリガーとヒステリシス

1. 次の送信見積りがBの70%を超え、削減可能な履歴がある場合、ターン境界で再構成する。Tool往復途中は対応関係を維持できる境界まで遅延し、hard limitへ達するなら送信前に再計画/停止する。
2. Scope/適用される認可変更、F/Lに関わる訂正・忘却・失効、固定設定・bootstrap schema・rendererの版変更では即時再構成する。実行中generationは旧依存で完了を受け入れない。catalog全体のepoch変化とSegment世代は別管理し、既存referenceの再検証は省略しない。
3. 明示的なタスク完了・新しいタスクへの切替では、次の推論の前に再構成する。
4. TTL経過だけでは再構成しない。adapterが示すcache miss見込みは、既に必要な再構成のタイミングを決める補助情報に限る。

再構成後のF+LはBの45%以下を目標とする。現在入力とMustが大きい場合は目標達成を必須にせず、100%以下の安全条件を守る。同じsource snapshotに対して効果のない再構成を繰り返さない。同一generationで再構成は1回、残りは範囲読取や明示的overflowへ分岐する。

引き継ぎフィールドは `goal, target, completion_criteria, scope_refs, constraints, decisions, open_items, active_operations, adopted_evidence_refs, unresolved_conflicts, omitted_history_locator`。各項目にorigin、source_refs、observed_at、statusを付ける。

Runtime状態と明示的な構造化記録は決定的に生成する。自然文からの意味抽出を「決定的」と偽らない。未抽出の場合は根拠の抜粋を残すか、LLM要約を派生記録として作り一度保存する。Memory OFFでは会話・活動記録とRuntime状態から生成し、Personal State依存を持たせない。

Segment manifestは `segment_id, previous_segment_id, start_reason, scope_snapshot, policy_version, bootstrap_tool_schema_digest, renderer_version, adapter_contract_version, fixed_render_blob_id, carry_record_id, last_entry_sequence, dependency_refs, input_budget, forget_epoch, created_at, status`。generation manifestには選択entryの範囲、D/Uのrefとdigest、最終wire digest/bytes、選択・省略理由、依存snapshot、およびtool selectionのdecisionId/revisionId/describe record/invocationIdを残す。本文を含むrenderingは削除対象の派生データとして追跡する。

## C. 記録と既存Tool Selectionへの接続

### 保存契約

アプリケーションが実際に受領した本文・Tool引数/結果を、モデルへ公開する前にSQLiteへcommitする。streamはchunkを逐次記録し、完了時に確定する。中断時は受領済み部分をpartialとして残す。保存失敗時は未保存内容をモデルへ渡さず、非破壊的にエラーを返す。副作用Toolはintentを先に記録し、結果保存に失敗したらoutcome_unknownとして照合し、再実行しない。

検索query、検索実行、順位付き結果、Fetch取得、抽出結果は別record。検索結果全体の原文と結果ごとの構造化項目を関連付ける。同じURLの取得も実行ごとに別recordとし、blobだけ重複排除する。

既存conversation_messagesは引き続き会話原文の正本。recordsに既存IDへのlocatorを登録し、同一本文を第二の正本へ複製しない。新しい取得データはblobが正本。外部ContextStillの取得結果もローカルrecordとしてsnapshot化し、外部IDと版を併記する。

記録JSON例。IDはすべて同一のopaque ID空間。rankは検索結果のみ1始まり、source_rangeは対応する表現の非圧縮UTF-8 byte offset `[start,end)`。

```json
{
  "id": "rec_fetch_01",
  "kind": "web_fetch",
  "origin": "external_observation",
  "run_id": "run_01",
  "turn_id": "turn_03",
  "parent_execution_id": "rec_fetch_call_01",
  "source_refs": ["rec_search_result_02"],
  "scope_refs": ["project:p1"],
  "observed_at": "2026-09-22T03:00:00Z",
  "recorded_at": "2026-09-22T03:00:01Z",
  "locator": {"url": "https://example.com/article", "file": null},
  "rank": null,
  "state": "complete",
  "content": {"representation": "received_body", "blob_id": "blob_01", "sha256": "sha256-placeholder", "bytes": 120000},
  "capture": {"truncated": false, "reason": null},
  "instruction_authority": "none"
}
```

受領本文、HTMLからの可読テキスト、LLM抽出は別のrepresentation/recordとする。HTMLのoffsetと可読テキストのoffsetを混同しない。content hashは指す表現の非圧縮bytesから計算する。

### 記憶参照の論理操作とTool発見

以下のrecall_activity/recall_memory/query_world/read_recordは四つの論理操作と仮称である。メモリー/World参照の高頻度入口は常設可能とし、常設しない操作は既存catalogから発見できるようにする。既存recall_conversation、ContextStillの5種、World照会、resultRefのページ読取を棚卸しし、既存で足りる操作は再利用、不足する記録横断・範囲読取だけを追加する。既存Toolの意味を一つの曖昧な万能検索へ潰さず、直接定義または検索用metadataとusageに使い分けを登録する。

常設する参照入口は直接呼び出せる。それ以外はtools_searchで候補を取得し、tools_describeで以下に相当する契約を必要時に読み、tools_invokeのargumentsとして渡す。以下のJSONはbackend操作の設計例であり、現在のgateway公開schemaではない。既存の直接提示Toolを廃止・改名する実装は本書だけでは決定せず、移行時に既存互換性と必要なbootstrap例外を確認する。

Contextには目的、検索すべき不足情報、利用中のToolの短い選択理由と使い方のref、未完了invocationを残す。選択していないTool一覧・全usage文書は残さない。再利用する使い方はtoolId/revisionId/schema_hash付きの原典refで保持し、candidateRef/executionRefを永続的な権限として保存・再生しない。現行referenceの10分TTLとrun/Scope等の束縛に従い、失効後は再検索・describeを行う。

入力例:

```json
{
  "recall_activity": {
    "query": null,
    "kinds": ["search_execution"],
    "time": {"preset": "recent", "before_record_id": "rec_current"},
    "run_id": null,
    "scope_refs": ["project:p1"],
    "parent_id": null,
    "rank": null,
    "limit": 10,
    "cursor": null
  },
  "recall_memory": {
    "query": "採用した設計と理由",
    "types": ["decision", "knowledge", "experience"],
    "scope_refs": ["project:p1"],
    "as_of": null,
    "limit": 10,
    "cursor": null
  },
  "query_world": {
    "seed_refs": ["entity_01"],
    "question": "現在の目標に必要な依存先は何か",
    "relations": ["depends_on"],
    "max_depth": 2,
    "scope_refs": ["project:p1"],
    "as_of": null,
    "limit": 10,
    "cursor": null
  },
  "read_record": {
    "id": "rec_fetch_01",
    "representation": "readable_text",
    "range": {"unit": "utf8_byte", "start": 0, "max_bytes": 8192},
    "query": null,
    "cursor": null
  }
}
```

初期limitは10、最大20。検索queryは1,024 bytes以下、World深さは最大3、read_recordは1回8,192 bytes以下。read_recordのrangeとqueryは排他的。queryはその記録内の全文検索。UTF-8境界へ調整した実際のrangeを応答に返す。relative timeはRuntimeの時刻とtimezoneで解決し、cursorには確定した時間範囲を固定する。

Scope入力は検索を狭めるhint。実効Scopeは呼出元の認可snapshotから導出し、入力で拡張できない。record_idが既知でも毎回認可する。検索のrank・totalも認可済み集合に限定する。

共通出力例:

```json
{
  "status": "ok",
  "items": [{"ref": "rec_search_result_02", "rank": 2, "text": "結果の短い説明", "origin": "external_observation", "source_refs": ["rec_search_01"]}],
  "refs": ["rec_search_result_02"],
  "scope": ["project:p1"],
  "observed_at": "2026-09-22T03:00:00Z",
  "uncertainty": {"state": "unverified", "reason": "search-snippet-only"},
  "truncated": false,
  "coverage": {"total": 5, "returned": 1, "total_relation": "exact", "index_state": "complete", "searched_ranges": []},
  "next": null,
  "snapshot_id": "snapshot_01",
  "reused_from": null,
  "instruction_authority": "none"
}
```

共通envelopeを最大8,192 bytesに制限し、read本文だけで枠を使い切らない。必要なら本文を短縮してcontinuationを返す。多数レコードでは各itemに時刻・不確実性を付ける。totalが不明ならnull/unknown、少なくともNならlower_bound。ページ終端と「調査が十分」を同一視しない。statusはok/empty/partial/unavailable。認可不可と存在しないIDは同一の安全なunavailable応答にする。Worldはunknown_seed/ambiguous_seed/stale/pending等の既存意味をreasonに保存する。

itemsの必須内容は論理操作ごとに次のとおり。共通のrefはローカル記録ID、外部のassertion/entity IDは別フィールドにする。

| 論理操作 | items内の必須フィールド |
| --- | --- |
| recall_activity | ref、kind、parent_execution_id、rank（該当時）、query（検索実行時）、text、observed_at、source_refs |
| recall_memory | ref、memory_type、claim、status、origin、observed_at、valid_until、source_refs、external_ref（該当時） |
| query_world | ref、entities、relations（type/from/to）、conditions、evidence_refs、derivation、missing_evidence、observed_at |
| read_record | ref、representation、sha256、actual_range（start/end）、text、source_refs、capture_state |

Memory OFFでも活動記録・原典読取の能力は発見・実行できる。Personal State/Worldの抽出・注入は既存設定に従う。外部ContextStillの可用性は各clientの設定・認可に従い、Memory flagだけで一律無効と決めない。利用不能な能力は既存source eligibilityに反映する。ON/OFF変更でF/Lの内容・権限が変わる場合にSegmentを再構成する。

### 再利用・取得終了・根拠

ターン内Tool上限は既存12回を初期値として維持し、search/describe/invokeもそれぞれ数える。探索・詳細読取・実行に必要な回数を含めて実測する。内部sourceへのfan-outにも別の予算を適用し、gatewayの1呼出しで上限を迂回させない。複数の外部メモリーsourceを束ねる場合の初期追加上限は2件/操作、6件/turnとし、既存のより厳しい上限を緩めない。同一ref/rangeの無益な再読を検出し、過去結果と次の取得方法を返す。

再利用keyはtool版、正規化引数、principal/Scope、認可epoch、forget epoch、source snapshot。Web検索はさらに鮮度要求を含める。同じturn内でも更新要求・訂正・source変化・失敗後の再試行なら再実行を許す。「さっきの検索」は保存記録の取得であり、新しいWeb検索ではない。

既存MCPのresultRefも所有者・ACL・10分TTLを持つため、長期record_idと同一視しない。結果を受領した時点で保存記録へ紐付け、TTL後の「さっき見た内容」は認可されたrecord読取で復元する。過去結果を読めることと、Toolを再実行できることは別。既存TTL結果領域だけでは全入力の長期保存要件を満たさない。

結果をmodelへ渡した記録はexposed_refs、回答が明示的に採用した記録はclaimed_refs、引用内容との照合が済んだものはvalidated_citationsとして区別する。Providerの構造化出力、または検証可能なinline citationでclaimed_refsを受け取る。未知ID・未提示IDは拒否するが、IDの存在検証だけで主張の正しさを保証しない。対応しない経路はclaimed_refs=unknownとし、Runtimeが推測で補わない。

## D. 大量入力・抽出

以下の閾値は非圧縮UTF-8の読取対象表現のbytesであり、保存上限ではない。

| サイズ | 初期方式 |
| --- | --- |
| 8,192 bytes以下 | 保存後、共通Tool応答予算に収まる部分を直接返す。残りはref/cursor |
| 8,192超〜65,536以下 | 保存後、最大2,048 bytesの決定的outlineとrefを返し、範囲読取/記録内検索 |
| 65,536超 | 同じoutline方式を基本にし、目的上必要なときだけ抽出jobを実行 |

outlineはHTML/Markdownなら見出しと直後の文、コード/JSONなら構造の境界、plain textなら段落の先頭を順序どおり最大20項目。offsetと表現hashを添え、採用規則とparser版を記録する。先頭文を結論と扱わない。見出しのない後半も記録内検索で到達できるようにする。

抽出job入力はpurpose/query、原典ref、対象range/hash、要求フィールド、Scopeと依存snapshot。出力はclaims[]、evidence_spans[]、uncertainty、omitted_ranges。証拠spanはRuntimeが原典と照合し、不一致を採用しない。

初期job予算は最大2 model calls、入力合計65,536 bytes、出力合計8,192 bytes、壁時計30秒。原典がそれを超える場合は範囲を選びcoverageを表示し、全文を検証したと主張しない。課金経路の追加抽出にはprice表と金額上限を必須とし、初期上限$0.02/job、$0.10/turn。モデルの最大出力を含む最悪費用を送信前に予約し、上限内へ入らなければ実行しない。単価不明・未設定では自動課金jobを起動せず、決定的outline/範囲読取へ戻す。これらは初期調整値であり実測性能を意味しない。

非同期jobはpendingを返し、Runtimeの完了通知で次generationへ反映する。全文読取を待つ必要がない依頼をブロックしない。job結果が遅れてもScope/forget/source版を再検証してから採用する。

主会話へ入れた本文は当generation限定。次ターンへは採用事実と根拠refを残す。history-boundなProviderではBの手順で安全に区間を切り替える。原典はSQLiteに残す。

## E. SQLiteの論理スキーマと運用

これは新規migrationの論理定義。既存テーブルへの対応を実装時に決め、重複した意味状態の正本を増設しない。時刻はUTCのINTEGER milliseconds、IDはTEXT、bytes/sequenceは非負INTEGER、JSONはjson_valid制約を付ける。

| テーブル | キー・主要列 | 不変条件 |
| --- | --- | --- |
| records | id PK、kind、origin、run_id、turn_id、parent_execution_id、observed_at、recorded_at、rank、locator_json、capture_state、version、existing_source_locator、forget_epoch | complete確定後の本文/意味はimmutable。更新は新record。既存会話本文はlocator参照 |
| record_scopes | record_id + scope_key + relation PK、policy_revision | 複数Scopeを保持。Scope intersectionを無条件に認可とみなさず既存policyを適用 |
| record_representations | record_id + name PK、blob_id、sha256、byte_length、parser_version | received_body/readable_text等を分離 |
| blobs | id PK、dedup_domain、sha256、codec、raw_bytes、stored_bytes、data BLOB、ref_count | UNIQUE(dedup_domain,sha256)。同一bytes確認後に共有、ref_count非負 |
| blob_chunks | blob_id + sequence PK、raw_offset、raw_bytes、codec、data BLOB、sha256 | 大きい原典の範囲読取用。inline dataとは排他 |
| record_dependencies | derived_record_id + source_record_id + representation + start_byte + end_byte PK、source_hash、dependency_kind | 抽出・要約・回答・renderingから原典へ辿れる。依存閉包で失効 |
| record_text_chunks | integer id PK、record_id、representation、start_byte、end_byte、text、index_version | 可読表現の派生索引。原典正本ではない |
| record_fts | FTS5(text), content=record_text_chunks, content_rowid=id, tokenize=trigram | insert/deleteとcontent表を同一transactionで更新 |
| context_segments / context_entries | Bのmanifestとimmutable entry、rendered_blob_id | renderingもScope・忘却の依存対象 |
| context_generation_records | generation_id + record_id + usage_kind PK、range、digest、selected、reason | exposed/claimed/citation/dynamicを区別 |
| record_jobs | id PK、kind、status、input_refs、scope_epoch、forget_epoch、lease、budget、result_ref | 既存workerのlease/fence方式を再利用 |
| record_tombstones | record_id PK、forgotten_at、forget_epoch、reason_code | 本文を含めない。既存journalへ統合 |

FKは関連整合性に使うが、forgetをCASCADE DELETEだけで完了させない。recordsの索引は(run_id,recorded_at,id)、(turn_id,id)、(kind,recorded_at,id)、(parent_execution_id,rank,id)、record_scopes(scope_key,record_id)、dependencies(source_record_id,derived_record_id)。Scope認可・時刻条件をSQLのLIMIT前に適用し、N+1で全候補をScope判定しない。

blobは4,096 bytes以上をzstd候補とし、圧縮後に小さくなる場合のみ圧縮する。大きい本文は64 KiB程度の独立圧縮chunkとoffset indexを使い、範囲読取ごとに全文解凍しない。hashは圧縮前bytes。dedup_domainを同じ認可・保持境界に限定し、hashだけで原典を読むAPIは作らない。

検索索引はUTF-8境界で最大8 KiBのtext chunkへ分割し、境界に2文字のoverlapを設ける。先頭だけを恒久的な索引対象にしない。最初の64 KiBを優先し、残りは非同期で全文索引化する。途中はindex_state=partialと未索引rangeを返す。後半を指定したread_recordは索引完了に依存しない。

3文字以上はtrigram、1〜2文字は認可Scope・時刻を限定したLIKE/完全一致/metadata検索を使う。大域の無制限scanは禁止。1回の短語scanは最大2 MiBの本文を読み、超過はcursorとpartialを返す。「この話題」が意味的に曖昧な場合までFTSだけで解決済みとしない。初期実装にembeddingを必須としないが、取得評価で不足があれば別途検討する。

### 全入力保存・容量・redaction

「全入力」はアプリケーションが受領したcontentを指す。認証header・secret store・輸送プロトコルの認証情報を新たに記録対象に加えない。Tool本文に含まれる内容は原典として保持し、表示・検索・外部送信時には別のprojectionで必要なredactionを行う。原典を診断用redactorで短縮しない。

初期保持期間は無期限、時間による原典の自動削除なし。初期live DB目安上限は10 GiB、80%で容量通知、95%で新しい大量取得・抽出を停止する。残りを進行中の記録確定と忘却操作に予約する。上限は設定可能。受領streamの永続化可能量を先に予約し、不足時は取得を中断してpartialと理由を保存する。保存されなかった全量を取得済みと表示しない。索引・rendering等の再生成可能データは容量回収可能だが、原典は勝手に消さない。これは初期運用値であり、アプリ全体のディスクquotaではない。

### 忘却の処理順序

1. 対象IDと依存閉包を特定し、新しいforget epochを採番。バックアップ外journalへ本文を含まない忘却intentをdurable保存する。中断時は起動時にreplayし、完了まで読取を止める。
2. SQLite transactionでtombstone、認可無効化、FTS削除、原典・表現削除、派生メモリー/World/回答・renderingの無効化、job取消要求、Segment失効を一括commitする。FTS削除に必要な旧textはcontent行を消す前に使用する。
3. 進行中generation/streamを取消し、遅着結果の保存・採用・表示をfenceで拒否する。既に表示/送信した内容が回収できたとは主張しない。
4. blobの参照を減らし、参照ゼロのblob/chunkを削除する。同一本文の別取得が残る場合は共有blobを維持する。記録単位の忘却と、同内容の全取得を忘れる操作を区別する。
5. 管理下バックアップをquarantineし、同じ削除処理を適用した検証済みのクリーンなバックアップへ置換する。処理不能ならrestore対象から外し、cleanup pendingを明示する。外部コピーは自動削除できないが、アプリで復元する際は最新journalの適用を必須にする。
6. 新Segmentを組み、旧remote session/continuationを再利用しない。Provider側削除APIがあれば実行・確認する。無ければcache等の物理削除完了は宣言せず、新規送信と参照の停止を保証する。

forget journalの永続化が失敗したら成功を返さず、対象の参照をブロックする。バックアップには索引だけでなくrendering、原典blob、派生出力が含まれ得るため同じ閉包で処理する。将来のバックアップは清掃完了後のsnapshotから作る。

SQLite通常表にはsecure_delete=ONを使用する案とし、FTS5側のsecure-delete対応を組込SQLite版で検証する。WAL checkpointと再構築/VACUUMはforeground外で、削除pending時またはfree page比20%以上を目安に実行する。これだけでSSD・OS snapshot・外部バックアップを含む物理完全消去を保証しない。

## F. 計測とProvider契約

最初に既存generation manifestへcontent-freeなusageを記録する。input/prompt、cache_read、cache_write（TTL別）、output、reasoningの各値はProvider定義と一緒に保存し、未提供はnull。reasoningがoutputに内包される経路では二重課金計算しない。stream_options.include_usageは対応確認済みの経路だけに要求する。

cache率は入力定義を正規化したうえで `sum(cache_read)/sum(total_input)` を主指標とし、generationごとの分布も出す。read/write/uncachedが排他的か、input値へ含まれるかをadapterで固定する。料金はProvider・モデル・価格版・通貨・取得日を持ち、未知なら費用もnull。切断でusageを取り逃した回数を欠測として計上する。

併せてTTFT、ユーザーに見える最初の本文までの時間、完了時間、再構成回数、Tool回数、重複照会再利用率、根拠ref付与率、取得成功率、制約保持率、全入力保存失敗率を測る。各指標はMemory ON/OFF、cold/warm、通常/Tool follow-up、Provider/modelで分ける。

adapter契約は `cache_support=verified|unsupported|unknown, cache_unit, minimum_input, ttl_semantics, cache_control_fields, usage_mapping, history_binding, edit_capabilities, token_capacity, byte_transport_limit`。Chat Completions互換という名称や、AgentSessionのbytes一致だけでcache対応を断定しない。ローカルのprefix一致率と実際のcached inputを別計測にする。

## G. 実装順序と受入条件

1. usage/費用/latencyの取得と欠測表示。現行履歴構築のbaselineを取る。
2. records/原典保存・Scope認可・忘却依存・共通IDを追加し、Memory OFFで四つの履歴質問へ回答可能にする。
3. 既存Tool Selectionへ記憶・活動・原典読取の能力を接続。既存の検索/describe/invoke/resultページ読取を再利用し、不足だけ追加する。大きな結果はref/outline/範囲読取にし、原典を渡す前に保存する。
4. F/L/DとSegmentを実装。rebuildableな経路で先に検証し、history-boundはadapter契約ごとに対応する。
5. Personal State/Worldの必要量選択と任意の抽出jobを統合し、閾値を実測で調整する。

受入では最低限、次を検証する。

- Memory OFFでも「さっきの検索」「2番目」「当時の本文」「何を根拠に」が保存IDを辿って回答でき、新規Web検索が不要。
- 音声/文字の切替でF/Lの既存bytesが変わらない。discovery経路のsearch/describe/invokeで3入口のschema集合・順序が変わらず、個別Toolの契約は応答として取得される。
- catalogが1,000/5,000件でもFのTool定義bytesは件数に比例して増えない。候補は最大8件、必要なusageページだけを取得し、契約に従ったinvokeまで到達できる。FTS/embedding/rerankerの縮退とユーザー訂正ルールも検証する。
- 常設のメモリー/World参照から一般Toolが必要と判断した場合、tools_search→tools_describe→tools_invokeへ進む。過去のTool名や成功例がメモリーにあってもsearchを飛ばさない。一般Toolが不要な質問では無用なTool検索をしない。
- 無関係なToolの追加・更新ではF/Lを作り直さない。一方、実行予定のToolの失効・schema変更・ACL変更は既存reference検証で拒否される。
- candidateRef/executionRef/resultRefのTTL後、過去結果はrecordから読めるが、過去executionRefによる実行は拒否される。
- 動態情報の変更後もF/Lは同じで、Dは最新。過去の制約と訂正が衝突したらRuntimeが失効を反映する。
- 100 KiB超のFetchは全文保存され、次ターンへ本文を持ち越さず必要事実とrefが残る。後半にだけある文字列を検索・範囲読取できる。
- 70%から45%への再構成後、次ターンで同じ理由の再構成が連発しない。Mustの大きいケースは黙って削除せず失敗理由が出る。
- history-boundのTool/thinking継続で不正な過去編集を起こさない。本文除去時は検証済み編集か新Segmentへ移る。
- 忘却直後、F/L/D、検索、job遅着結果、復元後DBのいずれにも対象が復活しない。管理バックアップの清掃状態を確認できる。
- 未認可Scopeへの既知ID直読、cursorのScope差替え、重複blob経由の読取を拒否する。
- 同一queryでもfreshness/snapshot/forget epoch変更時には古い結果を返さない。単なる同一query文字列で再利用しない。
- 保存中断・ディスク不足・process crashでpartialと未完了を区別し、副作用Toolを二重実行しない。
- 1万/10万recordと1 MiB原典で、検索・範囲読取・Context構築のp50/p95とDB/FTSサイズを同一機材で比較する。普遍的なlatency達成値を事前に偽装しない。

実装後の検証は、変更した保存/検索/Context/adapterのtargeted Rust tests、該当Frontend回帰、IPC/schema互換性を順に行う。Provider実機試験は上限費用付きの別laneで行い、fixtureの成功をcache実測の成功と数えない。本版は文書作成のみで、これらの実装・受入を完了したものではない。

## 根拠と再評価すべき点

現行コード確認: memory/context_window.rsと分割先、runtime/context/broker.rs、runtime/conversation_inputs.rs、memory/recall/mod.d/01.rs、memory/personal_state/journal.rs、database_backup.rs、redact.rs。Tool関連はtool_selection/gateway.d/01.rs、gateway_schemas.rs、schema.rs、retrieval.rs、service.d/02.rs、service.d/03.rs、contracts.d/01.rs、mcp/results.rs、providers/stream/agent_dispatch.rsを追加確認した。作業ツリーは他の変更を含むため、本案は特定commitの認定ではない。

- [SQLite FTS5](https://sqlite.org/fts5.html): trigramの3文字未満制約、external-contentの整合責任を参照。
- [SQLite secure_delete](https://sqlite.org/pragma.html#pragma_secure_delete): 通常表とFTS等のshadow tableの差、および消去保証の限界を参照。

次のレビューでは、特に「Dを外した後のProvider履歴整合」「Memory OFF時の引き継ぎ」「Scopeの認可式」「原典の保存失敗と副作用Toolの結果照合」「共有blobと忘却閉包」を確認する。予算・閾値は初期値として固定し、計測後に調整する。安全条件と原典保存の要件は閾値調整で緩めない。
