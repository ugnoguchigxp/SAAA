# SAAA Personal World Model 初回実装計画

作成日: 2026-09-19  
状態: 実装着手用の計画。コード・DB変更は未実施。  
対象: WM-M1「構造化された関係モデルの永続化と照会」  
上位文書: [Personal World Model Concept](../../docs/saaa-personal-world-model-concept.md)

## 1. 今回の到達点

初回は、確定済みの会話Sourceを根拠にした構造化データを、既存Personal Stateの履歴へ保存し、Scope・訂正・忘却を守って小さなWorldSliceを取得できるところまで作る。モデルを使わず、一時DBと固定fixtureで再現できる実装にする。

会話からの自動抽出、Runtime状態の取り込み、本番のContext Brokerへの投入はこの回に含めない。初回で土台と探索を検証し、次の計画で入力と利用先を接続する。基盤が動くことと利用者向けV0完成を区別する。

本計画はコンセプトのP2共通基盤と、P2bの構造化入力による検証部分を先行して実装する。P2aの会議・Task参照やP2cの会話接続を完了扱いしない。P2aを飛ばして本番へ出す計画ではない。

成功時に動く一連の処理:

```text
合成会話Sourceを登録
  → 型付きのWorld assertionを既存StatePatchとしてcommit
  → 同じtransactionでWorld投影を再構築
  → 関連経路・因果経路・ResearchGap候補を照会
  → 同じpatchの再送はno-op
  → 訂正・失効・忘却を反映
  → 再起動・再構築後も同じ結果
```

ここでの入力は信頼済みAdapterが組み立てる内部契約であり、LLMや外部クライアントにStatePatchの直接投入を許すものではない。

## 2. 実装の粒度と進め方

### 2.1 一度に一つの作業カードを渡す

初回をWM-00〜WM-15の16作業に分ける。一つの作業は「一つの責務と、それを確認するテスト」とし、通常は実装1〜3ファイルとテスト1〜2ファイルを目安にする。行数や所要時間だけで分割しない。型、永続化、探索、接続を一度に実装させない。

実装担当には毎回、共通契約の該当節、直前までの進捗記録、対象カードだけを実行対象として渡す。後続カードは参照できるが、先取りして変更しない。カード内の処理とテストを完了してから次へ進む。各カードでユーザー確認やcommitを強制するものではない。

一つのカードが実装5ファイル超の変更になる、または独立した設計判断を二つ以上必要とする場合は、先にカードを分割して計画へ記録する。テストfixtureやmodule宣言の機械的追加はこの目安から除く。閾値を守るために必要なテストを省略しない。

### 2.2 実装担当が決めてよいこと

- private helperの名前、型のimport、読みやすい関数分割。
- 同じ入出力と順序を保つ、標準ライブラリによる実装方法。
- 既存fixtureへの補助関数追加、決定的なテストデータの配置。

実装担当が独自に変えないこと:

- 正本、Scope対応、Source検証、状態遷移、忘却、上限、探索の意味。
- DBアクセスの所有者、公開API、Provider経路、依存ライブラリ。
- 合格条件、失敗時の挙動、対象外の機能。

仕様との不一致は最小の再現と変更案を記録する。コンパイルエラーや通常のテスト失敗は担当範囲で直す。契約変更が必要な場合だけ該当作業を止め、設計レビューへ戻す。曖昧な値を仮定して先へ進めない。

## 3. 現行実装から確認した注意点

確認時HEAD: `1b595ff`。実装開始時はWM-00で再確認する。

| 現在の箇所 | 確認した制約 | 計画への反映 |
| --- | --- | --- |
| `crates/personal-state-core/src/reducer.rs`の`Ledger::apply` | patch全体16 KiB、assertion数＋transition数32以下 | 候補50件というコンセプトの初期案を使わない。既存上限を維持 |
| 同`validate_assertion`、`store.rs` | payloadはJSON符号化後2,000 byte以下 | フィールド上限に加え、最終byte長を検査 |
| 同`validate_graph` | `Assertion.depends_on`は非循環の根拠依存 | Worldの関係edgeと混同しない。Worldの循環は許可 |
| 同`apply_transition` | 同kind・semantic_key・task_requestのActive重複を拒否 | 関係の版更新・競合を明示transitionで表現 |
| `sources.rs`、`store::load` | Sourceは会話messageとその版・範囲へ照合される | 初回の永続化入力は会話Sourceのみ。架空のRuntime Sourceを作らない |
| `worker_scope.rs` | ledgerのscopeはprimary、Project等はtask_requestに対応する経路がある | `AccessScope.scope`をproject IDへ置き換えない |
| `worker.rs`、`product_extract.rs` | Kindをdeserializeし、currentや配送Sourceへ既存assertionの情報を列挙する | World用Kind追加時に明示除外・拒否が必要 |
| `projection.rs` | 既存assertionをContext候補として列挙する | Worldを既存Personal State候補へ混入させない |
| `runtime/context/generation.rs` | personal-state等のsource_kindを指定して依存を再検証する | worldという名前を足すだけでは失効保証にならない。本番接続は後段 |
| `personal_state/schema.rs` | migration内でrecover→rebuildを呼ぶ | Worldテーブル・triggerの準備をrecoverより前に実施 |
| `persistence/schema.rs` | 現在DB版は20 | 他作業と競合しなければ21へ。実装時の最大版を確認 |

共通SourceRefやLedgerを「一般的に使えるように」作り直すことは、初回の課題にしない。

## 4. 変更範囲

### 含める

- World専用Kind、payload型、意味検証、名称照合。
- 既存StatePatchとWriterを通す保存、SQLite投影、再構築。
- 関連探索・因果探索、理由別Focus、ResearchGap候補、WorldSlice。
- 同一patchの再送、置換・撤回・競合、期限、Scope、忘却・復元。
- 通常抽出・通常Contextへの混入を防ぐ既存境界の変更。
- モデルなしのfixture、回帰試験、容量・性能の測定記録。

### 含めない

- World用LLM prompt、Extractor、自然文の訂正判定。
- 会議・Taskの実データ取り込み、外部資料Source Adapter。
- Context BrokerへのWorld供給、generation依存登録、UI・IPC・HTTP・MCPの新API。
- 汎用merge、条件の自然言語推論、confidence／strength、効果の数値予測。
- ContextStill／DeepStill実接続、調査キュー、新Scheduler、新crate、新dependency。

本番DB・実会話・認証情報をテストに使わない。既存の別作業ファイル、モデル設定、認証方式、Provider routing、LARM契約は変更しない。今回の依頼は計画作成までであり、ここに列挙した変更は後続実装の範囲である。

## 5. 初回で固定するデータ契約

### 5.1 Kindとpayload

既存Kindへ`WorldEntity`、`WorldRelation`、`WorldFocus`を追加する。JSON名は`world_entity`、`world_relation`、`world_focus`。`is_world()`と`is_continuity()`を定義し、既存7種の扱いを変えない。State assertionとruntime_refの実装は次段階とし、空の万能metadataで代用しない。

payloadのschema_versionは整数1、共通tagフィールドは`type`とし、値は`entity / relation / focus`とする。すべての構造体は未知フィールドを拒否する。Kindとpayloadのtagは一致しなければならない。`serde_json::Value`は保存境界に限り、意味検証後は型付きで扱う。

| 型 | 必須フィールドと制約 |
| --- | --- |
| EntityPayload | schema_version、entity_id、entity_kind、name、aliases。kindはproject / concept / metric。nameはtrim後1〜160 UTF-8 byte。aliasは最大4個、各1〜160 byte。entity_idはAdapter発行のopaque ID |
| RelationPayload | schema_version、from_entity_id、to_entity_id、relation_type、effect_input、conditions、basis、evidence_stances。端点は異なる既存Entity。relation_typeはconceptの6種に限定 |
| FocusPayload | schema_version、entity_id、reason、objective_assertion_id。reasonはcurrent_work / explicit_interest。objective_assertion_idはcurrent_work時のみ必須、既存のActive Objective ID |
| Conditions | 最大4組のkey/value、各key 1〜32 ASCII byte、value 1〜96 UTF-8 byte。keyで整列し重複keyを拒否。空配列は条件未指定であり、無条件の証明ではない |
| EvidenceStance | 既存SourceKeyとstance。stanceはsupports / challenges / context。最大4件。SourceKeyはAssertion.evidenceの要素であり、外部URL文字列を代入しない |

Entityのaliasはnameと同じ正規化関数で重複排除する。正規化は前後空白除去、連続するUnicode空白をASCII空白一つへ、ASCII英字のみ小文字化する。日本語の表記ゆれ、全角半角、Unicode正規化、語幹の同一視は行わない。正規化名は表示名を書き換えない。

`basis`は`user_statement`または`model_hypothesis`のみ。user_statementはsupportsに対応するSourceRole::Userの根拠を少なくとも一つ必要とする。Assistant由来だけでユーザーの発言扱いにしない。Runtime観測と外部資料の主張は後段。basisは主張の出所を示し、真偽の保証ではない。作用関係は常に仮説として返す。

`effect_input`は作用関係で必須。始点がmetricなら`quantity_increase`、conceptなら`intervention`とする。作用関係の終点はmetricに限定する。interventionのconcept名は「○○の導入」のように変更内容を示すfixtureを使う。非作用関係ではeffect_inputをnullとする。型だけで自然文の意味が検証できたとは報告しない。

### 5.2 ID・Scope・依存

- Entity IDとAssertion IDを分ける。訂正でEntity IDは維持し、新Assertion IDを発行する。
- primary ledgerのprincipalを使い、`AccessScope.scope="primary"`を維持する。
- 初回のWorld assertionは`task_request=Some("project:<opaque-id>")`に限定する。null、user全体、複数Project共有は拒否する。
- Project Scopeは既存`context_scopes`でactiveであることをAdapterが確認する。名前からScopeを新規発行しない。
- 全input_dependenciesについて、Source版が有効で、既存`personal_source_scope_refs`に対象Projectとの対応があることを検証する。LLMが書いたscope文字列を認可の根拠にしない。
- Relationは両端Entityの現在のAssertion IDを`depends_on`に持つ。Focusは対象Entityと、current_workの場合はObjectiveにも依存する。依存先は同Project、許可purpose・classification内、Activeでなければならない。
- Entityが別Entityへ関係を持つことを、そのEntity assertionの`depends_on`に書かない。A→B→AのWorld関係は有効であり、根拠依存の循環とは別物である。
- 端点Entityの訂正で依存元が失効したら、Relationを勝手に付け替えない。新しい根拠依存でRelationを再登録する。

Sourceの証拠範囲と、生成へ渡した全入力への依存を分ける既存契約を維持する。全入力への依存を、支持Evidenceだけへ減らさない。

### 5.3 semantic_keyと置換

semantic_keyは`wm1:`に続くSHA-256 hexとする。計算入力は固定順序のJSON配列を`serde_json::to_vec`で符号化したものとし、Mapの列挙順に依存しない。sorted_conditionsはkeyで整列した`[[key,value], ...]`配列で符号化する。開始・終了時刻はUnix ms、終了なしはnullで表す。

| Kind | ハッシュ入力の配列 |
| --- | --- |
| Entity | `["entity", project_scope, entity_id]` |
| Relation | `["relation", project_scope, from, to, relation_type, effect_input, sorted_conditions, valid_from, valid_until]` |
| Focus | `["focus", project_scope, entity_id, reason]` |

`related_to`のみ端点IDを辞書順へ揃える。表示名、basis、Evidence、payload IDをsemantic_keyに含めない。同じ関係に根拠を追加する場合は新assertionとして旧版をSupersedeし、更新履歴を残す。条件・有効期間が変わった場合は別のsemantic_keyになるため、旧版の終了が必要ならRetractを明示する。

同じkeyのActive重複は拒否する。曖昧な「どちらが正しいか」の自動判定はしない。反証を含む新しい版を採用する場合は、旧版をSupersedeし、新版をAssert→Disputeとする。DisputedからActiveへ戻すには明示patchが必要であり、初回に自動再昇格は作らない。

### 5.4 状態と上限

EntityとFocusはActiveのみを通常投影へ使う。RelationはActive / Disputedを返せるが、Disputedは因果経路の根拠に使わず、関連経路で競合表示しGap候補にする。Candidateは診断用に保存できるが、経路へ参加させない。期限切れ、Superseded、Retracted、Invalidatedは現在の経路から除く。

モデル仮説とCandidateは別概念である。仮説として検証・登録されたRelationはActiveかつmodel_hypothesisになり得る。Activeを実証済みと表現しない。

| 項目 | 初回の固定値 |
| --- | --- |
| 新規World assertion / patch | 最大8件。transitionを含む既存32操作・16 KiB制約も必須 |
| payload | JSON符号化後2,000 byte以下。長い値を黙って切らない |
| 読み込むSourceKey / patch | 最大4件。依存Entity等の全入力を含め、超過なら拒否 |
| World assertion | Candidate / Active / Disputedの合計上限10,000件。履歴と訂正は別に扱う |
| 探索 | 深さ3、Node 30、Edge 60、経路10、隣接候補走査500件 |
| WorldSlice | JSON符号化後8,192 byte以下。要求側は小さくできるが上限を増やせない |

コンセプトの「累計10,000件で訂正は受け付ける」という案は、そのままでは新assertionによる訂正と両立しない。初回は「有効項目数10,000件」に具体化する。同じtransactionでSupersedeされる項目を差し引いた数で判定し、Retract・forgetは上限にかかわらず可能とする。履歴はこの値では制限できないため、履歴を含むload・rebuild費用を別途測定し、本番の保持方針は次段階で決める。

## 6. 保存・投影の契約

### 6.1 入口を増やさず共通commitを検証する

`store::commit`へWorld用の検証hookを置く。別のWorld書き込みAPIだけを検証して、既存commitから不正payloadが入る構造にしない。新規assertionだけでなく、transitionの対象が既存World assertionである場合もhookを実行する。Worldを一切変更しないpatchには意味的な変更を加えない。

処理順は、Source・Scope検証 → World payloadと依存の検証 → `Ledger::apply` → 既存履歴・payload・依存の保存 → CAS → 既存投影とWorld投影の再構築、である。すべて呼出元のWriter transaction内で行い、失敗時は全rollbackする。ネットワークやawaitをtransaction内へ入れない。

同一patch ID・同一符号化内容の再送は既存契約どおりno-opで、revisionと件数は不変。同一patch ID・異なる内容はConflict。忘却後の再送でpayloadを復活させない。異なるpatch IDで同じ意味のデータを送った場合はActive重複拒否とし、初回は意味的なイベント重複排除を実装したと主張しない。後段のWorldDelta Adapterが安定イベントIDとpatchの対応を所有する。

### 6.2 投影テーブル

新規テーブルはすべて`personal_world_`接頭辞を付ける。履歴の正本は既存テーブルのまま。テーブルの用途とキーを固定し、実装担当が別の正本を追加しない。

| テーブル | 主キー・主な列 | 用途 |
| --- | --- | --- |
| personal_world_projection_meta | id=1、ledger_revision、input_epoch、policy_revision、built_at_ms | 投影整合性。再構築中は利用不可 |
| personal_world_entities | assertion_id PK、project_scope、entity_id、kind、name、canonical_name、status、valid_from、valid_until、revision | 対象の現在投影。project_scope＋entity_idは一意 |
| personal_world_aliases | assertion_id＋canonical_alias PK、project_scope、entity_id | 完全一致alias検索 |
| personal_world_relations | assertion_id PK、project_scope、from_entity_id、to_entity_id、relation_type、status、valid_from、valid_until、revision | 隣接探索。条件・根拠は上限付きpayloadを読む |
| personal_world_focus | assertion_id PK、project_scope、entity_id、reason、objective_assertion_id、status、valid_from、valid_until、revision | 現在の関連性の終点 |

必要な索引はEntityの`(project_scope, canonical_name)`、aliasの`(project_scope, canonical_alias)`、Relationの`(project_scope, from_entity_id, relation_type, assertion_id)`と`(project_scope, to_entity_id, relation_type, assertion_id)`、Focusの`(project_scope, entity_id, reason)`とする。SQLite外部キーは既存assertionと投影Entityの存在に適用するが、忘却時の消去をCASCADE任せにしない。

時間列はINTEGERのUnix ms。payload本文やGraph全体のJSON blobを投影へ重複保存しない。名称の投影は検索のための派生内容なので、忘却対象である。

再構築はまず全消去し、対象のActive Entity、利用可能Relation、Active Focusを同じsnapshotから作り直し、最後にmetaを記録する。metadataのrevision一致だけでは時間経過を検知できないため、読み取り時にも期限と依存状態を検査する。

Readerは投影を修復・書き込みしない。meta不一致なら`projection_stale`を返す。再構築はWriter側の明示操作または既存commit/recoveryに限る。履歴・epoch・policyと投影を同じread transactionで読む。

### 6.3 忘却とmigration

既存`personal_tombstones`へのINSERTを契機にWorld投影全体を消去する専用triggerを追加する。初回は対象だけを部分消去する最適化をしない。これにより既存のSource削除入口を取りこぼさず、名称やaliasも即座に消える。再構築時には既存のerased・依存閉包を使い、削除対象を戻さない。

Source本文の編集ではinput_epoch不一致により旧投影を読めなくし、再構築時に利用不能Source由来の項目を除く。編集と削除を同じものにしない。

World DDLと忘却triggerは`schema.rs`のrecover呼出しより前に準備する。旧DBを変更するmigrationは追加型とし、既存migrationの履歴を書き換えない。schema.sqlへテーブルを追記しただけで既存の`CREATE TRIGGER IF NOT EXISTS`が更新されたと考えない。既存triggerを置換する必要はなく、新しいWorld消去triggerを追加する。

実装開始時点でもDB版20なら21を使う。並行作業で版が進んだ場合は、最新構成へ適合させた版と理由を進捗記録に残す。自分の変更以外のmigrationを削除・巻き戻ししない。

## 7. 読み取り・探索の固定仕様

### 7.1 内部インターフェース

以下の名前は新規の予定APIであり、現在存在する関数ではない。外部公開はしない。

| 関数・型 | 契約 |
| --- | --- |
| `validate_world_patch` | Ledger、StatePatch、型付きpayload群、信頼済み検証Contextを受け、整合性を検査する。IOなし |
| `world::projection::rebuild` | Writer transaction内で呼ぶ。現在履歴から全投影を置換する |
| `world::query::activate` | read transaction、検証済みProject scope、AccessRequest、now_ms、seed指定、causal_direction、LimitsからWorldSliceを返す。causal_directionはforward / reverse、既定値forward |
| `WorldSeed` | EntityIdまたはExactName。最大4件。複数の名称候補を自動選択しない |
| `WorldSlice` | revision、as_of_ms、nodes、relations、focus、relevance_paths、causal_paths、research_gaps、notices、truncated |

WorldSliceは診断用構造化データとして返すだけで、初回はProviderへ送らない。source refsや条件を含むため、ログへ本文全体を無条件出力しない。

結果は、取得できたSlice、`projection_stale / pending_review / unknown_seed / ambiguous_seed`による省略、契約エラーの三つを区別する。省略時に経路配列だけを返して「関連なし」と同一視しない。契約エラーのreason codeは`world-invalid-payload / world-invalid-reference / world-scope-denied / world-limit / world-budget-too-small / world-projection-corrupt`を使い、既存Ledger・Sourceのエラーはそのまま保持する。認可不成立は空Sliceへの縮退ではなくエラーとする。

### 7.2 アクセスと現在性

SQLでproject_scopeと有効期間を絞り、payloadを読む前に既存AccessRequestでprincipal・purpose・classification・policyを検証する。`AccessScope.scope`はprimary、task_requestは照会Projectと一致させる。異なるProject間の横断探索は初回では禁止する。

同じProjectに、対象Sourceより新しい未処理または失敗中の会話Sourceがある場合は`pending_review`を返し、そのProjectのWorldSliceを省略する。初回は訂正意図を推測しない保守的な処理とする。別Projectの未処理入力によって恒常的に抑止しない。全体epoch差による一時的な再構築待ちとは区別する。

テストでpendingを解消するときは、合成Sourceについて既存の処理完了手続きを再現する。製品のjobをWorldの登録だけでcompletedへ書き換えるAPIは作らない。

### 7.3 探索順序

1. seedをID優先、正規化名、aliasの順で解決する。名前が複数対象に一致したら`ambiguous_seed`、存在しなければ`unknown_seed`とする。
2. seed IDを辞書順に揃え、幅優先で展開する。
3. 隣接edgeはrelation_type、相手Entity ID、assertion IDの順に読む。SQLの順序を省略しない。
4. 関連探索は両向きに辿れるが、edge本来のfrom/toと`traversed_reverse`を保持する。FocusのあるProjectまでの経路を返す。
5. 因果探索はActiveのincreases/decreasesだけで、forwardまたはreverseを明示指定する。関連探索の方向切替を流用して因果を反転しない。
6. 各経路で訪問済みEntityを持ち、同じEntityを再訪しない。別経路の到達までglobal visitedで排除しない。
7. SQLから取得・検査したedgeも500件の走査予算に数える。重複・却下edgeも無料にしない。各クエリに残予算以下のLIMITを付ける。

因果経路のconditionsは完全一致するものだけ結合する。空条件のedgeを含む経路は`conditions_unverified`とする。異なる条件のedgeは関連経路には残せるが、一本の因果経路にしない。符号の積、効果量、発生確率を出力しない。

関連経路はFocusを持つProjectへ到達したものだけを返す。因果経路は、同じ探索方向・条件でさらに伸ばせない最大の経路を返し、その単なるprefixを別件として重複列挙しない。深さや件数の打切りで止まった経路には打切りを付記する。複数seedから同じedge列になった経路は、向きを含むedge ID列で重複排除する。

Node・Edge・経路・深さ・走査上限に達した場合は`truncated=true`と理由を返す。SQLに取得残件があるか断定できなくても、上限到達時は保守的に打切りとする。

### 7.4 Focus・Gap・byte上限

初回のFocus順位はcurrent_work、explicit_interest。current_workのObjectiveが失効した場合はFocusを使わない。temporary_attentionはrequest生命周期との接続が必要なため後段へ回し、データを保存しない。

GapはFocusに到達した関連経路上の作用関係だけを対象とし、`disputed`、`conditions_unverified`、`hypothesis_unverified`の優先順で、一つの関係につき一件を返す。現在経路から除いたstale関係のGap検出は後段。初回に対応していない不足を検出済みと報告しない。

Gap keyは`[project_scope, relation_semantic_key, focus_entity_id, reason]`のcanonical JSON配列のSHA-256とする。revisionや生成時刻を含めない。質問は種別別の定型文とEntity名・条件から組み立てる。LLMによる質問生成やDBへのGap保存をしない。

順序はFocus区分、経路長、relation assertion ID、Gap key。出力のJSON byte数を測り、超過時は順位の低い経路単位で削り、孤立Node・Edge・Gapも落とす。根拠・条件・仮説ラベルだけを切らない。最低限のnoticeさえ収まらない指定予算は`budget_too_small`エラーにする。JSON文字列の途中切断は禁止する。

## 8. 配置と実装順

予定配置:

```text
crates/personal-state-core/src/world/
  mod.rs, model.rs, validation.rs, identity.rs, traversal.rs, relevance.rs
crates/personal-state-core/tests/
  world_contract.rs, world_traversal.rs
src-tauri/src/memory/personal_state/world/
  mod.rs, validation.rs, schema.sql, projection.rs, query.rs
  tests.rs, test_support.rs
spec/evidence/world-model/
  m1-progress.md, m1-baseline.md, m1-results.md
```

ファイル分割は責務に対応させる。少量のコードを機械的に多数のファイルへ分散しない。既存ファイルへの変更はmodule宣言、Kind境界、commit/rebuild/migration hookを中心にする。

順序は次で固定する。

```text
WM-00 → WM-01 → WM-02 → WM-03 → WM-04
  → WM-05 → WM-06 → WM-07 → WM-08 → WM-09
  → WM-10 → WM-11 → WM-12 → WM-13 → WM-14 → WM-15
```

この順序は実装担当の認知負荷を抑えるための直列順であり、別AI・別タスクへの自動委任を指示するものではない。

### WM-00: ベースラインと変更対象の確定

- 読む: 本書§1〜7、coreのmodel/reducer、store/sources/worker/projection/schema、persistence/schema、関連テスト。
- 変更: `m1-baseline.md`と`m1-progress.md`のみ。
- 作業: HEAD、dirty一覧、DB版、現行上限、関連テストの結果を記録。既存の未追跡ファイルを所有しない。
- 検証: §10のK1・K2・K3を実行する。失敗は既存／今回を区別できるよう最小ログを保存。
- 完了: 16カードすべてのstatus欄を持つ進捗表があり、ベースラインの成否が明記されている。

### WM-01: World型と名称正規化

- 変更: coreのworld/model、identity、mod、lib、world_contractテスト。
- 作業: §5.1の型、serde、正規化、semantic_key計算を追加。まだ既存Kindを変更しない。
- 検証: T01、T02。serialization round-tripに加え、未知field・不正enum・byte超過を拒否。
- 完了: 決定的な型とキーが動き、既存Ledgerへ影響がない。

### WM-02: Kind追加と既存経路の隔離

- 変更: core model、worker、product_extract、projection、commands、対応テスト。
- 作業: 3つのWorld Kindと分類helperを追加。workerのcurrent列挙と依存収集、product_extractの配送Source収集、既存Context投影、既存owner snapshotのitemsからWorldを除外する。抽出結果のWorld Kindは明示拒否。既存Continuityが正当に使うSource依存まで削らない。
- 検証: T03。通常ExtractorにWorld Kindを返すfixtureで保存0件。Worldが存在しても通常Context内容とcontinuity抽出依存が増えない。
- 完了: enum追加が製品機能の暗黙有効化になっていない。診断itemsの既存7種は変わらない。

### WM-03: payloadと根拠依存の検証

- 変更: core world/validation、world_contract。
- 作業: §5の型適合、端点、作用の向き、同Scope、depends_on、Evidence対応、件数上限を検証。clock・DB・ID生成は入れない。
- 検証: T04、T05。A→B→AのWorld関係は通り、Assertion依存循環は既存reducerで拒否される。
- 完了: 不正入力を許すためにreducerの既存検査を緩めていない。

### WM-04: 置換・撤回・競合のfixture

- 変更: core world_contractとtest helper。必要なWorld検証だけ調整。
- 作業: Entity→Relation→Focusの順に別patchで登録。Relationの置換、反証版のDispute、Objective撤回によるFocus失効を再現。
- 検証: T06、T07。同semantic_keyのActive二重化を拒否し、端点訂正後に旧依存Relationは経路対象外。
- 完了: 時間を進めるだけで期限切れが反映される。概念の真偽判定をコードに追加していない。

### WM-05: World DDLとmigration

- 変更: world/schema.sql、mod、personal_state/schema、persistence/schema、migrationテスト。
- 作業: §6.2のテーブル・索引、§6.3の新triggerを追加。recover前に作成。DB版を追加型で更新。
- 検証: T08。空DB、旧版DB、二回初期化で同じ構造。旧payload・会話件数が不変。
- 完了: 必要な索引をpragmaで確認。旧triggerの上書きに依存しない。

### WM-06: 型付き投影の再構築

- 変更: world/projection、test_support、tests。
- 作業: 履歴＋payloadから有効Entity、Relation、Focusを生成する。読み取り対象をtyped decodeし、壊れたWorld payloadは明示エラー。metaは最後に保存。
- 検証: T09。fixtureから再構築を2回行って同一内容。中間でエラーを注入するとtransaction全体がrollback。
- 完了: 正本のpayloadを更新しない。Worldが0件なら空投影を正常に作る。

### WM-07: 共通commit・rebuildへ接続

- 変更: store、world/validation、world/tests。
- 作業: 共通commitでScope/Source対応とcore検証を実行。既存rebuildからWorld再構築を呼ぶ。Worldのない既存patchもrevision更新に合わせて投影metaを整合させる。
- 検証: T10、T11。不正Worldを直接store::commitへ渡しても拒否。同一patch再送はno-op。途中失敗で履歴・payload・依存・投影・revisionが全て不変。
- 完了: 入口の迂回がなく、書き込みは既存Writer transactionのみ。

### WM-08: 忘却・編集・復元

- 変更: world/tests、必要なprojection/recovery hook。
- 作業: 本文削除→tombstone→依存payload消去→World投影消去→再構築を試す。alias等の派生名称とquery結果も照合。
- 検証: T12、T13。Memory OFF、再起動、古いDB＋現在journalからの復元、Source版編集を含める。
- 完了: 旧payload・名称・関係が復活しない。現在journalがない場合の既存拒否を維持。ここを通過するまで照会実装へ進めない。

### WM-09: Scope付き名称解決と読み取り境界

- 変更: world/query、tests。
- 作業: read transactionで投影meta・AccessRequest・Source/Scope状態を検査。ID/完全一致名/alias解決とpending_reviewを追加。
- 検証: T14、T15。別Project同名、未認可purpose、期限切れ、epoch差、対象Projectの未処理Sourceを入力。
- 完了: Scope外名称を曖昧候補に含めない。Readerからrebuildを呼ばない。

### WM-10: 上限付き関連探索

- 変更: core world/traversal、world_traversal、Adapter query/tests。
- 作業: §7.3の幅優先探索。IOなしの経路展開器と、残予算付きSQL取得を分離する。新しい汎用Graph frameworkは作らない。
- 検証: T16、T17。正規経路、逆向き、循環、diamond、高次数、各上限ちょうど／超過。
- 完了: 返却件数だけでなく実際の候補走査数も制限。入力順を変えても出力順が同じ。

### WM-11: 因果経路と条件

- 変更: traversal、world_traversal。
- 作業: 作用関係だけのforward/reverse経路。Disputed、条件不一致、related_toを除外。元の向きを保つ。
- 検証: T18、T19。important_forを含む関連経路が、因果経路へ混入しない。条件空は未検証表示。
- 完了: 数値効果や新Relationを生成・保存しない。

### WM-12: FocusとResearchGap候補

- 変更: core world/relevance、対応テスト。
- 作業: §7.4の順位、Objective依存、Gap理由・固定質問・安定keyを実装。
- 検証: T20。仮説経路＋current_workはGap、FocusなしはGapなし。Objective撤回でcurrent_work由来のGapを除外。
- 完了: 同じ不足は同じkey。新しいDBテーブル・調査実行を作らない。

### WM-13: WorldSliceとbyte予算

- 変更: core world/model/relevance、Adapter query/tests。
- 作業: Sliceの組み立て、notice、経路単位の削減、最終byte長検査。
- 検証: T21。日本語・escape・長いaliasを含むJSONで8,192 byte以下。経路の端点と根拠が欠けた出力を返さない。
- 完了: budget不足と経路不明が区別され、同じ入力・時刻・revisionで同じ出力。

### WM-14: 初回受入と性能測定

- 変更: 統合テスト、`m1-results.md`、必要な局所修正。
- 作業: §9の基本シナリオを一時DBで実行。0／100／1,000／10,000項目と訂正履歴を含むfixtureを用意し、load・commit・rebuild・queryを別々に測定。
- 検証: T22、§10のK1〜K5。最初に小規模から実行し、測定が過大なら中断理由を残す。
- 完了: 成功数、未実施、追加遅延、走査数、payload量を記録。実測せず性能保証を書かない。

### WM-15: 回帰確認と引き渡し

- 変更: 進捗・結果文書、必要な新規moduleのsize baselineのみ。
- 作業: §10の最終確認を実行。変更ファイルと対象外の未変更を確認。実際の公開範囲、未接続範囲、次段階の課題を残す。
- 完了: §11の全条件を満たす。途中の失敗を隠すためテスト・gate・上限を変更していない。利用者向けV0完了とは報告しない。

## 9. 固定fixtureと期待値

### 9.1 基本fixture

時刻は`T=1_800_000_000_000` ms。Sourceは合成user会話で、同一principalとProject Scope `project:fixture-saaa`へ結ぶ。Source本文、版、digest、範囲は既存sources::loadで取得し、DB保存検証をモックで迂回しない。Objective O1は既存のKind::ObjectiveでActiveにする。

| ID | 内容 |
| --- | --- |
| E1 | concept「Speculative Decodingの導入」、alias「投機的デコードの導入」 |
| E2 | metric「Decode Latency」 |
| E3 | metric「Voice Response Latency」 |
| E4 | project「SAAA」 |
| R1 | E1 decreases E2、effect_input=intervention、model_hypothesis |
| R2 | E2 increases E3、effect_input=quantity_increase、model_hypothesis |
| R3 | E3 important_for E4、effect_input=null、user_statement |
| F1 | E4 current_work、objective_assertion_id=O1 |

R1・R2のconditionsは両方`[["config","fixture-a"]]`。各Relationに支持Source参照を付ける。このデータは実際の性能保証ではない。

patchはO1、Entity群、Relation群、Focusの順に分ける。各patchの16 KiB・32操作上限を実際に検査する。fixtureを一つの巨大patchへ詰めるため制約を緩めない。

期待結果は関連経路E1→E2→E3→E4、因果経路E1→E2→E3。GapはR1とR2に対してhypothesis_unverifiedが各一件。R3について因果Gapは作らない。出力には仮説表示、conditions、Evidence参照がある。

### 9.2 テストID

| ID | ケース | 必須の期待結果 |
| --- | --- | --- |
| T01 | 型・byte上限・不正enum・未知field | 不正値を拒否。2,000 byteを文字数と取り違えない |
| T02 | 正規化・alias・semantic_key | 空白/ASCII大小のみ同一化。related_toの端点反転は同一key、有向edgeは別key |
| T03 | 旧ExtractorとContextへWorldを混在 | Worldを抽出候補として採用せず、current・依存・通常Context・通常snapshotへ漏らさない |
| T04 | 存在しない端点・違うScope・effect_input不正 | 全patch拒否。部分保存なし |
| T05 | World循環と根拠依存循環 | 前者は保存可能、後者は既存Error::Cycle |
| T06 | 同key重複・置換・反証 | 二重Active拒否。置換は旧Superseded、新ActiveまたはDisputed |
| T07 | Entity訂正・期限・Objective撤回 | 依存Relation/Focusを現在の経路へ残さない |
| T08 | 空DB・旧版DB・再migration | 旧会話・payloadを維持。schemaと索引が揃う |
| T09 | 再構築の再現性と失敗 | 2回で同じ内容。失敗時は旧snapshotまたは全rollback |
| T10 | Source・policy・epoch・直接commit迂回 | 不正更新を拒否。Worldのないpatchは既存どおり |
| T11 | 同patch再送／IDだけ同じ異内容 | no-op／Conflict。revision不変／rollback |
| T12 | 忘却とMemory OFF | payload、name、alias、edge、Focusを消去。OFFで抑止されない |
| T13 | Source編集とバックアップ復元 | 旧版を使わない。現在journalなしは拒否、削除済みの復活0件 |
| T14 | Project A/B同名・権限・名称候補 | 許可Projectだけ照会。曖昧一致を勝手に選ばない |
| T15 | meta差、時刻経過、未処理Source | stale/pendingの明示。別Projectのpendingとは区別 |
| T16 | forward/reverse・diamond・cycle | 経路単位で循環を抑止し、別の有効経路は保持 |
| T17 | Node/Edge/path/depth/走査上限 | 各上限を超えず、打切り理由を返す |
| T18 | related_to、important_for、Disputed | 因果経路へ入れない。逆探索でも意味の向きを保存 |
| T19 | 条件一致・不一致・空 | 一致のみ結合。不一致は別、空は未検証 |
| T20 | FocusとGap、時刻変更・再照会 | Objective依存と順位を維持。同じ不足のkeyは安定 |
| T21 | JSON予算・日本語・最小予算 | 構文有効、byte上限内。根拠を欠く経路を返さない |
| T22 | 基本fixture→再送→訂正→再構築→忘却 | §9.1の経路、訂正反映、消去を一連で検証 |

## 10. 検証コマンドと実行時点

リポジトリrootで実行する。以下は既存コマンドである。テストのfilterが0件に一致した場合は成功扱いしない。

```sh
# K1: 純粋コア。coreを変更するカードごと
bun run check:personal-state

# K2: 保存・抽出・忘却の回帰。Adapterを変更するカードごと
cargo test --locked --manifest-path src-tauri/Cargo.toml memory::personal_state

# K3: 既存Contextの回帰。WM-00、02、15
cargo test --locked --manifest-path src-tauri/Cargo.toml runtime::context

# K4: migrationとWriterの回帰。WM-05、08、14
cargo test --locked --manifest-path src-tauri/Cargo.toml persistence::

# K5: サイズと文書。節目とWM-15
bun run size:check
bunx --bun spec-html check ./spec/docs --warnings-as-errors
```

新規Rustテストは`world_`接頭辞またはworldモジュール配下へ置き、個別カードでは対象テストを先に実行する。関連回帰が通った後に同じ試験を理由なく繰り返さない。

WM-15の最終確認は`bun run check:local`と`bun run test:rust-packages`。変更範囲外で既に失敗していた項目はベースラインと照合し、無関係な修正を始めず未解消として報告する。今回の変更による失敗があれば完了にしない。新規moduleのsize登録は既存`size:register`を使い、他作業のファイルや既存baselineの増量が混ざっていないかdiffを確認する。coverage基準を下げない。

この計画書の作成時点では、上記の実装テストは実行していない。実装後の手順と、今回実施した文書チェックを混同しない。

性能測定は固定合成データ・同じbuild profile・同じ端末で実施し、warm-up 5回、測定30回の中央値とp95、最大値を残す。小規模から順に実行し、一回5秒を超えるfixtureはその規模の測定を中断して問題として記録する。5秒は初回の測定中断条件であり、対話用途の合格遅延ではない。本番接続の性能基準はM2/M3着手前に、実測を基に別途固定する。

## 11. WM-M1の完了条件

- WM-00〜15に実施結果があり、T01〜T22の期待結果がテストへ対応している。
- 型付きの構造化入力が既存Source・StatePatch・Writer経由で保存できる。
- 同じ入力・時刻・revisionに対して、基本fixtureの経路とGapが決定的に返る。
- 直接commitの迂回、Scope外参照、忘却後の復活、相関から因果への昇格が0件。
- 読み取りに副作用がなく、上限と打切り理由が実際のSQL取得を含めて検証されている。
- Worldが既存Extractor・通常Context・既存公開snapshotへ混入しない。
- 空DB・旧DB migration・再起動・現在journalを使う復元が検証されている。
- regressionと性能結果、未実施の項目、利用者向け未接続を報告している。

失敗するテストをignoreへ移す、assertを弱める、fixtureから難しい例を削る、既存認可をalways trueにする、新しいWriterで迂回することは完了手段にしない。

## 12. 失敗時の扱いと引き渡し

| 状況 | 対応 |
| --- | --- |
| 通常のコンパイル・対象テスト失敗 | カード内で修正し、失敗した範囲を再検証 |
| 現行コードが計画の接続点と変わっている | 変更の有無を確認し、同じ契約で追従可能なら記録して続行。正本やScope契約が変わるなら再設計 |
| 新規依存、Kind以外の基盤全体改造、別Writerが必要に見える | 実装せず、最小の不足と代替案を記録して計画へ戻す |
| 忘却・Scope・rollbackテストが失敗 | 後続機能を増やさず、その境界を修正。解消できなければ未完了 |
| 10,000項目でload/rebuildが過大 | 測定を残し、製品接続へ進まない。制約を黙って増やさない |
| 外部サービスが必要になった | 初回の境界逸脱。既存fixtureで検証可能な範囲へ戻す |

進捗記録はカードID、status、変更ファイル、実行コマンド、結果、次に開始するカード、未解決事項の7項目に揃える。`planned / in_progress / done / blocked`を使い、実行していない検証をdoneにしない。別タスクへのメッセージ送信を引き渡し手順に含めない。

実装担当へ渡す依頼文の例:

```text
World Model初回実装計画のWM-03だけを実装してください。
共通契約は同計画§5、前提はWM-00〜02の進捗記録を使ってください。
変更はカードに列挙されたcoreの検証とテストに限定してください。
T04・T05と既存core検証を通し、結果をm1-progress.mdへ追記してください。
後続カードを先取りせず、契約変更が必要なら再現例と変更案を記録してください。
```

## 13. 次段階の境界

| 次段階 | 新たに設計してから実装するもの | 開始条件 |
| --- | --- | --- |
| WM-M2: 現在状態とRuntime参照 | P2aのState payload、会議/Task正本との対応、Runtime Source版・終了・取消、request単位Focus | M1完了。会話Source偽装を使わないSource契約と固定fixtureを定義 |
| WM-M3: 構造化抽出とContext接続 | WorldDelta→安定イベント/patch、既存workerのcoverage分離、抽出出典、日本語corpus、Broker候補、generationの依存再検証 | M2完了。World ON/OFFでも単一Runtimeが維持される試験を固定 |
| WM-M4: 限定有効化 | shadow比較、回答品質・遅延・訂正反映の合格値、機能flag、運用診断 | M3のoffline試験完了とlive評価。固定corpusの誤昇格・Scope漏洩・復活0件 |

M2以降の詳細カードは、M1で確定した型・性能・問題点を用いて作る。現在未確定の外部契約を、初回計画へ架空のAPIとして書き込まない。World Model V0の利用者向け完成はM4までとし、本書の完了とは分ける。
