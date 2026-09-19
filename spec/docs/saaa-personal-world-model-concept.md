# SAAA Personal World Model Concept

作成日: 2026-09-19  
改訂日: 2026-09-20

状態: 5要素を中心とするコンセプト改訂。今回の改訂は文書のみで、機能・DB・公開APIは変更していない。

実装への具体化: [5要素改訂実装計画](saaa-personal-world-model-initial-plan.md)で、M1の再確認と五要素への拡張をW2-00〜20の到達点に分け、実装担当にはD00〜D42の詳細カードと固定契約を渡す。旧WM-M1計画は履歴資料として保存する。本書の5要素への拡張が既に実装されたとは扱わない。

## 1. 採用する方向

SAAAのPersonal Stateに、ユーザーに関係する対象の現在状態と、対象同士の関係を表すWorld Modelを追加する。最初の役割は「この情報が、今取り組んでいる仕事になぜ関係するか」「判断するには何の根拠が足りないか」を説明できるようにすることである。

変更の正本は既存のPersonal Stateのassertion / transition履歴に置く。グラフは、その履歴とRuntime参照から作る読み取り用の投影とする。独立した知識DB、会話Runtime、SQLite Writerは作らない。

Personal World Modelを「SAAAがユーザーに関係する世界について持つ、再利用価値の高い構造化された理解」と定義する。中核は次の5要素であり、因果だけのモデルにはしない。

| 要素 | 答える問い | 表現 |
| --- | --- | --- |
| 因果・影響（Causality / Influence） | 何が何に作用するか | 方向を持つ関係と、照会時に導出する多段経路 |
| 目標（Goal） | なぜその主体に重要か | 既存の目標参照、目標の所有と寄与の関係 |
| 条件（Condition） | どの状況で成立するか | 関係の適用条件と、その充足状態 |
| 相関（Correlation） | 何と何が一緒に変化するか | 因果と区別した対称関係と相関の符号 |
| 依存（Dependency） | 実現に何が必要か | 必要な構成・前提への関係と、利用可能性 |

五つは同じ種類のデータではない。目標は参照対象と関係、条件は関係の属性として扱う。単に五つのRelation Typeを追加する設計にはしない。五要素すべてをV0の受入対象にするが、介入後の数値予測や因果効果量の推定は行わない。

### 1.1 保存する理解と必要時の推論

再利用する理解の永続化と、必要時のLLM推論を組み合わせる。現在のProject / Goalに関係する、ユーザーが重要と明示した、複数の場面で使う、毎回の再構成に費用がかかる理解を保存候補にする。重要性と確からしさは別であり、重要な未検証仮説も仮説として保持できる。

細かな中間説明や一回限りの推論は、許可されたWorldSliceとKnowledgeから必要時に構成する。生成しただけでは保存せず、再利用価値・出典・Scope・重複・競合を検証して通常の候補採用経路へ戻す。派生経路を直接観測された関係へ変換しない。

未知の話題について、既存Graphへつなぐために関係を捏造しない。unknownやinsufficient_evidenceを有効な出力とする。ニュースやSNS本文はEvidence側に置き、必要なら集約された観測を候補として取り込む。World Modelを全知識や全投稿の保存先にしない。

この文書は[Personal AI Concept](saaa-personal-ai-concept.md)のWorld Modelを具体化する下位提案であり、[Personal Stateロードマップ](personal-state-architecture-roadmap.md)の正本・Scope・訂正・忘却契約を継承する。従来のP2はproject:SAAA・会議・Task参照に限定されている。本提案では、その基礎をP2aとして維持し、SAAAの音声応答改善に限った関係モデルをP2bとして追加する。ロードマップの対象範囲を暗黙に変更せず、実装計画の確定時にこの分割を反映する。

## 2. 最初に提供する体験

ユーザーがSAAAの音声応答改善に取り組んでいるとき、ある技術について質問する。SAAAは技術名だけで関連資料を検索するのではなく、現在のProject、改善したい指標、その技術が指標に作用するという仮説を結び付ける。

返す内容は次の三つである。

- 関連する理由: どの関係を辿ると現在のProjectに到達するか。
- 判断の限界: どの関係が仮説で、どの条件・観測が不足しているか。
- 次に確認する問い: 実際の採用判断に必要な調査や計測は何か。

例えば「Speculative DecodingはSAAAに関係するか」に対して、技術からDecode Latency、音声応答遅延、SAAAの改善目的までの経路を返す。ただし、SAAAの実際の構成で効果が測定されていなければ、採用効果は未検証と明示する。この例の技術的関係は受入用の仮説データであり、実測結果を意味しない。

Projectが中止された場合、過去に関心があったことを理由に現在の優先事項として提示しない。関連する関係が見つからなければ「関連なし」と断定せず、「現在のモデルでは経路が見つからない」と返す。

## 3. 現在のSAAAにある土台

以下の基盤調査は2026-09-19、HEAD `1b595ff`時点の記録である。2026-09-20の改訂ではWM-M1実装も参照した。現行との差分は§12に記す。

| 確認した構成 | 現在あるもの | 本提案での利用 |
| --- | --- | --- |
| `crates/personal-state-core/src/model.rs` | SourceRef、AccessScope、Assertion、Transition、StatePatch、版・依存・tombstone | Worldの意味状態にも同じ訂正・失効契約を適用する |
| `src-tauri/src/memory/personal_state/store.rs` | Writer transaction内での検証、履歴保存、CAS、投影再構成 | World更新を同じcommit境界に参加させる |
| `src-tauri/src/memory/personal_state/schema.sql`、`journal.rs` | 消去可能なpayload、依存索引、忘却・復元保護 | Node名、alias、関係条件などの派生内容も忘却対象にする |
| `src-tauri/src/runtime/context/scope.rs` | 明示Scope解決、Scope epoch | World独自のScope推定・権限を作らない |
| `src-tauri/src/runtime/context/source.rs`、`broker.rs` | 型付きContext候補、非命令データの検査、共通予算、選択・省略 | WorldSliceを通常のContext Source候補として供給する |
| `src-tauri/src/memory/context_still_recall.rs` | ContextStillへのrecall経路 | 根拠の追加取得との境界として使う |
| `README.ja.md`、`src-tauri/Cargo.toml` | Rust / rusqlite、単一SQLite Writer | TypeScript / Drizzleの別永続化経路を導入しない |

[共通Runtime統合計画](context-memory-unified-runtime-implementation-plan.md)は2026-09-17時点の統合完了を記録している。旧ロードマップに残る専用会話経路の説明を、現在の追加実装の前提にはしない。

WM-M1ではWorld用の型と投影・照会が追加されているが、Source検証は会話の保存構造に結び付いており、外部・Runtime Source参照は別途設計が必要である。DeepStillの実接続はV0の前提にせず、調査依頼と結果受領の境界から設計する。

## 4. 情報の所有境界

| 情報 | 正本 | World Modelの扱い |
| --- | --- | --- |
| 会話・資料・研究結果の本文 | 既存の会話・成果物保存先、ContextStill等 | 版付き参照を持ち、全文はコピーしない |
| 現在の目的・制約・決定・未決 | Personal StateのContinuity assertion | 参照する。同じ目的を別のGoal台帳に登録しない |
| Taskの進捗・会議の開始終了・実行結果 | その状態を所有するRuntime | 参照と有効時点を投影する。モデルの推測で上書きしない |
| 対象の識別、状態についての主張、関係、明示された関心 | Personal StateのWorld assertion / transition | 本機能が意味を管理する |
| 探索用Node / Edge / Focus | 上記履歴からの派生投影 | 再構築可能な索引。直接更新するAPIは設けない |
| 関連経路、因果経路、ResearchGap候補 | その時点の投影から計算 | 原則として実行時だけ存在する |
| 調査の依頼・実行状態 | 将来のTask / Research連携 | World側へ二つ目のジョブ管理を作らない |
| 操作の許可、送信可否 | 既存Policy・明示委任 | Worldの関心や因果関係から許可を生成しない |

「Graphを正本にして履歴へ後追い記録する」方式は採用しない。Graphだけが更新された状態を作らず、履歴のrevisionと投影のrevisionを対応させる。

## 5. 最小の概念モデル

### 5.1 Entity: 何についての理解か

WorldEntityは対象を識別するための参照である。V0の種別は`project`、`concept`、`metric`を基礎に、`goal`、`actor`、`runtime_ref`を追加対象とする。技術と一般概念はconceptで扱い、Taskや会議はruntime_refで既存IDを参照する。goalは既存Goal・ObjectiveのID、版、採用状態を参照し、専用の目標管理機構は作らない。actorはユーザー・組織・Agent等の主体参照であり、認証や権限を代替しない。

他者が持つ目標の推定はWorldの仮説として扱い、ユーザーが採用した目標とは区別する。モデルが候補目標を提案しても、自動的に実行Goalへ登録しない。

名前、alias、型、外部参照も出典・Scopeを持つ主張として記録する。Node一覧はその投影である。既存対象のIDがある場合はIDを優先し、表示名の一致だけで別Projectを統合しない。

名称解決は許可Scope内で、外部ID、正規化名、明示aliasの順に行う。正規化は表記比較用であり、意味の同一性の証明ではない。複数候補が残れば未解決として返し、LLMや曖昧一致だけで自動統合しない。V0ではFTS・embedding・汎用Entity mergeを導入しない。

### 5.2 State assertion: 現在どうなっているか

対象、属性、値または既存状態参照、適用Scope、有効期間、根拠を持つ。例えば「SAAAの今回の改善対象は音声応答遅延」「会議は終了」「Taskは実行中」を表す。

同じ対象・属性・適用条件に競合する有効な主張があれば、単に新しい方を採らず`disputed`として扱う。値がない`unknown`、古くなった`stale`、競合している`disputed`を区別する。Runtime所有の値はRuntimeが優先し、LLM候補との投票にはしない。

### 5.3 Relation assertion: どう関係しているか

関係もNode間の確定した事実ではなく、根拠付きの主張として保存する。関係payloadには始点・終点、関係型、適用条件、根拠ごとの支持／反証／参考の区別を持たせる。時刻、出典、適用Scope、生命周期は共通のassertion契約を使う。

| 区分 | V0の関係型 | 意味 |
| --- | --- | --- |
| 因果・影響 | `causes`、`increases`、`decreases`、`enables`、`inhibits` | 発生、量の増減、実現の促進・阻害を区別する |
| 目標 | `has_goal`、`serves_goal` | 主体・Projectが持つ目標と、対象がその目標へ寄与するという主張 |
| 相関 | `correlates_with` | 対称関係。positive / negativeは共変動の符号 |
| 依存 | `depends_on` | 始点が終点を必要とする関係 |
| 補助・互換 | `related_to`、`part_of`、`important_for` | 既存の関連・構造を保持。因果伝播には使わない |

`related_to`と`correlates_with`は対称、残りは有向として扱う。`supports`と`contradicts`はEvidenceが主張を支持・反証する区別に置く。causesという型名でも、根拠が仮説なら因果仮説である。increases / decreasesの始点は量の増加または明示した介入とし、単なる技術名の場合は導入・設定変更等の何を変えるかを明示する。

ConditionはRelation Typeではなく関係の属性とする。短いラベルと根拠参照から始め、現行のkey / value条件との互換性を保つ。評価は成立・不成立・不明を区別し、自然文を保存できたことを成立の証明にしない。不明な条件を持つ経路は未検証として返し、不成立の経路は現在の有効な作用説明に使わない。複数edgeの条件は出所を保って集約し、任意の自然文条件を自動的に論理結合しない。

Goalでは`主体 --has_goal--> 目標`と`手段・望ましい状態 --serves_goal--> 目標`を区別する。指標から目標へ接続する場合は「低いほど望ましい」などの望ましい方向を明記する。目標の中止・撤回・失効は既存の正本から反映し、関連度やFocusも再評価する。重要性はGoalへの接続から導出する方針を優先し、important_forの大量追加に依存しない。

Correlationは因果経路に含めない。因果仮説を検討するときは相関を上書きせず、別の仮説と根拠を作る。相関のstrengthには測定方法・対象・期間が必要であり、confidenceとは別である。

`A depends_on B`はAがBを必要とする意味であり、BからAへの因果効果を自動生成しない。依存先の利用可能性はavailable / unavailable / unknownを区別する。GraphにBの情報がないだけならunknownであり、現実にBが欠けているとは断定しない。Worldの依存関係は循環し得るが、根拠追跡用の`Assertion.depends_on`が持つ非循環制約とは別である。

### 5.4 Focus: 今、何を優先しているか

対象ごとに一つの関心度を上書きせず、`current_work`、`explicit_interest`、`temporary_attention`を別の根拠付きシグナルとして扱う。識別にはprincipal、適用Scope、対象、理由を使う。

現在の仕事は明示Scopeと目的・Task参照から、長期関心はユーザーの明示情報から、一時的な注目は現在requestから得る。繰り返し話した回数だけで長期関心に昇格させない。Projectの存在やそのScopeの登録だけで、取り組み中とはみなさない。

V0の順位は、現在の仕事、明示された関心、一時的な注目の順とし、同順位は関連距離と安定IDで決める。任意のinterest / importance / activityの小数を先に導入しない。一時的注目はrequest終了で除外し、会議・TaskはRuntimeの終了・撤回に従う。明示関心は訂正・撤回または明示期限まで維持する。時刻はコアへ入力し、読み取り時にも期限を検査する。

## 6. 確からしさの扱い

V0では単一のconfidence値を真実の確率として使わない。次の二つを分ける。

- 生命周期: candidate、active、disputed、superseded、retracted、invalidated、stale。既存の状態遷移契約に沿う。
- 根拠の性質: 明示発言、Runtime観測、資料の主張、モデル仮説。引用元と支持・反証の参照を保持する。

`active`は現在の投影に採用されたことを意味し、内容が科学的に実証されたことを意味しない。明示発言は「ユーザーがそのように述べた」根拠にはなるが、因果関係の実証にはならない。相関・同時出現から`increases`や`decreases`へ自動昇格させない。

モデル由来の関係は仮説から開始する。同じ資料の再投入、転載、同じ根拠を使った言い換えで確からしさを上げない。資料同士の独立性が分からない場合も、件数を確率へ変換しない。

根拠による評価区分としてobservation / hypothesis / supported / disputedを扱う。ただし既存の生命周期とは別軸に置く。supportedは明示された条件と根拠で支持されたことを示し、一般的な因果保証にはしない。V0で因果仮説を自動的にsupportedへ昇格させない。

confidenceは根拠に対する評価であり、効果量や相関のstrengthと分ける。数値を使う場合は用途、算出方法、方法の版、更新規則を先に固定し、未評価はnull等で明示する。LLMが出した0.8をそのまま真実の確率にはしない。多段経路の減衰も順位付け用の指標であり、確率の積とは扱わない。具体式と校正方法は次の実装計画で決める。

予測と実際のOutcomeが異なる場合は、同じ指標・構成・適用条件・比較基準かを先に確認する。比較可能なら反証を追加し、評価規則に従うconfidence低下とResearchGapを生成する。条件が違う場合は別条件の観測または条件不足として扱う。同じOutcomeの再送で評価を繰り返し下げず、どの予測・関係版への観測かを保持する。

## 7. 更新の流れ

未知のTopic・Eventを取り込む場合は、既存World Model検索、許可されたcontextStill検索、根拠の充足判定を先に行う。不足があればResearchGapを返し、必要に応じて既存Task側からDeepStillへ調査を依頼する。取得したEvidenceから五要素の候補を抽出し、以下の検証経路へ渡す。十分な根拠がないまま接続先を生成しない。この合成処理は更新側に置き、WorldSlice照会にネットワーク待ちや永続化の副作用を持ち込まない。

```text
確定済み会話 / 許可済み資料参照 / Runtimeイベント
  → 出典とScopeを確定するAdapter
  → 構造化WorldDelta候補
  → 型・意味・依存・版の検証
  → Personal Stateのpatchとして単一Writerへcommit
  → 同じrevisionのWorld投影
  → 共通Context Brokerへ候補提供
```

WorldDeltaは概念上の入力契約であり、公開APIの型や新サービスを意味しない。入力sourceのID・版、base revision、候補対象、状態、関係、Focus差分を含む。モデルは認可、Scope epoch、fence、発行IDを決定できない。これらは既存の信頼済みAdapterが供給する。

決定的な検証では、型だけでなく、参照先の存在、関係型と対象型の適合、Scope、期限、支持・反証参照、名称の曖昧さ、重複を確認する。Rust側を最終的な検証境界とし、TypeScriptの入力検証だけでDB更新を許可しない。

同一source版・同一候補の再送はno-opとする。同一イベントIDで内容が違う入力は競合として拒否する。関係の同一性には端点と型だけでなく、Scope・適用条件・有効期間を含める。条件違いの結果を一つに潰さない。初期版で意味的な同一性を判断できない場合は別候補とし、自動統合を避ける。

通常の訂正では履歴を上書きせず、置換・撤回のtransitionを追加する。古いrevisionや取り消されたScopeからの背景結果はそのまま採用せず、既存の競合検証へ戻す。commit時にsource・epoch・policyを再検査し、履歴と投影の更新は同じtransactionで完了する。

会話由来の抽出は既存の非同期抽出機構への追加として扱い、独立した抽出daemonは作らない。現在のユーザー入力は既存経路で直ちに推論へ届く。訂正対象に依存する古いWorldSliceは抽出完了まで省略し、古い状態を確定値として混ぜない。抽出失敗や未処理はcoverageで可視化する。

Runtimeイベントと外部資料のSource契約は段階的に拡張する。外部IDを会話message IDに偽装して既存の検証を通さない。出典の版・現在性・削除を照合できない外部資料は、永続World更新の対象外とする。

## 8. 参照と探索

### 8.1 いつ参照するか

明示された対象と現在の仕事の関係を尋ねられた場合、あるいは現在Scopeの目的・判断に関連する対象を解決できた場合に参照する。すべての雑談や音声の相槌にGraph探索を挟まない。V0では明示ID・名前・aliasでseedを解決し、曖昧な全会話分類を前提にしない。

### 8.2 二種類の経路

関連経路は「なぜ現在の仕事と関係するか」を説明する。構造・重要性・作用の関係を辿ってよいが、各edgeの種類と本来の向きを残す。

因果・影響経路は§5の五つの作用関係を辿る。逆探索は原因候補の検索であり、矢印の因果方向を反転しない。経路の存在から介入効果を断定しない。相関・目標・依存の関係は因果伝播へ混ぜない。Goalからhas_goalを逆に辿ってProjectへ戻る探索は関連性の説明であり、因果の反転ではない。

比較可能なincreases / decreasesだけで構成され、全条件が成立する場合に限り、符号を合成した潜在的な方向を返せる。例えば+ × − × +はdecreaseである。causes / enables / inhibitsや未確認条件が含まれる場合はunknown、複数の有効経路で符号が競合する場合はmixedとする。異なる変数・比較条件を機械的に合成しない。

初期版ではstrengthの積も、複数経路の確率合成も行わない。返すのは方向、条件、根拠、未検証点を持つ経路である。導出された始点と終点の関係を直接のassertionとして保存しない。

### 8.3 上限と返却内容

V0の初期上限は因果深さ3、関連深さ4、Node 30、Edge 60、経路計10、候補取得500件・展開検査500回、シリアライズ後8 KiBとする。関連深さ4は技術→指標→指標→Goal←Projectの説明に必要な範囲である。これらは性能実測値ではなく、無制限探索を防ぐ設定値である。実装時に固定fixtureで計測し、変更理由を残す。Context Brokerの残り予算が小さければ、さらに縮小する。

探索は最初から許可Scope・有効状態で絞る。Node数だけでなく、SQL取得件数と隣接edgeの走査にも上限を設ける。経路ごとの訪問済み集合で循環を防ぎ、全体の走査上限で分岐爆発を防ぐ。単一のglobal visitedだけで別の有効経路を消さない。

WorldSliceは対象、現在状態、関係、Focusの理由、経路、ResearchGap候補、as-of時刻、使用revision、根拠参照、競合・欠落・打切り理由を返す。五要素への拡張では関連Goal / Project、相関、依存先の利用可能性、条件の評価を含める。CausalPathは参照ID列、hop数、導出であること、inferredDirection、評価済みならconfidenceと方法、各edgeの条件を保持する。取得対象を絞っても条件や仮説表示を落とさない。FocusやGoalも全体上限内に収め、返却されないNodeへの参照を残さない。

上限到達と「経路なし」を区別する。取得後にScope外Nodeを隠す方式は採らず、名称や経路の存在自体も漏らさない。

Brokerへは非命令のContext候補として渡す。条件や仮説表示を削って経路だけを残さず、説明単位で選択・省略する。WorldSliceの組み立てでネットワーク照会やLLM抽出を待たない。根拠の追加取得は既存のrecall等へ分ける。

Worldはoptional sourceとし、障害時には省略理由を残して通常会話を継続する。ただし、Worldが必要な質問へ完全な回答ができたとは扱わない。既存の必須制約・認可情報をWorldのoptional枠へ移さない。dispatch・出力時には、選択したWorld assertionと実際の依存source / Scopeの失効を既存のgeneration検証へ接続する。

## 9. ResearchGapは「判断に必要な不足」から作る

単にconfidenceが低いedgeを列挙するのではなく、現在の目的に到達する経路上で、判断に必要な根拠がない、期限切れ、反証がある、適用条件が未確認という場合に候補を返す。

V0では、現在の仕事または明示された関心に関係する不足を対象にする。未知のTopicについて明示的に判断を求められた場合は、経路がなくてもmissing_knowledgeを返せる。Gapを作るために架空のedgeを追加しない。一時的な話題だけでは常設の調査候補を増やさない。優先順位はFocusの区分、質問との直接性、経路距離、安定IDの順で決定し、隠れた小数スコアは使わない。

不足の種類はmissing_knowledge、unknown_causal_direction、missing_mechanism、low_confidence_relation、missing_condition、conflicting_evidenceを基本とする。confidence未評価を数値的な低評価と混同しない。探索打切り・認可による非表示を、世界についての知識不足と断定しない。

候補には対象assertion参照、関連する目的参照、不足の種類、質問、必要な確認を含める。例は「現在のモデル・ハードウェア・ストリーミング構成で、Speculative Decodingの導入は音声応答の対象指標を改善するか。未導入時との比較計測が必要」である。

V0は候補の返却までとし、ResearchGapテーブルやqueued / resolved管理は作らない。同じ不足は対象・条件・不足種別から同じ候補キーにする。後で調査を開始するときは、このキーをTask / Research側の重複防止へ渡す。DeepStillが返した結果も通常のEvidence候補として再検証し、調査完了だけで関係を真にしない。

## 10. RustとSQLiteへの配置

意味的な境界はPersonal State内のWorldモジュールとする。初期配置案は次のとおり。

```text
crates/personal-state-core/src/world/
  対象・状態・関係の型、検証、探索、Focus、ResearchGap候補

src-tauri/src/memory/personal_state/world/
  payload codec、投影索引、Source Adapter、Context候補の組み立て

src-tauri/src/runtime/context/
  既存Brokerとgeneration依存検証への接続
```

純粋なWorldロジックはIO・時刻取得・ID発行・LLMに依存しない。既存のKindへWorld用の明示variantを追加する方向とし、既存Decision等のpayloadへ無理に押し込まない。既存codec・テスト・migrationとの互換性を確認してから型を確定する。共通ライブラリ全体の汎用化は前提にしない。

探索用にはEntity、State、Relation、Focusの投影索引を追加できる。検索に必要なScope、端点、型、有効状態、assertion ID、revisionは列として持ち、JSON全文走査を探索の基本経路にしない。一方、正本payloadは既存の消去可能ストアを利用し、別の可変Relationテーブルを正本にはしない。

削除・再構築は同じWriterと既存migration経路で扱う。投影が欠落またはrevision不一致なら利用を止め、再構築まで縮退する。V0の規模では全再構築から始めてよく、差分更新やSnapshot＋Overlayは計測で必要になってから採用する。

既存の`store::load`はledgerを読み込むため、Graphの取得制限だけでDB全体の費用がboundedになるとは見なさない。V0では入力候補数・payload量・保存量にも上限を持たせ、超過時は未処理を報告する。初回実装計画で既存制約を確認し、初期案の候補50件を新規World assertion最大8件へ具体化した。既存のpatch全体16 KiB、assertionとtransitionの合計32操作、payload 2,000 byteも維持する。World assertionはCandidate / Active / Disputedの合計10,000件を上限とし、置換される項目を差し引いて判定する。累計上限で訂正を塞がず、履歴の増加を含むload・commit・再構築費用を別途計測する。撤回・忘却は上限にかかわらず受け付ける。

具体的なDDL、テーブル名、公開インターフェースは次の実装計画で決める。この文書で新DB、Hono、Drizzle、Graph DB、Vector DB、新規frameworkを導入することは決定しない。

## 11. 訂正・忘却・現在性

根拠の削除はedgeだけでなく、派生したNode名・alias・条件・Focus・投影・Contextへの依存にも伝播する。生成に渡した入力全体への依存を保持し、引用に選ばれた根拠だけで忘却範囲を狭めない。

通常の訂正と忘却は区別する。訂正は履歴を残して現在値を変える。忘却ではpayloadと内容を含む投影を消去し、再生防止に必要な内容を含まない記録だけを残す。既存のforget journalによる復元保護をWorldにも適用し、feature OFF中やバックアップ復元後も削除済み情報を復活させない。

一部の根拠だけが残っても、削除対象を入力に使った旧assertionをそのまま継続利用しない。必要なら残った許可済みsourceから新しいassertionとして再評価する。

外部サービスへ接続できないことと、根拠が削除されたことは区別する。再検証できない出典は確認日時・期限に応じてunavailable / staleとして扱う。現在性が必要な関係を、最終確認時点を隠して確定値として返さない。

## 12. V0の段階と完成条件

2026-09-20時点のWM-M1はproject / concept / metric、六つの関係型、key / value条件、current_work / explicit_interestを持つ。本改訂のGoal、相関、追加の作用型、confidence等は拡張対象であり、実装済みとは扱わない。M1レビュー（`spec/evidence/world-model/m1-review-2026-09-20.md`）で指摘した認可・失効・探索上限等は、新定義への変更で解消したことにしない。

導入は、M1の不具合修正、五要素の固定fixtureと意味契約、型・保存・照会の拡張、Runtime / Goal参照、LLM抽出とEvidence取得の順に進める。既存の履歴・投影基盤を再利用し、次の実装計画では一つの責務と確認テストを一つの作業カードに分ける。

旧データのimportant_forからGoalを、related_toから相関を自動生成しない。旧条件は成立根拠がなければ不明、旧confidenceは未評価とする。既存関係は元の意味で読み出せるようschemaと移行を版管理し、新しい根拠がある場合だけ通常のpatchとして追加・訂正する。

| 段階 | 対象 | 完了の判断 |
| --- | --- | --- |
| P2a: 現在状態 | SAAA Project、現在の会議、Task参照。既存履歴・Source・投影の拡張 | Runtimeの終了・訂正・撤回が反映され、Scopeと忘却の既存契約を満たす |
| P2b: 五要素による説明 | 音声応答改善に必要な因果・影響、Goal参照、条件、相関、依存、Focus、bounded探索、Gap候補 | 下記の構造化シナリオが再現可能に通る |
| P2c: 会話への接続評価 | 既存非同期抽出への限定追加、BrokerへのWorldSlice供給 | 同じ質問群で有効化前後を比較し、誤断定を増やさず関連説明と不足検出を改善する |

構造化入力だけが動くP2bと、実際の会話から利用できるP2cを区別する。利用者向けV0完了はP2cまでとし、手入力Graphの探索成功だけで完成とはしない。P2cは最初にshadow評価を行い、通過後に限定有効化する。既存Memory設定に従い、World専用の有効化制御も初期OFFとする。

受入用の関係例:

```text
Speculative Decodingの導入 -- decreases --> Decode Latency
Decode Latency           -- increases --> Voice Response Latency
Voice Response Latency   -- serves_goal --> Natural Conversation
  target condition: lower_is_better
SAAA                     -- has_goal --> Natural Conversation
```

導入は明示した変更を表すconcept、二つのLatencyはmetric、SAAAは現在の仕事に対応するproject、Natural Conversationは既存目標への参照である。二本の作用は構成条件付きの仮説とする。`Decode Latency increases Voice Response Latency`は、Decode Latencyが増える方向で応答遅延も増えるという仮説を表す。したがって最初の減少が後段の減少へつながる可能性を説明できるが、条件未確認なら結果を断定しない。指標と目標の接続には低いほど望ましいことを明記する。関連経路はGoalからhas_goalを逆に辿ってSAAAへ戻れるが、因果経路はmetricまでである。

| 入力・操作 | 期待結果 |
| --- | --- |
| 上の構造化Observationを投入して関連を照会 | SAAAまでの根拠付き関連経路を返す。因果経路はmetricまで |
| 同一Observationを再投入 | assertion・edge・Focusが増えず、確からしさも変わらない |
| 最初の作用が仮説で構成未確認 | 採用判断の不足としてGap候補を返す。測定済みとは表現しない |
| related_toしかない経路を照会 | 関連は説明できるが、因果経路は返さない |
| TTFTと会話継続時間の負の相関のみを投入 | 相関を返す。因果へ昇格せず、判断に必要ならunknown_causal_directionを返す |
| A increases B、B decreases C、C increases Dを照会 | 条件成立・比較可能なら潜在的なdecreaseを返す。導出経路であり直接edgeは保存しない |
| 条件が不成立または不明 | 不成立の経路を有効扱いせず、不明なら無条件の効果を断定しない |
| depends_onの依存先が欠ける観測、または情報未取得 | 前者はunavailable、後者はunknownとして区別する |
| 関連Goalを撤回 | 現在の重要性をそのGoalから導出せず、FocusとGapを再評価する |
| 未知Topicを明示的に質問 | 架空の経路を作らず、必要な知識の不足を返す |
| 比較可能なOutcomeが予測と逆、同じOutcomeを再送 | 反証・評価変更・Gapを記録し、再送で二重に評価を下げない |
| 反証または異なる構成の観測を追加 | 競合または別条件を保持し、黙って上書きしない |
| Projectの中止後に古い抽出が到着 | 現在の仕事へ復活させず、古い依存を再検証する |
| 別Projectの同名Nodeを投入 | 名前だけで統合せず、Scope外の名前・経路を返さない |
| 循環・高次数Graphを照会 | 深さ・件数・走査・byte上限で止まり、打切りを報告する |
| 一時的な話題のrequestが終了 | temporary_attentionを現在のFocusから除外する |
| sourceの削除後に投影再構築・再起動・復元 | 派生内容が再出現せず、依存する既存generationを無効化する |
| World無効化・障害・Context予算不足 | 通常Runtimeを継続し、必要なら回答の不足を説明する |
| 自然文の質問・訂正を既存抽出経路へ投入 | 更新coverageを追え、次の有効generationで訂正済みSliceを使う |

決定的なテストは既存Rustテストに追加する。抽出品質と回答品質は固定した日本語corpusを用い、同一Provider設定・同一入力でWorld OFF / ONを比較する。関連Projectの正解率、仮説の断定件数、不足検出の適合率、Scope漏洩、訂正反映、追加byte数、照会・再構築のp50/p95を記録する。

評価corpusは実装前に固定し、関連あり・関係不明・相関のみ・反証・条件違い・中止・忘却を含める。Scope漏洩、削除後の復活、Runtime正本の誤上書き、相関からの因果昇格は固定corpus上0件を必須とする。関連説明の改善と性能予算はベースライン採取時に合格値を決め、結果を見てから基準を緩めない。満たせない場合はshadowのまま修正し、機能追加で取り繕わない。

## 13. 今回採らない案と理由

| 案 | 採らない理由 |
| --- | --- |
| `wm_nodes / wm_relations`を独立した更新正本にする | Personal Stateの訂正・依存・忘却との二重管理が発生する |
| 別のworld-model crate・サービスから始める | 共通のSource・履歴契約を再実装する負担が先行する。まず既存crate内のモジュール境界で分離できる |
| TypeScript / Hono / Drizzleを追加する | 現在のRust WriterとRuntime境界に別経路を作る必要がない |
| 最初から汎用Knowledge Graphを構築する | 保存量と名称統合の難しさが先行し、現在の仕事への価値を測りにくい |
| strengthを掛けて効果を数値予測する | 数値の意味と条件が定義されておらず、効果・確率との混同を招く |
| confidenceが低い項目をすべて自律調査する | 現在の目的への寄与、費用、実行委任を別途判断する必要がある |
| 長期関心を会話頻度から自動学習する | 一時的な話題や仕事上の言及を本人の関心と取り違えやすい |

SNS crawler、全投稿のNode化、Graph可視化UI、GNN、因果発見、Bayesian推定、独自Scheduler、DeepStillの実接続、ContextStillへの大量複製はV0に含めない。

## 14. 次の実装計画で確定する事項

方向性は「Personal State内のWorld assertionと、その関係投影」で固定する。次に必要なのは別方式の網羅的比較ではなく、以下の契約を既存コードへ具体化することである。

1. World用Kind・payload schemaと、既存codec / projection / migrationの互換性。
2. Runtime参照と外部EvidenceのSource検証・版・失効契約。V0で実接続する入力範囲。
3. Scope解決済み入力からWorldを選択し、generationへ依存を登録する接続点。
4. 訂正中の古いSliceの抑止、投影再構築、忘却・復元処理の拡張箇所。
5. 固定corpus、容量・遅延予算、P2a / P2b / P2cの実装順と合格条件。
6. Goalの参照失効、条件の三値評価、相関の対称性、依存先の利用可能性を表す型と五要素の受入fixture。
7. confidenceの用途・未評価表現・方法の版・更新規則、多段経路の減衰指標、PredictionとOutcomeの比較契約。
8. 旧schemaの互換性、M1レビュー修正と拡張の作業カード分割、ResearchGapから既存Taskへ渡す境界。

この文書の作成ではコード、DB、公開API、認証方式、Provider経路を変更していない。後続実装ではWorldの型・永続化索引・既存接続点に必要な変更を行うが、会話Runtime全体の再設計や外部サービスの改造まで含めない。
