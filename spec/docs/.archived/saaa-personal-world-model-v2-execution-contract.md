# Personal World Model v2 実装契約

改訂日: 2026-09-20。状態: 計画。コード未変更。

[全体計画](saaa-personal-world-model-initial-plan.md)のM1拡張に対する固定仕様である。[詳細カード](saaa-personal-world-model-v2-work-cards.md)は本書のC番号を参照する。本書に示す新規型・関数は実装予定であり、現存APIと混同しない。全体計画と重なる項目は本書を具体的な契約として使い、矛盾があれば実装前に計画を訂正する。

## C0. 現存する接続点と分離方針

rootはリポジトリroot。以下でcoreは `crates/personal-state-core/src/world/`、adapterは `src-tauri/src/memory/personal_state/world/`。

| 現存箇所 | 現存関数・型 | 改訂時の扱い |
| --- | --- | --- |
| core/model.rs | WorldPayload::decode、EntityPayload、RelationPayload、WorldSlice | v1 wire型は保存。v2型をmodel_v2.rsへ追加 |
| core/identity.rs | entity_key、relation_key、focus_key、normalize_name | v1キーと名前正規化は不変。v2キーはidentity_v2.rs |
| core/validation.rs | validate_world_patch、WorldPatchInput、WorldError | v2対応の純粋検証をvalidation_v2.rsへ追加。旧v1試験は残す |
| core/traversal.rs | traverse、conditions_consistent、maximal_only | v1用は保持。v2経路はtraversal_v2.rs。DBアクセスを置かない |
| core/relevance.rs | build_gaps、trim_to_budget | v2はrelevance_v2.rs / slice_v2.rs。旧返却型を無理に拡張しない |
| adapter/validation.rs | load_existing、validate_commit | versioned decoderを経由し、共通commitからv2検証を呼ぶ |
| adapter/projection.rs | rebuild、wipe、insert_entity、insert_relation、insert_focus | v1/v2の共通読取ビューから同じ表へ投影 |
| adapter/query.rs | activate、load_entities、load_edges、load_focus | 旧activateはv1専用を維持。新activate_v2はquery_v2.rs |
| personal_state/store.rs | commit、rebuild | 単一WriterとCASを維持。再送の確認順序だけC5に従い調整 |
| personal_state/schema.rs | migrate、add_column | recoverの前にC6の列を追加 |
| adapter/schema.sql | personal_world_*表・索引・forget trigger | C6で明示する差分だけ |
| personal_state/sources.rs | revalidate | 観測Sourceの版・消去を再検査する既存入口 |

新規ファイルは役割ごとに分け、旧コードの全面移動をしない。core/mod.rsとadapter/mod.rsの登録、および型追加によるimport変更だけは各カードの許可範囲に含む。v1とv2は同じWriter・同じ正本であり、新しい会話Runtimeを作るものではない。

## C1. wire型と内部ビュー

v1の既存struct、enum、schema_version=1の検証は変更しない。EntityKindへGoalを直接追加してv1でも受理する実装を禁止する。新ファイルmodel_v2.rsに、以下の型を追加する。全wire structはdeny_unknown_fields、enumはsnake_case、Optionは常にJSONへnullまたは値を出す。記載したoptional fieldは省略時nullとして受理し、canonical出力では省略しない。

| 新規型 | 全フィールド（型） |
| --- | --- |
| EntityPayloadV2 | type=entity、schema_version=2、entity_id:String、entity_kind:EntityKindV2、name:String、aliases:Vec<String>、objective_assertion_id:Option<String> |
| RelationPayloadV2 | type=relation、schema_version=2、from_entity_id:String、to_entity_id:String、relation_type:RelationTypeV2、effect_input:Option<EffectInput>、conditions:Vec<Condition>、comparison_id:Option<String>、basis:Basis、evidence_stances:Vec<EvidenceStance>、target_direction:Option<TargetDirection>、correlation_sign:Option<CorrelationSign>、epistemic:Epistemic、confidence:Option<Confidence>、strength:Option<CorrelationStrength>、assessment_refs:Vec<SourceKey>、mechanism:MechanismState、outcome_update:Option<OutcomeUpdate> |
| FocusPayloadV2 | type=focus、schema_version=2、entity_id:String、reason:FocusReason、objective_assertion_id:Option<String> |
| Confidence | value:u16、method:ConfidenceMethod |
| CorrelationStrength | magnitude:u16、method:String、population:String、period_start_ms:i64、period_end_ms:i64 |
| OutcomeUpdate | prior_assertion_id:String、outcome_source:SourceKey、prediction_source:SourceKey、comparison_id:String、expected:EffectDirection、actual:EffectDirection、predicted_at_ms:i64、observed_at_ms:i64 |

enum値を固定する。

- EntityKindV2: project / concept / metric / goal / actor。
- RelationTypeV2: 旧6種とcauses / enables / inhibits / has_goal / serves_goal / correlates_with。
- TargetDirection: lower_is_better / higher_is_better。CorrelationSign: positive / negative。
- Epistemic: observation / hypothesis / supported / disputed。
- ConfidenceMethod: manual_v1 / counterevidence_v1。path_rank_v1は返却値専用で保存不可。
- MechanismState: unassessed / missing / described。
- EffectDirection: increase / decrease / unchanged / unknown。経路返却用だけmixedを追加する。

v2の全IDは1〜160 UTF-8 byte。comparison_id、測定method・populationは各1〜96 byte。時刻はUnix ms、区間はstart < end。name / alias / conditions / Evidenceは全体計画の上限を継承する。条件keyは既存check_conditions_structと同じ `[a-z0-9_-]` のみ。相関strengthはmagnitude <= 1000かつmeasurement情報とassessment_refs必須。confidenceもvalue <= 1000、assessment_refsは1〜4件必要。すべてのassessment_refs、prediction_source、outcome_sourceはAssertion.evidenceとinput_dependenciesに含める。全体のSource数4・payload 2,000 byteは緩めない。

goalのみobjective_assertion_id必須、他kindはnull。Focusのreasonにtemporary_attentionを追加しない。request内attentionはC7の入力専用である。

WorldPayloadV2はEntity(EntityPayloadV2) / Relation(RelationPayloadV2) / Focus(FocusPayloadV2)の内部enumとする。JSONではenum名を足さず、各payloadのtypeで区別する。

新規versioned.rsに `VersionedWorldPayload::{V1(WorldPayload), V2(WorldPayloadV2)}` と `decode_versioned(kind_name, value)` を置く。整数schema_versionで一回分岐し、1は既存decoder、2はv2 decoder、その他はInvalidPayload。tag/kind不一致も拒否する。v1読取をv2 payloadへ再保存しない。

内部共通ビュー `WorldEntityView / WorldRelationView / WorldFocusView` はversioned.rsで構築する。旧関係はepistemic=hypothesis、confidence / strength / comparison_id=null、mechanism=unassessedとして読む。元schema_versionと元semantic_keyを保持する。これは説明用の未評価値であり履歴更新ではない。新旧検証は元version別に行う。

## C2. キーと置換

`identity_v2.rs`で以下の配列をserde_json::to_vecし、SHA-256の小文字hexへwm2:を付ける。配列順、null、数値型を変えない。条件はkey昇順の二要素配列。from/toを並べ替えるのはrelated_to / correlates_withのみ。

```text
Entity:   ["entity", project_scope, entity_id]
Focus:    ["focus", project_scope, entity_id, reason]
Relation: ["relation", project_scope, from, to, relation_type,
           effect_input, conditions, comparison_id, target_direction,
           correlation_sign, valid_from, valid_until]
```

同一性は上記配列からversion prefixを除いた内容で判定する。v1 Relationはcomparison_id / target_direction / correlation_signをnullに補って比較する。v1の条件並びはcanonical化する。confidenceや根拠の追加は同一性を変えない。comparison_id変更は別主張なので、旧版を終える場合は明示Retractが必要。

同一性が同じ有効Assertionに新Assertionを足す場合は、同一patch内のSupersede { by: new_id }を必須にする。曖昧に両方残さずworld-duplicate-identityを返す。Entityは同Project・entity_idにつき一つのActiveを維持する。v1/v2混在でも同じ検査を行う。置換後のRelation依存先は新しい端点のAssertion IDを明示する。

Golden試験の入力例:

```json
["relation","project:p","a","b","correlates_with",null,[["config","a"]],null,null,"negative",100,null]
```

この文字列のUTF-8・改行なしに対する固定期待値は `wm2:67f6b1001cb091903064f8b29584817915b309443b6b30559f979ca2c6de9fe0`。計画作成時に独立したSHA-256計算で確認した。テスト内で製品のkey関数を呼んで期待値を作らない。A/Bを反転しても相関keyは同じ、increasesは異なる、confidence変更は同じ、comparison_id変更は異なることを別々にassertする。

## C3. 意味検証とエラー

v2の端点マトリクスは全体計画§5.3を使う。補助3種は異なる既存Entity間で許可し、新しい因果意味を与えない。追加の固定条件:

- target_directionはmetric→goalのserves_goalだけ必須。他はnull。
- correlation_signとstrengthはcorrelates_withだけ使用でき、signは必須。
- comparison_idはincreases / decreasesだけ任意。他はnull。nullでは数段の符号合成を行わない。
- causes / enables / inhibitsと増減関係の新規epistemicはhypothesis。counterevidence更新だけdisputedを許可する。supportedは予約値としてdecodeできても新規保存は拒否する。
- その他の関係はobservation / hypothesis / disputedを許可。ただしdisputed保存には同patchでDispute transitionが必要。
- Activeかつepistemic=disputed、Disputedかつepistemicが別値、という不一致は拒否。生命周期の直接Disputeなら、payload再評価の履歴を伴う置換を使う。v1の旧transition契約は変えない。
- mechanism=missing / describedにはassessment_refsが必要。LLMが説明文を付けただけでdescribedにしない。
- manual_v1は出典付きの構造化評価入力。意味の正しさをRustが証明できるとは扱わない。counterevidence_v1はC10の再計算と一致する場合だけ許可する。

core/validation_v2.rsの入口を `validate_versioned_patch(input: &VersionedPatchInput) -> Result<(), WorldError>` とする。入力は既存WorldPatchInputと同じledger / patch / payloads / existing / project_scope / nowだがpayload mapはVersionedWorldPayload。v1だけのpatchも版横断重複検査を通す。新規Entity・Relation・Goal参照の検査と、既存Worldへのtransitionのみの検査を両方行う。

検証順序を固定する: 要求認可 → 上限 → decoder/構造 → 各Assertion認可 → 有効参照 → 端点マトリクス → 根拠・評価 → semantic_key → patch適用後の同一性 → 容量。新規参照の解決はpatch内の予定状態を含めるが、GoalのObjectiveはこの段階では既存Activeのみを許可する。patch内で新規Objectiveを同時作成しない。

| 条件 | 戻り値 |
| --- | --- |
| wire不正、範囲外、型不適合、未許可method、評価不整合 | world-invalid-payload |
| 件数超過 | world-limit |
| 不在・期限切れ・撤回済み参照、依存ID不足 | world-invalid-reference |
| principal / purpose / classification / Project不許可 | world-scope-denied |
| 版横断の同じ意味の有効項目を無断で重複 | world-duplicate-identity（WorldErrorへ追加） |
| 保存済みpayloadの解読不能、未対応投影値 | world-projection-corrupt |
| 小さいSlice予算 | world-budget-too-small |
| 同patch IDで異なる内容、古いbase_revision | 既存personal-patch-Conflict / personal-patch-StalePatchを維持 |

大文字小文字を含む既存ErrorのDisplay結果はD00でassertして固定する。既存エラーを新しい文字列へ一括変換しない。認可拒否は空Sliceへ変換せずエラーにする。失効した読取項目は除外し、認可済みrequestに対するprojection_staleはnotice付き空Slice。内部破損はnoticeで隠さない。

## C4. 条件・観測の純粋契約

`conditions_v2.rs`に以下を置く。観測と関係の対応は必ずrelation_assertion_idで行う。同じconfigという名前だけで別関係へ流用しない。

```text
ConditionObservation { relation_assertion_id, key, value, evidence: SourceKey,
                       valid_from_ms, valid_until_ms }
AvailabilityObservation { entity_id, value: available|unavailable,
                          evidence: SourceKey, valid_from_ms, valid_until_ms }
ConditionResult { relation_assertion_id, key, expected_value,
                  state: satisfied|violated|unknown, evidence: Vec<SourceKey> }
evaluate_conditions(relation_id, conditions, validated_observations)
    -> { items: Vec<ConditionResult>, aggregate: satisfied|violated|unknown }
evaluate_availability(entity_id, validated_observations)
    -> { state: available|unavailable|unknown, evidence: Vec<SourceKey> }
```

関数はIOなし。AdapterがC7で期限・認可・Sourceを検査した集合だけを受け取る。該当relation/keyの値を重複排除し、0種類または2種類以上ならunknown、一種類で期待と一致ならsatisfied、それ以外はviolated。aggregateはviolatedが一つでもあればviolated、それ以外でunknownまたは条件0件ならunknown、残りはsatisfied。availabilityも0種類・対立ならunknown。一つならその値を返す。未知Nodeの観測を足してNodeを作らない。

具体例: r1のconfig=aに対し、r1/aだけならsatisfied、r1/bだけならviolated、r1/aとr1/bならunknown、r2/aだけならunknown。条件[]はunknown。必要物Bへの観測なしはunknown、B unavailableの有効観測だけならunavailable。

## C5. Writer・再送・忘却

adapter/validation.rs::validate_commitはrequestの認可、active Project、input Sourceの検査後にdecode_versionedとvalidate_versioned_patchを呼ぶ。non-World patchの結果は不変。既存のledgerを読み、型付きの既存payloadを渡す。erased/消去済みの内容を復元するloaderを作らない。

v2新規payload_refは `wm2-payload-`＋canonical JSON bytesのSHA-256 hexとする。canonical bytesは型付きv2 DTOをserde_json::to_vecしたもの（structフィールド順はC1の順、Optionはnullを出力）。Adapterが既存のissued payload登録へこのIDとbyte数を渡す。入力JSONのキー順には依存しない。これにより同じStatePatchの外側にあるpayload mapの差替えも検知する。v1 payload_ref規則は変えない。

store::commitは現在、World検証後にLedger::applyで再送判定している。置換後の旧参照を再検証すると正しい再送まで失敗し得るため、既存applied_patchesのSHA-256 fingerprint確認をWorld意味検証より先に行う。まず要求の認可とProject、およびv2 payloadの内容hashとpayload_refの一致を検査し、適用済みで完全一致なら書込みなしのfalse、同ID異内容ならConflict。未適用だけSource再検査・意味検証・Ledger::applyへ進む。再送用に新規v2のdecoderとhash検査を使えるのはD17時点である。共通の再送判定helperを使い、ledgerのfence/CAS/認可を新規patchで迂回しない。

削除後に旧patchを再送しても、適用済みなら内容を再保存しない。未適用の古いSourceを使うpatchは拒否。すべて呼出元のWriter transaction内。fault注入でpayload保存・transition保存・投影再構築のどこで失敗してもrevisionと件数が戻ることを確認する。

## C6. DDLと投影

現行World表はkind / relation_typeをTEXTで保持し、新しい型を追加するCHECKはない。新関係型ごとの表やGoal本文列は不要。必要なDDL変更は投影形式の版だけとする。

```sql
ALTER TABLE personal_world_projection_meta
  ADD COLUMN projection_version INTEGER NOT NULL DEFAULT 1;
```

新規DB用のadapter/schema.sqlにも同列を追加する。既存DBはpersonal_state/schema.rs::add_columnで存在確認して追加し、recover前に完了させる。再構築のmeta INSERTは列名を明示しprojection_version=2を記録する。旧v1専用Readerはversion 2に対してprojection_staleとして省略し、五要素を無理に読み込まない。activate_v2だけv1/v2混在投影を読む。M1では旧Readerの本番接続はないため、旧APIを通じて新kindを誤解釈する経路を残さない。

v1用のprojection_version=1を直接作る旧単体fixtureは保持できる。製品rebuild後のintegration試験はactivate_v2へ移し、同じ旧データの意味を保つ期待値を残す。

現行全体DB版は21。着手時も21なら22、進んでいれば最新＋1をD00で確定する。他作業のmigration番号を奪わない。既存索引とforget triggerはそのまま使う。新しい派生表を足さないためtrigger対象の追加は不要。Goal参照は2,000 byte以内のpayloadから取得する。

rebuildは消去→有効Entity→有効Relation→有効Focus→metaの順。Goal参照のObjective、Relation端点の依存失効をledger.statusで確認する。未知kindをConceptへ黙ってfallbackするparse_kindをv2で使わない。読取時にも時間・Source・Objectiveを再検査する。rebuild時刻で固定しない。

## C7. 内部照会APIと取得順

adapter/query_v2.rsに `activate_v2(c: &Connection, input: &ActivateInputV2) -> Result<WorldSliceV2, String>` を追加する。内部専用でIPC/HTTP/MCPへ公開しない。

ActivateInputV2は旧入力のproject_scope / access / now / seeds / causal_direction / limits / max_bytesの役割を継承し、limitsは新規LimitsV2 { causal_depth, relevance_depth, nodes, edges, paths, fetch_rows, scan_steps }（すべてusize、上限は本節）とする。ほかにrequest_id:String、explicit_question:bool、flags:IncludeFlags、condition_observations、availability_observations、temporary_attention_entity_idsを追加。観測は二配列合計30件、attention最大4 ID。flagsはcausal / goals / correlations / dependenciesのboolで既定すべてtrue。重複seedは正規化後排除、最大4件。

有効期間はfrom <= now < until、終了なしは無期限。観測のSourceはledger上の存在・access.permits・source_valid・sources::revalidate・Project mappingを検査する。scope不正は要求エラー、不在/旧版/期限切れ観測は除外してobservation_unavailable notice。任意のクライアント入力を「検証済み」と扱わない。M1の呼出元は内部fixtureだけ。

手順: (1)要求認可とProject (2)meta/version/revision/epoch/policy (3)許可され現在有効なEntity・Goal (4)seed解決 (5)観測検証 (6)隣接取得と経路評価 (7)到達Focus/Goal選択 (8)Gap (9)説明単位選択と整形。途中でDB修復しない。同じread transactionで実行する。

取得はBFSのfrontierから行い、各depthでfrom側・to側を同じ残取得予算で読む。WHEREはProject・現在有効期間・frontier端点、ORDER BY assertion_id、LIMIT remaining_fetchを明示する。fetch_rowsはSQLが返した件数、scan_stepsは展開で検査したedge数。両方最大500で、破棄・重複行も取得費用に数える。LIMIT+1の隠れた取得をしない。上限ちょうどなら保守的にtruncated:fetchを付ける。残数0ではSQLを発行しない。

取得候補は認可・依存・flag・型を確認してからfrontierに加える。循環防止は経路単位のvisited、SQL再取得防止は別のexpanded集合。探索途中のprefixを返却経路数に数えない。無限にprefixを蓄積せずscan_stepsで停止する。原始取得は双方向でも、因果展開は指定方向だけ。

因果の深さは3、関連経路は4。Node 30 / Edge 60 / 経路合計10 / fetch500 / scan500 / byte8192はSlice全体で共有する。関連4段は技術→指標→指標→Goal←Projectに必要な変更であり、因果深さは引き上げない。要求側は各上限を小さくできる。深さ0ならseed説明のみ。因果/関連のスケジュールは各depthで因果一展開、関連一展開の交互、各queueはseed ID・edge ID順とし、共有予算の二重初期化を禁止する。

以下の読み取りfield型は、ID/name/noticeがString、IDリストがVec<String>、revisionがu64、as_of_msがi64、hops/path_indicesがusize、状態・種類は対応enum、根拠はVec<SliceEvidence>、confidence/参照のnullableはOptionとする。永続DTOのSourceKeyをそのまま本文に展開しない。

flags=falseの種類は候補経路から除外する。goals=falseならgoal node/has_goal/serves_goalと目標由来のGapを返さない。causal=falseなら作用関係を関連経路にも用いない。省略理由はdisabled:* noticeで示し、非表示を知識不足へ変換しない。

## C8. 経路評価とSlice

traversal_v2.rsのevaluate_pathは作用の本来のfrom→to順で評価する。reverse探索の表示順と区別する。数値合成の条件は、すべて増減関係、全条件satisfied、全edgeの非null comparison_idが一致、中間接続が同じmetric IDであること。先頭concept interventionは許可するが、途中にconceptを挟む経路は合成しない。条件・基準が違えばunknown。

方向はdecreasesの個数が奇数ならdecrease、偶数ならincrease。causes等はunknown。ranking scoreは方向を合成できる経路だけ `min × 9^(h−1) / 10^(h−1)` をu64で計算し最後に一回整数除算。h=1で減衰なし。例: 900/800/700の3 hopは567。どれかnullならnull。複数経路のmixedは同じ始終点とcomparison_idごとの集約結果に置き、個別経路の方向は消さない。打切りがある集約にはcomplete=false。

Sliceは新規slice_v2.rsに定義し、JSONキーはsnake_case。既存WorldSliceを破壊的に変更しない。

| 項目 | 具体的な内容 |
| --- | --- |
| envelope | schema_version=2、revision、as_of_ms、nodes、relations、focus、relevance_paths、causal_paths、effect_summaries、relevant_goal_ids、relevant_project_ids、correlation_ids、dependencies、research_gaps、notices、truncated |
| node | entity_id、entity_kind、name、assertion_id、objective_assertion_id（nullable） |
| relation | assertion_id、semantic_key、from_entity_id / to_entity_id、relation_type、lifecycle、epistemic、basis、target_direction、correlation_sign、strength、confidence、comparison_id、condition_results、condition_state、evidence |
| causal_path | node_ids、steps（assertion_id:Stringとtraversed_reverse:bool）、hops、derived=true、direction、confidence（path_rank_v1またはnull）、condition_state、truncated |
| relevance_path | node_ids、steps、focus_entity_id、goal_id（nullable）、condition_state、truncated |
| dependency | relation_id、required_entity_id、availability、evidence |
| effect_summary | from_entity_id / to_entity_id、comparison_id、direction、path_indices、complete |
| evidence | SourceKeyとstance。観測由来はcontext。名前・本文を複製しない |

条件と根拠を削ってbyteを節約しない。候補は「一つの経路＋終点Focus＋必要Node/Relation/Goal/根拠＋そのGap」、または「単独相関/依存＋両端＋根拠＋そのGap」を説明単位にする。未知seed Gapは独立単位。順位はFocus区分、seedからの距離、種類（関連→因果→相関→依存→未知）、安定IDの辞書順。scoreは同順位で安定IDより前の比較だけに使い、nullは評価済みより後。

各単位を順位順に仮追加し、参照閉包をunionして件数・JSON長を測る。収まらない単位は全体を採用せず次へ進みtruncated:budgetを付ける。notice自体のbyteも計数する。採用後の要素を無関係にpopしない。経路合計10件、全Node30、全Edge60を最後にもassertする。何も入らなくてもnotice付き空envelopeが予算内なら返す。それも入らなければBudgetTooSmall。max_bytes=1は必ずエラー。

## C9. GapとFocus

relevance_v2.rsにbuild_gap_candidatesを置く。入力は採用候補の経路と終点Focusの組、評価済み関係、explicit_question、seed解決結果。Project内の全Focus×全edgeを作らない。

| kind | 発生条件 |
| --- | --- |
| missing_knowledge | 明示質問のseedが許可範囲内で未解決。認可拒否・曖昧seed・打切りでは生成しない |
| unknown_causal_direction | 関連Focusへ到達する経路上のcorrelates_with。相関を因果へ変更しない |
| missing_mechanism | 到達経路上の関係にmechanism=missingと根拠がある |
| low_confidence_relation | 到達経路上のconfidenceが0〜499。nullと500は対象外 |
| missing_condition | 到達経路上のunknown条件、unknown availability、または比較不能Outcome |
| conflicting_evidence | disputed、対立する条件観測、または比較可能なOutcome反証 |

Gapはkind、subject（entity_idsとrelation_ids、またはunresolved_seedの排他的選択）、goal_id nullable、focus_entity_id nullable、question、keyを持つ。架空の関係IDを空文字で入れない。missing_knowledge以外は対象Relationと根拠を保持する。questionはkindごとの固定日本語テンプレート＋許可済み表示名で作り、LLMは呼ばない。

keyは `wmg2:`＋SHA-256(JSON配列 `[project,kind,sorted_entity_ids,sorted_relation_ids,normalized_seed_or_null,sorted_conditions,goal_id_or_null]`)。同じ不足を異なる探索順で重複生成しない。Gap種別順はconflicting_evidence、missing_condition、unknown_causal_direction、missing_mechanism、low_confidence_relation、missing_knowledge。

temporary_attentionはrequest_idと入力Entity IDだけから一時生成する。DBへ書かず、次requestで省略されれば消える。未許可・未解決Entityは採用しない。current_workのObjectiveが失効したとき、explicit_interestへ降格して延命しない。

## C10. Prediction / Outcome更新

core/outcome_v2.rsにcompare_outcomeを置く。入力は旧Relation Assertion ID、旧Relation payload、Prediction、Outcome。保存するOutcomeUpdateのprediction_source / outcome_sourceもこれらの入力と一致させる。PredictionとOutcomeはmetric_id、comparison_id、sorted_conditions、direction、時刻、SourceKeyを持つ。metric_idは対象Relationの終点metric。comparison_idと非空条件が完全一致し、観測時刻が予測時刻以後で、旧関係の適用期間内なら比較可能。その他はmissing_conditionのみ、score変更なし。directionがunknown / unchangedなら反証確定しない。

期待increase・実測decrease、または期待decrease・実測increaseだけを反証とする。同方向なら変更なし。増減関係以外のOutcome更新はInvalidPayload。反証時は元Relationをコピーし、epistemic=disputed、challenges根拠とOutcomeUpdateを追加。旧scoreが800なら640、799なら639、nullならnull。counterevidence_v1とOutcomeUpdateを検証側でも再計算する。Source4件・Evidence4件・payload上限を超えればLimitで停止し、古い根拠を削って収めない。

adapter/outcome_v2.rs::prepare_outcome_patchは現在の旧Relation版がActiveであることを確認し、一つの新Assertionと次のtransition列を既存StatePatchへ組み立てる。

```text
旧Relation: Supersede { by: new_id }
新Relation: Assert → Activate → Dispute
```

新Relationのdepends_onは有効な端点等へ向け、Supersededになる旧Relationを追加しない。旧版の情報を生成へ使った全Source依存を新assertionへ引き継ぎ、Outcome Sourceをunionする。旧Relation IDはOutcomeUpdateの由来参照に残す。旧版を根拠依存にすると即Invalidatedになるため禁止する。

operation keyは `wmo2:`＋SHA-256(JSON配列 `[project,prior_assertion_id,outcome_source.id,outcome_source.version]`)。一つのSource版から同一RelationへのOutcomeを二つ生成する場合は拒否し、Sourceを分ける。prepared patchには確定したbase_revision、入力版、ID、sequence、recorded_at、payload bytes、fingerprintを保持し、再送は同じprepared内容を送る。呼び直すたびにtimestampやIDを作り直さない。

再試行時にoperation keyが適用済みなら再評価・再減衰せずno-op。同keyの異なる入力はConflict。未適用でbase_revisionまたは旧Relation版が変わっていればStalePatch、勝手に新Relationへ付け替えない。二つのOutcomeが同じ旧版を使えば最初だけ成功する。新たな比較は明示的に新しい予測・対象版で行う。

新しい永続operationテーブルは作らず、operation keyをpatch IDとして既存applied_patchesを使う。issued_patch_idは信頼済みAdapterが同じIDを登録する。通常Writerに渡すだけで、LLMからID・fenceを受け取らない。

## C11. 固定検証データと実行規則

共通fixtureはprincipal=owner、Project=project:p、now=100、Source=s1/version1、config=a、有効期間[0,200)。GoalのObjective=o1、Goal Entity=g1、Project Entity=p1。負例はprincipal=other、Project=project:q、now=200を使う。認可fixtureのpurpose/classificationは既存AccessRequest型の実値を使う。

v1 fixtureをコピーしてv2の期待値だけ書き換えるのではなく、実際のWriter経由で作成する。単体fixtureと統合fixtureは別の試験である。次の具体例を必須にする。

- aliasの先頭一致でなく完全一致、同名別Projectの漏洩0件。
- Node上限2、未接続Focus10件でも返却Node<=2。
- 関係500件以上でseed隣接・二段目到達を確認。取得500件・展開500回の実カウンタをassert。
- a→b条件成立、b→c不成立ならa→bが残る。条件が同じだが観測なしならunknown。
- 3 hopの900/800/700は567。異なるcomparison_idならunknown/null。
- 技術→指標→指標→Goal←Projectは関連4 hop、因果2 hop。関連深さ3を要求した場合は打切りであり「無関係」としない。
- source削除後のrebuild・restart・古いDB復元でGoal名・条件・評価が復活しない。
- Outcome再送は640のまま、revision不変。新しいIDの同意味反証を無条件に通さない。

製品コード変更はカードで指示された範囲のみ。これらのテストは後続実装で追加・実行するもので、計画作成時点で成功したとは扱わない。
