# SAAA Personal World Model 実装計画 — 5要素改訂版

作成日: 2026-09-19。改訂日: 2026-09-20。

状態: 実装担当へ渡す改訂計画。今回の変更は文書のみ。上位文書は[Personal World Model Concept](saaa-personal-world-model-concept.md)。

本書は旧WM-M1計画を置き換える。旧計画は `spec/evidence/world-model/m1-plan-original-2026-09-19.md` に保存し、WM-00〜15 / T01〜22の過去記録を読むためだけに使う。W2番号は到達点のまとまりとし、今後の実装指示には詳細カードのD番号を使う。旧計画の「関係型は6種」「confidenceは対象外」を新しい契約へ持ち込まない。

次段階: 五要素の実装報告を受けた[M2実装計画](saaa-personal-world-model-m2-plan.md)。本書の保存形式・五要素の意味を維持し、Runtimeの現在状態への参照を追加する。

## この計画の読み方

本書は全体の範囲・段階・受入条件を管理する。DeepSeek等へ渡す実装単位は、次の二文書で固定する。

- [実装契約](saaa-personal-world-model-v2-execution-contract.md): wire型、関数境界、キー配列、検証順、DDL、探索・条件・整形、エラー、Outcome再送の仕様。
- [詳細作業カード](saaa-personal-world-model-v2-work-cards.md): 一責務・一レイヤーを基本とするD00〜D42の43枚。対象ファイル、指定契約、操作、具体的期待値、試験laneを記載。

W2-00〜20の表をそのまま一括実装させない。Dカード一枚と指定C節、直前の結果だけを渡す。設計選択を実装AIに委ねる「適切に実装」等の指示を避ける。上限や推論規則の変更は、本書と契約を先に整合させる。

## 1. 到達点と段階

既存Personal Stateの履歴・単一Writer・World投影を維持して、因果・影響、目標、条件、相関、依存を扱えるようにする。再利用する理解を保存し、細かな推論は必要時に行う。Graphを埋めること自体は成功条件にしない。

| 段階 | 到達点 | 本書の粒度 |
| --- | --- | --- |
| M1再確認 | 既存実装とレビュー修正の確認、残る復元・予算境界の補強 | W2-00〜03で詳細化 |
| M1拡張 | モデルなしの構造化入力で五要素を保存・照会し、訂正・失効・忘却を守る | W2-04〜20で詳細化 |
| M2 | Runtime / 外部EvidenceのSource契約、会議・Task参照、現在状態の接続 | §11の設計ゲート後に詳細カード化 |
| M3 | LLM候補抽出、根拠取得、必要時推論、共通Brokerとgeneration依存検証 | M2の契約・実測後に詳細カード化 |
| M4 | shadow比較と限定有効化 | M3の評価基準を固定してから実施 |

今回の実装単位はM1再確認とM1拡張である。利用者向けV0完成はM4までとし、手入力Graphの探索成功で完了扱いしない。M2以降の架空APIや未確認Sourceを先取りして実装しない。

## 2. 現在地と再確認するもの

2026-09-20のコードには `crates/personal-state-core/src/world/` と `src-tauri/src/memory/personal_state/world/` が存在する。Entityはproject / concept / metric、Relationはrelated_to / part_of / depends_on / important_for / increases / decreases、条件はkey / value、Focusはcurrent_work / explicit_interestである。新規モジュール一式を作り直さない。

`spec/evidence/world-model/m1-results.md` と `m1-review-2026-09-20.md` の追記ではR1〜R10修正と回帰試験の成功が報告されている。本書作成時には試験を再実行していないため、独立した再認定とはしない。既存のT13はSource編集の確認であり、Worldを含む旧DBを現在journalで復元する確認とは分ける。10,000項目の性能は未認定である。

| レビュー事項 | 入口で確認する期待結果 |
| --- | --- |
| R1 認可、R2 失効 | principal / purpose / classification / Project失効と、時刻のみ進めたEntity / Objective / Sourceの失効を名称解決前から反映 |
| R3 取得予算 | 500件以上のProjectでもseed近傍を探索でき、SQL取得と探索それぞれが上限内 |
| R4 逆探索、R5 条件 | causal reverseは原因側のみ。不成立の長い経路が有効な短い経路を消さない |
| R6 Gap、R7 Focus | 無関係なFocusとの直積を作らず、Focusも最終Node上限内 |
| R8 byte、R9 参照整合性 | 要求byte数を増やさず、削減後もFocus・Gap・経路の参照先が存在 |
| R10 経路上限 | 中間prefixで返却枠を使い切らず、予算内の到達経路を返す |

## 3. 実装担当の進め方

一度に一枚のDカードを実行する。標準粒度は実装1〜3ファイルとテスト1〜2ファイル。一枚が実装5ファイル超、または独立した設計判断二つ以上になる場合は、先にカードを枝番へ分割する。module宣言やfixture登録の機械的変更は目安から除く。

各カードの入力は本書の指定契約、直前の進捗記録、固定fixture。出力は対象コード、期待結果をassertするテスト、結果記録である。全カード共通で、対象外レイヤーの変更、新依存・新Writer・新サービス、後続カードの先取りを禁止する。private helper名や同じ契約内の関数分割は担当者が決めてよい。

失敗時は再現を残して担当範囲で修正する。型・正本・認可・意味・上限・合格条件を変える必要がある場合だけ、そのカードを止めて変更案を記録する。通常の修正に毎回ユーザー確認を挟まない。失敗試験のignore化、assertの弱化、fixture削除、coverage引下げで通過させない。

新しい進捗は `spec/evidence/world-model/v2-progress.md`、結果は `v2-results.md` に記録する。旧M1のdoneを新カードへ転記しない。各行にカードID、planned / in_progress / done / blocked、変更ファイル、実行コマンドと件数、期待結果との比較、未解決事項、次カードを残す。

## 4. 全段階で維持する境界

- 意味状態の正本は既存Assertion / Transition。Graphは再構築可能な投影。更新は既存StatePatchと同じWriter transactionで行う。
- primary ledger、既存AccessScope、activeな明示Project、全入力Sourceへの依存を維持する。Project名やLLM出力を認可情報にしない。
- Goalの採用・撤回は既存Objective等が所有する。World Goalは版付き参照。Task状態や調査キューをWorldで二重管理しない。
- Worldの循環と、Assertion.depends_onの非循環な根拠依存を区別する。
- 読み取りは副作用なし。ネットワーク、LLM、投影修復、永続化を行わない。
- Source本文・alias・条件・Goal参照・評価も訂正・失効・忘却の対象。派生経路から削除済み情報を復活させない。
- Runtime・外部資料IDを会話Sourceに偽装しない。M1拡張は合成会話Sourceと同Projectの既存Objectiveを使う。

## 5. M1拡張のデータ契約

### 5.1 版と互換性

payload schema_version=2を追加し、1と2のdecoderを分ける。既存Kind WorldEntity / WorldRelation / WorldFocusを継続し、v1 JSONの形・キー計算を変えない。未知version・未知field・Kindとtagの不一致は拒否する。旧履歴の一括書換えはしない。

新規v2のsemantic_keyはwm2接頭辞とSHA-256を使う。固定順序のJSON配列で、EntityはProjectとentity_id、FocusはProject・対象・理由、RelationはProject・端点・型・effect_input・正規化条件・comparison_id・目標方向・相関符号・有効期間を含める。Evidence、confidence、表示名は含めない。related_toとcorrelates_withのみ端点を辞書順へ揃える。ハッシュ前の配列は実装契約C2で固定済み。期待hashは独立計算でfixtureに固定する。

v1/v2間の同じ意味の二重Activeをprefix差で許さない。Writerでversion非依存の論理同一性も検査し、既存IDを明示したSupersede＋新Assertで更新する。互換読取時は旧important_forをGoalに、related_toを相関に変換しない。旧confidenceは未評価、旧条件の成立は不明。v1を再送した際の既存no-op / Conflictも維持する。

### 5.2 対象・参照

Entityはproject / concept / metricにgoal / actorを追加する。nameはtrim後1〜160 UTF-8 byte、alias最大4件・各160 byte。ID発行は信頼済みAdapterが行う。名前の正規化は旧契約を維持し、曖昧名を勝手に統合しない。

goalには同Projectの既存Objective Assertion IDを必須とし、参照先がActiveかつ許可・期限内であることをcommitとqueryで検査する。Goal EntityはObjectiveに根拠依存を持ち、参照先の訂正で失効したら自動付替えしない。actorは会話根拠付きの主体識別のみとし、ユーザー権限を生成しない。推定した他者の目標をユーザーObjectiveへ登録する機能、runtime_refと現在状態の新payloadはM2以降で扱う。

### 5.3 関係と型の適合

| 関係 | 端点と追加契約 |
| --- | --- |
| increases / decreases | 始点metricならquantity_increase、conceptならintervention。終点metric。介入内容は名称と出典で明示 |
| causes / enables / inhibits | 始点conceptのintervention、終点conceptの出来事・行為・状態。増減符号はunknown |
| has_goal | projectまたはactor → goal。effect_inputなし |
| serves_goal | conceptまたはmetric → goal。metricの場合target_direction=lower_is_better / higher_is_betterを必須。conceptはnull |
| correlates_with | metric ↔ metric。positive / negativeを必須。effect_inputなし。自己関係は拒否 |
| depends_on | goal以外の対象間。始点が終点を必要とする。effect_inputなし |
| related_to / part_of / important_for | 旧契約の補助関係。因果伝播へ入れない |

これはM1拡張で検証可能な端点に絞った契約であり、より広い自然言語を自動解釈する規則ではない。全Relationは端点の現在Assertionへ依存する。disputedなRelationは関連説明と反証表示へ使えても、有効な因果方向の合成には使わない。

### 5.4 条件と依存先の状態

保存条件は既存key / value最大4組を維持する。keyは1〜32 ASCII byte、valueは1〜96 UTF-8 byte、key整列・重複拒否。自由文条件はvalueに保存できるが、V0で自然文評価器を作らない。

条件評価は照会入力の、信頼済みAdapterが根拠・Scope・時刻を検証した有限の観測集合で行う。観測はrelation_assertion_idに紐付け、同じkeyでも別関係へ流用しない。同じkeyとvalueの観測があればsatisfied、同じkeyに異なる一意な値があればviolated、未観測または競合はunknown。条件0件も適用条件未指定としてunknown。単に二つのedgeの条件が一致するだけではsatisfiedにしない。M1では保存済み会話Sourceを参照する固定観測fixtureを使い、LLM出力を直接観測集合へ入れない。

依存先も検証済みのavailability観測からavailable / unavailable / unknownを返す。Nodeがあるだけではavailable、Nodeがないだけではunavailableにしない。availabilityが未指定・失効・競合ならunknown。この観測入力は照会用であり、M1で新しい可変状態台帳を作らない。観測参照もSliceの依存情報とbyte予算に含める。

### 5.5 根拠・評価・予測と結果

生命周期と根拠の評価区分を分離する。評価区分はobservation / hypothesis / supported / disputed。因果・影響はhypothesisから開始し、この段階ではsupportedへの自動遷移を実装しない。明示発言のbasisはユーザーSourceを必要とし、発言があるだけで因果を実証済みにしない。

confidenceはnullまたは0〜1000の整数（千分率）とmethodの組にする。真実の確率とは呼ばない。M1の初期評価は信頼済み評価入力のmanual_v1だけとし、根拠参照を必須にする。LLM任意値や資料件数から生成しない。strengthは相関のみ、optionalな0〜1000の絶対係数と測定方法・対象・期間の組とする。方法を解釈して統計検定する実装は含めず、未測定ならnull。符号はpositive / negative側に置く。

経路評価は全edgeが評価済み、増減型のみ、同じ非null comparison_idかつ全条件成立の場合のみ `floor(min(confidence) × 0.9^(hops−1))`、method_version=path_rank_v1。整数演算で最後に一回切り捨てる。unknown条件または未評価edgeを含む経路はconfidence=null。これはランキング指標であり確率・効果量ではない。

Prediction / Outcome比較は純粋関数として追加し、独立したPrediction DBは作らない。入力は関係版、指標ID、条件の正規化表現、比較基準ID、期待方向、実測方向、Source参照。比較条件の一致は信頼済みAdapterが確認した入力に限る。不一致・不明ならmissing_condition、同条件で方向が逆なら反証・disputedとconflicting_evidenceを提案する。評価済みconfidenceは `floor(old × 0.8)` としmethod_version=counterevidence_v1、未評価はnullのまま。成果は候補patchであり純粋関数は保存しない。

同一Outcome Source版・関係版のキーで安定patch IDをAdapterが作り、同じ反証を二重適用しない。二件の結果が同じ旧版を基にした場合はCAS競合を返し、古いscoreで上書きしない。新しい根拠で再評価する手順を結果に残す。

## 6. 保存・照会と予算

既存commitのWorld検証hookへ拡張を入れる。別入口だけで検証しない。型・Source・認可・Goal参照・端点・条件・評価の検証、Ledger適用、履歴保存、CAS、投影再構築を同一transaction内で行う。直接commitの不正v2も拒否し、失敗時は全rollbackする。

投影はpersonal_world接頭辞の既存表と索引を再利用する。DDL差分は実装契約C6のprojection_version列だけとする。本文は消去可能payloadを正本とし、新規可変Relation台帳は作らない。schema migrationは着手時の最新DB版から追加し、既存migrationを書き換えない。DDLとforget triggerの準備はrecoverより前。旧DB、空DB、v1/v2混在、再構築で同じ意味が得られることを確認する。

| 上限 | 固定値 |
| --- | --- |
| 新規World assertion | patchあたり8件、既存32操作・16 KiB以内 |
| payload | JSON 2,000 byte以内。超過は拒否、黙って短縮しない |
| 入力Source | patchあたり最大4件。全入力依存を含む |
| 有効保存量 | Candidate / Active / Disputed計10,000件。置換旧版を差し引き、撤回・忘却を妨げない |
| 探索 | 因果深さ3、関連深さ4、Node 30、Edge 60、因果＋関連の返却経路計10 |
| 検索・探索費用 | 関係候補のSQL取得500件、展開時の隣接検査500回を独立計数。Focus取得30件、観測入力30件まで |
| Slice | JSON 8,192 byte以下。要求側が小さくした上限を増やさない |

全Snapshot読込・根拠閉包確認・ledger loadの費用はこの探索予算で制限できたとは扱わず、件数と性能を別に測る。seedからの隣接取得と安定ID順を優先し、Project全体の先頭500件だけでseed近傍を代表させない。予算不足は打切り理由を返す。最低限の説明が収まらなければworld-budget-too-smallとする。

因果forwardは作用のfrom→to、reverseはto→fromのみ。逆探索でも元のedge方向を返す。関連探索はGoal所有者へ逆に辿れるが因果へ混ぜない。条件不成立は展開から外し、不明は未検証の説明候補に残す。増減のみで比較可能かつ全条件成立の経路だけ符号合成する。causes等を含む場合unknown、同じ始終点・同じ比較条件の有効経路が対立する場合mixed。打切り時は全経路を網羅した結論にしない。

返却はWorldSliceV2とし、JSONフィールド名は実装契約C8で固定する。旧WorldSliceを破壊的に変更しない。causal / goals / correlations / dependenciesのflagsを内部入力へ追加し、条件は対象関係と常に一緒に返す。省略flagで根拠や条件だけを隠せない。FocusとGoalも全体予算に含め、説明単位でtrimし参照閉包を検査する。

## 7. Gap・Focus・必要時推論の境界

Gapはmissing_knowledge / unknown_causal_direction / missing_mechanism / low_confidence_relation / missing_condition / conflicting_evidenceの六種。低評価は評価済みconfidence < 500に限定し、nullを低評価に変換しない。mechanism不足は信頼済みの構造化評価入力で明示された場合だけ候補化し、Graphの形から無条件に推測しない。

Gapの対象は現在の仕事・明示関心へ実際に到達する経路、または明示的に判断を求められた未知Topic。名称未解決時はEntityを捏造せず照会seedを指す一時候補とする。認可拒否や探索打切りはmissing_knowledgeの証拠にしない。候補キーはProject・対象IDまたは正規化seed・条件・不足種別・Goal参照から安定化する。queued / resolved等の管理はTask側とし、この段階では候補返却のみ。

Focusはcurrent_work / explicit_interestを維持し、temporary_attentionはrequest内のみ追加する。current_workは有効Objective参照が必要。順位はcurrent_work、explicit_interest、temporary_attention、距離、安定ID。active_project / current_research / long_term_interestの拡張はM2で根拠と失効を決め、旧reasonから無条件変換しない。

必要時推論はM3で、保存済みSliceと許可されたKnowledgeを入力に行う。出力はそのTurnの説明であり自動保存しない。再利用価値と根拠が確認できた場合だけ通常のWorldDelta候補へ戻す。M1の純粋探索にLLMを呼ぶ実装を足さない。

## 8. 到達点の一覧と詳細カードへの対応

以下はW2到達点の一覧であり、一枚の実装指示ではない。実行順は詳細カードD00〜D42に従う。前提が満たされても、直前カードの失敗を放置して後続へ進めない。全カードに§3の入力・出力・禁止事項・失敗時手順を適用する。coreは `crates/personal-state-core/src/world/`、adapterは `src-tauri/src/memory/personal_state/world/` を指す。V01〜V21は全体の検証観点として残し、実行試験はDカードのdNN_接頭辞を使う。旧T番号を再利用しない。

| カード・前提 | 対象と手順 | 合格条件・試験 |
| --- | --- | --- |
| W2-00 / なし | 現行HEAD・差分・型・DB版・旧結果を記録。§10のK1〜K4をbaseline実行。新進捗表を作成 | V01: 成功/失敗/未実施と件数を記録。既存失敗と今回原因を区別 |
| W2-01 / 00 | adapter認可・現在性のR1/R2試験を確認。Source依存閉包とGoalに使うObjectiveの期限も補強 | V02: 別principal、用途、機密区分、失効Project、時刻のみの失効で名称・alias・Focus・経路を返さない |
| W2-02 / 01 | core探索とadapter予算のR3〜R10を再確認。不足ケースだけ修正 | V03: 500件、逆探索、条件prefix、無関係Focus、小byte、10分岐二段経路で§2を満たす |
| W2-03 / 02 | 既存復元試験にWorldを含む専用fixtureを追加。新機能を混ぜない | V04: 古いDB＋現在journalでWorldの名称・alias・関係が復活しない。journal欠落は拒否 |
| W2-04 / 03 | coreのv1/v2 DTO・decoder・schema・keyを追加。保存変更はまだ行わない | V05: v1 golden不変、v2往復、未知field/version拒否、対称関係と版横断同一性のgolden |
| W2-05 / 04 | core EntityとGoal参照の意味検証、adapterでObjectiveを解決 | V06: 同ProjectのActive Objectiveだけ受理。別Project・取消・期限切れ・別kindを拒否 |
| W2-06 / 05 | core Relationの§5.3端点マトリクスと評価DTO検証 | V07: 全関係の正例と不適合例。相関符号、metric目標方向、評価範囲・根拠・方法版の検査 |
| W2-07 / 06 | core条件・availability評価の純粋関数 | V08: 成立/不成立/不明、空条件、競合、順序違い、依存先不在情報と実際の欠落の区別 |
| W2-08 / 07 | adapter codecと共通commit hookをv2対応。観測入力のSource・Scope検証を追加 | V09: 不正直接commit拒否、version横断重複拒否、明示置換成功、同patch no-op、失敗全rollback |
| W2-09 / 08 | migrationと投影再構築を拡張。意味を変えるデータ変換はしない | V10: 空DB/旧DB/混在/再migration/再構築で同じ結果。Goal依存失効とforget triggerも通る |
| W2-10 / 09 | queryでv2の認可・時刻・Source・Goal・観測を再検証 | V11: 時刻だけ進めても全五要素が失効。scope外の存在も漏れず、Reader書込み0件 |
| W2-11 / 10 | coreの因果経路・符号合成・path_rank_v1を実装 | V12: +−+は減少、追加作用型はunknown、未評価/不明条件はscore null、逆探索と循環・打切り表示 |
| W2-12 / 11 | core関連探索にGoal経路、相関、依存の専用結果を追加 | V13: metric→Goal←Projectが関連として辿れ、相関と依存から因果を生成しない |
| W2-13 / 12 | core Gapの六種とFocusの経路対応、request内attention | V14: 関連する不足だけ返す。未知seedに架空edgeなし。request終了でattention消失 |
| W2-14 / 13 | adapter Sliceへ五要素を統合し、説明単位trimとflagを追加 | V15: 全体件数・SQL取得・byte上限、参照閉包、条件保持、小予算拒否、flag組合せ |
| W2-15 / 14 | core Prediction/Outcome比較と候補差分の純粋関数 | V16: 同条件の反証、別条件の不足、未評価維持、800→640、supportedへ自動昇格なし |
| W2-16 / 15 | adapterで評価差分を既存patchへ変換。安定OutcomeキーとCASを検証 | V17: 同Outcome再送はno-op、競合拒否、反証根拠削除で評価派生も失効 |
| W2-17 / 16 | 五要素を含む§9の合成会話fixtureをWriter→queryで統合 | V18: A〜Fすべて成功。派生direct edgeが保存されないことをDB件数で確認 |
| W2-18 / 17 | 通常抽出・Personal State投影・公開snapshotのWorld除外を回帰確認 | V19: Worldが既存会話Contextへ未接続のまま。既存Continuity結果は不変 |
| W2-19 / 18 | 五要素・長い日本語・履歴増加を含む性能測定。上限を変更しない | V20: §10の規模別結果と中断理由を保存。未測定規模を認定しない |
| W2-20 / 19 | 全ゲートとtest対応表を確認し、結果・未接続範囲・次段階条件を記録 | V21: K1〜K6、V01〜V20の証拠、既存失敗、性能制約が追跡可能 |

実際のゲートは詳細カードD05までの既存基盤、D33までの保存・照会接続、D40までの意味・統合とする。認可・忘却・データ消失・互換性の失敗はゲートを通過させない。試験が既に存在し十分なら、重複実装せず実行結果と対応を記録する。

## 9. 固定受入シナリオ

すべて合成Source・一時DB・明示時刻で再現する。実会話・本番DB・Provider credentialを使わない。各patchは8 assertion以内に分割し、Source依存数も守る。

| ID | 入力 | 必須の期待結果 |
| --- | --- | --- |
| A 目標への関連 | 導入concept decreases Decode Latency、Decode Latency increases Voice Latency、Voice Latency serves_goal Natural Conversation（lower_is_better）、SAAA has_goal同Goal | 因果はmetricまで、関連はGoal経由でProjectへ。Goal撤回後は現在の重要性を導出しない |
| B 相関 | TTFTと会話継続時間の負の相関、因果Evidenceなし、関連するcurrent_work | 相関とunknown_causal_direction。causesを保存しない |
| C 多段 | A increases B、B decreases C、C increases D、同じ成立条件・同じ非null comparison_id、score 900/800/700 | inferredDirection=decrease、path_rank_v1=567、3 hopの導出。条件不明なら方向unknown・score null |
| D 依存 | Action depends_on Component。観測をavailable / unavailable / なしへ切替 | 利用可能・欠落・不明を区別。BがGraphにあるだけで利用可能にしない |
| E 未知・条件 | 明示質問の未知seed、またはA→B成立・B→C不成立 | 未知はmissing_knowledge、架空edgeなし。後者は有効なA→Bを保持 |
| F 結果からの更新 | 関係版の期待decrease、同条件の実測increase、score 800、同Outcomeを再送 | disputed・反証・score 640・Gap。再送で512にならず、別条件なら反証確定しない |

A〜Fに対して、再送、訂正、時刻失効、Scope違い、Source忘却、再構築、再起動を組み合わせる。旧v1の関係も同居させ、互換読取の結果を固定する。現在journalによる復元試験はWorldを実際に含むfixtureで行う。

## 10. 検証と完了条件

以下は後続実装時のコマンドであり、この計画改訂で実行済みとはしない。filterに一致する試験0件を成功扱いしない。

```sh
# K1 core変更カード
bun run check:personal-state

# K2 adapter変更カード
cargo test --locked --manifest-path src-tauri/Cargo.toml memory::personal_state

# K3 境界カード W2-00 / 18 / 20
cargo test --locked --manifest-path src-tauri/Cargo.toml runtime::context

# K4 復元・migrationカード W2-00 / 03 / 09 / 20
cargo test --locked --manifest-path src-tauri/Cargo.toml persistence::

# K5 文書・サイズの節目
bun run size:check
bunx --bun spec-html check ./spec/docs --warnings-as-errors

# K6 最終
bun run check:local
bun run test:rust-packages
```

各カードでは対象試験を先に実行し、関連suiteを確認する。通過後に理由なく同じ重い試験を繰り返さない。既存の別文書エラー等はbaselineと照合し、対象外修正へ広げず未解消として報告する。今回原因の失敗は未完了。既存のcoverage・size ratchetを緩めない。

性能は同一端末・build profile、warm-up 5回、測定30回のmedian / p95 / maxを記録する。規模0 / 100 / 1,000 / 10,000を順に試し、fixture構築時間とload / rebuild / query / commitを分離する。一回の構築または操作が5秒を超えた規模は中断し、測定可能な小規模までの結果を残す。これは対話遅延の合格値ではない。未測定の10,000件を製品容量として認定しない。履歴増加と全件load費用は別に記録する。

M1拡張の完了には、五要素の意味、全V試験、旧データ互換、認可・失効・忘却・復元、再送・CAS、全体予算と参照整合性が必要。性能未認定は制約として残し、製品接続前に容量・遅延の合格値と保持方針を決定する。文書チェックだけで実装完了とはしない。

## 11. 後続計画へ渡す契約

| 段階 | 次の詳細計画で確定する内容 | 開始・終了ゲート |
| --- | --- | --- |
| M2 入力と現在状態 | Runtime / 外部Evidenceの版・取消・削除、runtime_ref、会議/Task状態、Goal正本の接続、Focus追加理由と期限、観測の採用方式 | M1拡張の証拠が揃い、保持量・遅延方針を固定。終了は正本との矛盾・Source偽装・失効漏れ0件 |
| M3 候補と利用 | World検索→contextStill→根拠充足→任意の調査依頼→五要素候補→決定的検証。WorldDelta安定イベント、抽出coverage、Broker非命令候補、generation依存登録、必要時推論 | M2 Source契約を満たす固定日本語corpusを先に用意。誤断定・推測補完・Scope漏洩・忘却復活0件を検証 |
| M4 評価と限定有効化 | 同一Provider・同一質問のON/OFF比較、関連説明とGapの適合率、追加byte・p95、訂正反映、feature flag | shadow前に改善・非劣化・遅延の数値基準を固定。満たさなければOFFを維持 |

DeepStillはTask / Research側へ依頼を渡す境界から始め、自動実行はV0必須にしない。調査結果もEvidence候補として再検証する。重要度やGoal推定から実行権限を生成しない。2B等へのモデル振分けはこの計画で追加せず、既存のモデル責務を維持する。

## 12. 実装担当への依頼文

```text
Personal World Modelの詳細カードD14だけを実装してください。
入力は実装契約C4、前提は詳細カードD00〜D13の結果です。
coreの条件評価とそのテストに限定してください。availabilityとSQLには進まないでください。
d14_の具体的な期待結果とK1を確認し、v2-progress.mdへ結果を追記してください。
DB・LLM・後続カードを先取りしないでください。
契約変更が必要なら、再現例と変更案を記録して該当カードを止めてください。
```

引渡しには、完了カード、実行した試験と件数、未実施項目、変更したschema、互換性、性能の認定範囲、次カードを記載する。別Codexタスクへの送信を手順に含めない。
