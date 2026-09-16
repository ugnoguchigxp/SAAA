# Personal State P1 — 現在状態・Source Set・Context Viewの接続計画

状態: 実装中・製品接続の実機受入待ち。2026-09-13 LARM契約照合版。[全体ロードマップ](personal-state-architecture-roadmap.md)と[確認根拠](larm-personal-state-contract-review.md)を併読する。本文の型名はSAAA側の案であり、既存LARM API名と区別する。

実装進捗（2026-09-13 更新）: 状態基盤に加え、SQLite v18の配送outbox、短期Provider認証とKeychainのsubject binding、製品用HTTP配送・canonical計測・v2 View・attempt取消・多層cleanup、実workerを通す受入harnessを実装した。合成HTTP試験とローカル検証を実施。LARMの常駐27Bでは配送から実generationまで確認したが、runtime消去用の起動設定適用に管理者権限が必要で、忘却の実機受入は未完了。日本語goldの人手確認、全受入行列・性能比較も残るため、P1全体の完了・利用可能とは判定しない。詳細は `spec/evidence/personal-state/product-connection-progress.md` を参照。default OFFを維持する。

## 1. P1で完成させるもの

2Bが限定的な受領・確認・構造化判断を担当し、27Bが根拠付きの回答とタスク判断を行う。SAAAが現在の目的・有効な制約・訂正・未決・参照対象を管理し、毎回のbounded Context Viewへ必要なものを渡す。View・snapshotを失っても原文、状態変更履歴、Runtime台帳から再開する。

P1は次の四つに分ける。P1-Aだけで継続性が実現したとは報告しない。

| 区分 | 成果 |
| --- | --- |
| P1-A 状態基盤 | principal、出典、assertion/transition、依存関係、current projection、認可・忘却の決定的契約 |
| P1-B 推論接続 | Source provision/登録、one-shot View、委譲・返却、取消・忘却outbox、定型通知 |
| P1-C 状態抽出 | 27Bの差分候補を検証・採用し、未反映入力を取りこぼさない有限worker |
| P1-D 受入 | 日本語固定corpus、race・回復・権限試験、実機・資源評価、最小診断 |

対象は既存利用者principalの単一primary scopeと既存Task/requestである。複数scopeの自動推定、World Model全体、人格・感情推定、Goalの自動生成、ContextStillへの新規送信、外部agent export API、音声保存、新規RAG/embedding、VoiceMem変更、KVエンジン改修は含めない。

## 2. 実装開始時の契約確認（P1-00）

最初にSAAA HEAD、dirty状態、schema、関係する既存試験を記録する。今回読んだSAAAは `cc8c66c`、LARMの確認版は照合記録を参照。作業開始時には再取得する。

| 確認対象 | 現在確認できた土台 | P1-00の確定事項・不足時の扱い |
| --- | --- | --- |
| memory | context_window、control_plane、conversation_inputs、SqliteWriter | 既存表・test-only関数・source削除triggerを監査。過去migrationを編集せず最新番号から追加 |
| 2B | larm_voice/decision.rsはshadow分類で回答を変更しない | readiness証跡の有無。未認定ならshadow＋定型文を維持し、権限を拡張しない |
| Provider接続 | larm-sessionは4 Providerのleaseを取得。別のLARM stream経路も存在 | 音声・通常Chatが実際に通る経路を追い、Context用Allocation/capabilityを対応付ける。全経路対応と誤報しない |
| source配送 | LARMはhost上のprovisionとattestationを要求。登録APIは本文を受け取らない | Macの正本から同じprincipalのhostへ送る許可済み経路、provision、返却metadata、削除を確定。存在しなければ専用依存作業としP1-B未完了 |
| Context API | POST /v1/contexts、POST /v1/context-views、DELETE /v1/contexts/:id | 認証・冪等キー・列挙/状態照会・失効・API/schema版を対応表に固定 |
| 入力予算 | 20M source quota、131,072 context、125,000最大入力、4,096出力予約、1,976余白 | 実releaseとtokenizer/chat template、materialized byte上限、期限、認定modeを取得。SAAA側も125,000入力・4,096出力で上限固定する |
| 取消 | LARM active requestはnon-preemptive。呼出し側HTTP cancelが必要 | SAAA RunCancellationから実際のHTTP/stream/操作へ届く経路と終了照会。ローカルfuture終了だけで遠隔停止としない |
| source/snapshot消去 | Context登録削除は全versionとready Viewを無効化。source storeには別のdeleteがある | host上の本文・attestation・snapshotのcleanup公開経路と保証を確認。base messages由来のsnapshot依存も含める。登録DELETEだけを物理消去としない |
| 資源制御 | activityは瞬間観測。自動cross-runtime排他を保証しない | SAAA内の直列化、foreground優先、他consumer混在時の動作、semantic/mixed認定版を固定 |

成果物はAPI/コード根拠、request/response例（本文・credentialなし）、担当Adapter、試験名、不足時動作を持つ契約表とする。base messages＋動的suffixのsnapshot適合条件も確認する。ローカルで新しいHTTP APIやSFTP/SSH常設bridgeがあると仮定しない。本計画の原本確認に用いたSSHを、そのまま製品のデータ配送方式にはしない。

P1-00を通るまではモデルなしのコア試験を進められるが、source配送・認可・取消・View bindingの不足を通常Chat fallbackで合格扱いしない。必要なLARM契約追加は対象・所有者・完了条件を別作業として文書化する。

## 3. コアとAdapterの境界

`crates/personal-state-core/` は型、reducer、依存関係の検証、認可条件、required/optionalの選定を担当する。Tauri、HTTP、LLM SDK、tokenizer実装へ依存せず、時刻・ID・計測済み予算・認可結果を引数で受ける。

SAAAの既存SQLite writerが永続化を担当する。SAAA内のRaw本文は既存保存先のみとし、LARM向けの送信複製は登録台帳で追跡する。モデル処理・ネットワーク中はDB transactionを保持しない。

Runtime Adapterはrequest/runの識別、受付・実行・取消・結果の台帳接続を担当する。LARM Adapterはprovision・登録・View作成・materialization結果・HTTP取消を担当し、SAAAのcurrent stateを直接書き換えない。2Bの出力もコアの更新権限を持たない。

## 4. 状態モデルと変更台帳

| 型案 | 必須情報 |
| --- | --- |
| AccessScope | principal、用途、classification、task/requestまたは明示scope共通、policy revision |
| SourceRef | source kind、ID、version、digest、coverage、role/生成主体、時刻、利用可否、AccessScope |
| Assertion | assertion ID、kind、semantic key、payload参照、根拠SourceRef、depends_on、extractor provenance、observed/effective/recorded time、valid_from/valid_until |
| Transition | event ID、sequence、対象assertion、assert/activate/supersede/retract/invalidate/resolve、理由、根拠、時刻 |
| CurrentProjection | scope/state revision、input epoch、現在有効・候補・競合・失効の項目、根拠、鮮度 |
| StatePatch | SAAA発行のpatch ID、生成元generation/attemptまたはjob ID、base revision、input epoch、policy revision、採用fence、source coverage、差分候補 |
| SourceRegistration | principal、SAAA source版/範囲、LARM context ID/version/handle、digest、tokenizer、分類、registration incarnation、desired/observed状態、expiry |
| GenerationManifest | task/run/request revision、generation ID、attempt ID、用途、input epoch/policy revision、projection revision、base/登録物の入力依存、実入力の根拠allowlist、Allocation/runtime/release、View ID/digest/lease epoch/期限、応答状態 |

Continuityのkindはobjective / constraint / decision / pending_decision / open_loop / active_referent / progress_refとする。task/requestに属する条件をscope共通へ暗黙に変えない。対象が曖昧なら元の有効状態を維持し候補にする。

同じrunのtool往復・回答・再試行・状態抽出は別generation/attemptとし、各Viewと出力を一意に結ぶ。Task/runの終了状態と個々のgenerationの成否を別記録にする。

current statusはcandidate / active / disputed / resolved / superseded / retracted / invalidated / staleを区別する。superseded_by、retracted_at、invalidated_atはtransitionから導出する。通常更新でassertionやtransitionの内容を上書きしない。新しい主張は新assertionとして追加する。currentを求める時刻は引数として固定し、valid_from前・valid_until以後・根拠期限切れをactiveにしない。idle workerが停止していても読み取り時に判定する。通常期限切れは履歴を残し、forgetとは区別する。

extractor provenanceにはmodel識別子・release、extractor version、prompt digest、schema version、設定digestを持つ。credentialは含めない。Runtime由来は実event ID/sequenceを持ち、会話message IDを偽造しない。

既存working_state_itemsとcapsuleをcurrent projection/版付き投影として利用できるよう移行する。既存の直接書き込み口を同じtransition経路へまとめる。根拠のない旧項目はstale、復元不能な旧capsuleは再構成対象とし、推測でactiveにしない。

### 依存関係と忘却可能な履歴

assertion→sourceとassertion→assertionの辺を持ち、逆引き索引を作る。依存は同じprincipal内の既知IDに限り、循環を拒否する。根拠を支持するdepends_onと、生成時に露出した情報のinput_dependenciesを区別する。分類・用途・忘却伝播はモデルが選んだ引用だけでなく、そのgenerationの全入力依存から導出する。実際に渡った範囲が不明なら投入候補全体を保守的に含める。派生assertion・回答・artifactの分類は入力の最大制限、用途は許可集合の共通部分を継承し、モデルに制限緩和を宣言させない。

sourceやassertionが失効したら、推移的な派生物もcurrentから外す。複数根拠の一部が失われた場合も保守的に失効し、残る根拠だけで再評価する。大きな依存閉包を分割cleanupしても、tombstone/epochと祖先の有効性検査により、失効途上の項目を投影しない。

payloadは削除可能な保存領域へ分離する。忘却ではassertion本文・派生値・引用・必要に応じ識別可能なdigestを消去し、復活防止に必要な不透明ID・時刻・処理状態だけを保持する。通常更新の履歴不変性と、明示忘却によるpayload消去を区別する。古いDB backupは忘却台帳を再適用するまで有効化しない。復元前に現行のtombstoneを退避し復元後へ合成する。現行台帳を失い最新の忘却を復元できない場合は、旧backupをそのまま利用可能にせず回復未完了とする。

## 5. 入力順序・差分適用・未反映source

入力保存とsource登録待ち/抽出待ちは同一transactionまたはdurable outboxで結ぶ。確定ユーザー入力を先に保存し、assistantの失敗・中断でも訂正を残す。生成途中の断片を決定の根拠にしない。

state revisionとは別にinput epochを確定入力・訂正・削除で進める。P1はscope単位の保守的比較とし、推論後に新入力があれば旧patchを拒否する。request revisionは対象Taskの依頼変更時だけ、LARM lease epochはLARMのbindingから取得する。状態抽出patchは新input epochで拒否するが、無関係な進捗質問だけで既存Taskの成功事実を消さない。Task結果は保存入口でrun/attempt・忘却・認可を検査し、有効な本文だけを保存する。忘却後の遅延結果は本文を破棄し、実行終了・失敗等の必要最小限の非本文metadataを台帳へ残す。公開時にも依頼版・根拠・忘却・policyを再検証する。epoch差分の影響が未判定なら公開を保留し、対象が無関係と検証できた場合のみ現在の許可状態で公開する。

commitではbase revision、input epoch、policy revision、採用fence、機能flag、出典版/digest/利用可否と評価時刻での有効期限を再確認し、transition、projection、coverage、job結果を原子的に保存する。同じpatchはno-op、同じIDで異なる内容はconflict。モデルに最新epochを自己申告させない。worker候補の採用fenceはjob lease generation、Task同梱候補はSAAAが発行したgeneration/attemptの有効性とする。回答本文を抽出sourceとして使う候補は本文保存後に抽出をenqueueし、保存前の未来sourceを根拠にしない。

sourceは厳密な保存sequenceと版付き範囲で追う。DB read transactionの外へ読み出した本文にも版/digestを付け、送信直前と結果採用時に照合する。長文はUTF-8境界で分割し、処理していない範囲を全文処理済みにしない。分割messageの途中結果は候補として保持し、同じmessageの後半にある否定・条件を読む前にdecisionへ昇格させない。全範囲のcoverageと必要な前後文脈を確認する最終化jobを通し、予算内で意味を確定できなければ未完了を残す。source windowの先頭/末尾だけでなく中間messageも対応表に含める。編集・置換で正本のversionが変わった場合は旧版へのcurrent依存をstaleとして再評価する。旧版は履歴として残せるが、利用者が指定した歴史的参照と現在値を区別する。

coverageは反映済み、検証済みNO_CHANGE、不確実性を候補として記録済み、未処理/抽出失敗を区別する。「採用は未決」を正しく記録できたsourceは処理済みにできる。例外や内容未解釈はNO_CHANGEにしない。連続frontierは未処理で止め、後続の個別coverageも保持して無限再処理を防ぐ。

未反映入力は現状態より新しい根拠として次のgenerationへ渡す。関連する訂正が未処理なら古い値を確定値として提示しない。対象不明の変更は確認へ回し、関係するTaskの次の副作用を伴う操作だけを既存Runtime境界で保留する。通常の受付まで全体停止しない。

P1-00で確定する新loaderは正本を安定IDでページ読みする。既存context_windowの最大400件・長文4,000文字の先頭/末尾抽出を、source exportや意味抽出の入力へ流用しない。64,000 bytesの旧入力検証は従来経路用に維持し、managed経路には認定token/byte予算を適用する。全履歴を一括RAM展開せず、範囲と完全性を記録する。

初回ONでは既存履歴の処理範囲を記録し、未確認の過去を処理済みにしない。既存有効stateの検査と直近の連続範囲を先に扱い、古い履歴は有限jobまたは明示参照時に読む。古いsourceを後から処理する際も現在の訂正・撤回と照合し、過去への誤復帰を防ぐ。pendingがbase予算を超えた場合は範囲分割・原文の追加照会を行い、必要な未処理条件があるTaskはcontinuity-incompleteとして保留する。登録sourceがないことを理由に、欠落した必須入力を通常Chatへ逃がさない。

## 6. 読み取り・送信・保持の契約

| 項目 | P1方針 |
| --- | --- |
| principal | 既存認証とLARM credentialの対応から決定。別principalのsource/assertion/登録物を混ぜない |
| 用途 | tactical、reasoning、state_extract、diagnosticsを明示。モデルや外部文書から用途を変更させない |
| 分類 | public/internal/confidential/restrictedを保持。個人会話はconfidentialを初期分類とし、未分類外部sourceは分類確定まで送信しない |
| 許可 | 分類に加え宛先・用途・既存ユーザー設定/委任を評価。restrictedを通常用途へ自動送信しない。派生物の制限緩和なし |
| 2B投影 | 分類に必要な確定発話と許可済み状態keyの最小範囲。事実回答や全履歴整理のために詳細を渡さない |
| log/telemetry | 本文・PII・credentialを記録しない。理由code、件数、時間、非機密の版を記録。digestや高cardinality IDを通常metricsに出さない |
| diagnostics/export | 認可済みの本人向け状態/出典ビューを用意。一般logへ出力しない。新規の外部agent export APIはP1対象外 |
| retention | SAAA原文は既存の保持設定を継承。新しい無期限保存を増やさない。派生物の有効期限は根拠・用途・設定の最短条件に従う |
| 送信複製 | provision用一時ファイルは完了/失敗後に削除。host sourceと登録物は必要期間のみ保持し、不要解除・forgetはdurable cleanupへ記録 |
| backup | 新しい自動backup機能は作らない。既存backupの所有者・保持期限・忘却反映方法をP1-00で記録。管理外backup/OS snapshotの消去を保証しない |
| secure erase | アプリ上の利用停止、管理対象削除、遠隔cleanup確認を別表示。物理secure eraseは保証しない |

全ての投影・登録・View作成・結果送出・export相当の読み取り時に権限とforgetを確認する。LARMにclassification fieldがあることだけで用途別認可まで実装済みと解釈しない。

Context PackとLARM登録sourceはdataとして扱う。LARMは選択sourceをsystem message内のdata blockとしてmaterializeするため、role名だけで信頼性を決めない。SAAA policyで引用内の命令を実行権限にせず、tool実行時は別途既存Policyを照合する。

## 7. 2Bと27Bの委譲・返却

2Bの出力はallowlistの構造化候補だけを受け付ける。P1では自由な短文回答、結果の要約、約束を許可しない。現行shadow分類をproduction routingへ自動昇格させない。semantic readiness未認定ならSAAAの定型文と27Bが機能経路を担当し、2Bはshadowのままでよい。

SAAAは確定入力の原文SourceRef、task/run ID、request revision、scope、現在state、未反映訂正、権限参照をTaskInputとして渡す。2Bの言い換えだけで依頼を再構成しない。質問と実作業の最終判断は27Bへ渡す。

TaskUpdateは既存run/request/event sequence、Runtime状態参照、成果物/回答、確認要求、根拠、任意StatePatchを持つ。Runtime状態は台帳から読み、27Bの「終わった」という文から生成しない。結果は27Bまたは決定的な形式変換から表示/TTSへ送り、2Bに意味を再生成させない。

受領・開始・処理中・停止要求・停止確認・失敗はSAAAの定型文を使う。台帳にない進捗を補わない。27B停止時も受領と取消・状態通知は可能だが、2Bが事実回答を代行しない。

2Bを使う場合は短い直列burstを27Bの前に置き、timeoutなら定型へ戻す。27B実行中の通知に2B generationを必須にしない。mixed認定がない同時generationは禁止し、background抽出も同じSAAA内schedulerで直列化する。他consumerの停止や全hostの排他を暗黙に要求しない。

## 8. Source Setへの登録とquota

SAAAが持つ原文・artifactから、用途と分類を確認したimmutable export単位を作る。単位はsource版とcoverageを持ち、長文は削除・再利用・View予算を扱える範囲に分割する。別source/分類を一つの無差別な要約へ混ぜない。

SourceRegistrationはSAAA source版/範囲とLARM context ID/version/handle/digestを対応付ける。LARMのDELETEはcontext IDの全versionに及ぶため、独立に解除する単位は独立したcontext IDにする。同一の登録操作の再送だけ同じ冪等キーを使う。解除後の再登録は新しいregistration incarnationと冪等キーを発行し、過去の成功response replayを現在の登録の証拠にしない。内容変更は新export identityにする。

provisionはcanonical tokenizerのattestationを作り、byteCount/tokenCount/tokenizerDigestを返す。SAAAは本文・token数を自己申告しただけで登録成功とは扱わない。通信不明時は登録状態を照合し、成功を確認したものだけViewへ参照する。API冪等性の有効期間を超えた再送もP1-00で確認する。provision/登録の前後でsource版・policy・tombstoneを再検査し、送信中に忘却されたsourceの遅い登録成功はactiveにせずcleanupへ送る。孤立したhost複製も登録失敗時のcleanup対象とする。

principal全体の20M quotaを監視し、SAAA以外の登録も勝手に削除しない。満杯なら、自分が所有し未使用かつ正本から復元できる非必須の登録を解除する。pinの取得と登録解除候補のclaimをSAAA側で直列化する。進行中/ready Viewに必要な単位をpinし、解除後の再登録は現sourceの権限・忘却状態を再検査する。これは登録領域の整理でありSAAA正本の削除ではない。

新旧版の一時重複がquotaに収まらない場合は、対象runを安全な境界で止めて旧登録を解除してから登録する。quotaは登録直前にも再評価し、他consumerが枠を消費した場合の拒否を通常の競合として扱う。全てpin済み、または他consumer所有なら待機/容量不足を返し、他者の登録を削除しない。本文保持・token quota・snapshot byte quotaを混同しない。managed sourceの配送経路が未成立なら、この段階を未完了とする。

## 9. GenerationごとのView構成

### 必須入力と任意入力

base messagesにはsystem/developer policy、現在発話、対象task/scopeの必須状態、未反映の訂正、必要なRuntime事実を含める。GenerationManifestの依存集合にはbase中の引用・assertionの出典も含め、登録sourceだけを追跡する方式にしない。state本文は明示的なdataとして包む。原文との重複範囲は除き、出典参照を残す。

登録sourceには変更頻度の低い資料・履歴・成果物の必要範囲を置く。Composerがrequiredとoptionalを決める。requiredは今回の判断に欠かせない根拠、optionalは補助情報であり、27Bや検索scoreに操作権限の判断を任せない。P1は既存参照・task対象・近傍sourceを使い、新しいembedding検索を実装しない。

LARM itemsは現行schemaで1〜512件、同一context ID/versionは重複不可である。必須が多すぎる場合は、出典・分類・順序を保ったexport単位への再構成か、作業の分割を行う。根拠を消して数だけ減らさない。

### 作成・consume

1. 同じread snapshotからGenerationManifestを作り、input epoch/policy revisionとrequest revisionを固定する。
2. 対象releaseの認定、Allocation、source登録を確認する。最新認定値から予算を決める。
3. request本文・tools・stateを固定してrequest digestを計算する。以後の本文変更には再計測と新Viewが必要である。`baseInputTokens`、`maxInputTokens`、deadline、`canonicalizationVersion=context-view-v1`、itemsを付けてViewを作る。計測方法・包装余白はP1-00のcanonical計測契約に従う。
4. orderedItems、omitted、source版、token budget、Allocation/runtime/release/lease epoch/期限を照合する。requiredの欠落を許可しない。LARMのcanonical順序をSAAAが別の並びへ勝手に置換しない。
5. 同じAllocation、View ID、認定されたmodelとcapabilityを指定してChat requestを送る。現行KV:mem READMEは `x-larm-allocation-id`、`x-larm-context-view-id`、`x-larm-capability: llm.coding` を指定する。別用途のcapabilityはroute認定を確認し、名称を推測しない。
6. Viewはone-shotとして扱う。作成requestの再送とgeneration再試行を分ける。同じ作成冪等キーのreplayがexpired/consumed Viewを返した場合、新しいattemptと作成キーを発行する。応答のstream終端とTask台帳を別に記録し、次のgenerationでは新Viewを作る。

最終入力予算は `min(SAAA要求上限, context limit - output reserve - safety margin)` で、現在最大125,000 token。LARMは包装・chat templateを含むmaterialize後の実入力を再計測するため、source tokenの単純合計だけでは合格にならない。出力上限も4,096 token以内とし、出力を小さくしただけで入力上限が増えるとは扱わない。

失敗時の扱い:

| 状況 | 動作 |
| --- | --- |
| optionalが省略 | omittedを記録し、必要な根拠が残っているかを確認して続行 |
| required欠落・予算超過・tokenizer不一致 | 作成/実行を止め、再登録・範囲分割・追加照会。必須状態を黙って落とさない |
| sourceなしの軽い要求 | itemsを捏造しない。認可済みbaseのみの27B通常推論として区別し、KV:mem利用とは記録しない |
| View expired/consumed/invalid、release/lease変更 | 新しいbindingで新Viewを作る。旧View IDを再consumeしない |
| snapshot miss/破損/非認定 | 有効sourceによるrebuildを使う。current stateやTask進捗を失わない |
| consume前後のtimeout | Context operationと応答/runを別照合。materialization成功だけでTask成功にしない。操作の自動再実行なし |
| source/権限の変更 | 未使用Viewの採用を止め、対象generationの出力を拒否。第11節の手順へ |

state抽出の根拠allowlistは、baseと実際にmaterializeされたことを確認できるsourceだけにする。View作成時のoptional候補はその後省略され得る。P1の抽出generationでは選定後の全sourceをrequiredに固定して必要な入力を保証する。Task同梱候補が未確認optionalを根拠にした場合は採用せず、該当sourceを明示した別抽出へ回す。分類・忘却のinput_dependenciesは根拠allowlistより狭めない。

snapshotを最大限活用するため、安定したsource ID・版・構成を保つ。ただしprefixや動的baseの変更とreuseの適合性は認定条件に従い、hitのために古い訂正を残さない。P1の正しさはsnapshot OFFでも成立させる。

## 10. 27Bの状態抽出worker

状態更新は、確定sourceの差分から行う。20M全体の定期再処理をしない。27BのTask結果にStatePatch候補を同梱できる場合も、通常workerと同じ検証経路を通す。回答や実行成功の保存を、state候補の採否待ちで止めない。

抽出用途は `personal_state_extract` として既存Provider Adapterへ明示設定し、認定された27B経路を使う。タスク用Viewを再consumeせず、抽出にも必要範囲と独立したgenerationを用意する。2Bやクラウドへ暗黙にfallbackしない。

初期上限はworker同時1、idle30秒後、1 jobでLLM1回・wall time30秒・出力2,000 token。P1-00でSAAA内の会話/Taskと共有schedulerへ接続し、foreground復帰時は新claimを止めて実際のHTTP requestへcancelを送る。activity確認だけで安全な予約が取れたと扱わない。取消未確認の呼出しを重ねて並列化しない。

jobはscope/input epoch/source版・範囲集合/extractor versionで識別し、lease generationを持つ。失敗は初回＋最大3回、backoff30秒/2分/10分とし、idleを再び満たしてから実行する。foreground中止・新epochによる競合は失敗回数と分け、連続中止・pending最古時刻を診断する。

入力はactive/candidate状態、未処理source、必要な出典のallowlist。出力は最大32操作、各値2,000 UTF-8 bytes、全体16KiBを構造上限とし、2,000 output token上限が先に効けばそれに従う。不正enum/field/source/既存ID参照、過大出力を拒否する。新assertion IDはSAAAが発行する。

意味不明な置換は候補に残す。source roleがuserであっても、引用・仮定・否定を無視して明示決定にしない。frontierを進めるために失敗をNO_CHANGEへ変換しない。失敗後も現在発話・pending差分・Runtime通知で対話を継続し、必要条件が欠けるTaskだけを保留する。

起動時は期限切れleaseを再評価し、適用済みpatchを検出する。現行 `cancel_unhandled_jobs` で新workerのjobを一括取消ししない。永続jobに本文を複製せず、source参照と有効性から再読込する。

## 11. 訂正・取消・忘却の状態機械

| 操作 | 状態への効果 | 実行への効果 |
| --- | --- | --- |
| 訂正 | 新しい根拠・assertion/transitionとrequest revision | 古い前提の次操作/出力を止めて再計画。全履歴の忘却ではない |
| 撤回 | 主張をretractedにする | その主張を根拠とする判断を再評価 |
| 取消要求 | Runtimeのcancel_requested | HTTP/外部操作へ停止要求。完了とは別 |
| 取消完了 | 停止を確認したRuntime状態 | 遅延結果を公開しない。既に実行済みの副作用は残る |
| 忘却 | tombstone、本文・派生状態・登録物の利用停止と削除 | 影響runを取消し、残る根拠だけで再構成 |

忘却は以下の順序を不変条件にする。

1. SAAA writerでtombstone・input epoch・逆引き対象・forget outboxを確定する。以後、対象sourceと依存するprojectionを利用禁止にする。
2. 影響run/attemptの出力と新規dispatchを無効化し、AbortSignalを送る。Rustでは既存RunCancellationを実HTTP/stream cancellationへ接続する。取消確認を待つ間も対象出力は拒否する。
3. 遅れて届くtoken、tool call、StatePatch、TTS送出待ち内容をrun/request/epochで破棄する。既に再生・実行したものを巻き戻せるとは表示しない。
4. LARMの対象Context登録を冪等に削除する。provision済みsource本文・attestation・snapshotのcleanupも別の追跡項目とし、未確認ならremote_cleanup_pendingを残す。
5. 削除完了を確認した範囲の残存sourceから登録集合を再構成し、新しいViewを作る。旧sourceを含むView/snapshotは再利用しない。未完cleanupのsourceへ依存する推論は再開しない。
6. 変更履歴、派生assertion、backup復元、送信待ち、snapshot miss/rebuildの各経路から復活しないことを検証する。

SAAAのtombstone commitと、source送信・tool dispatch・結果本文保存・TTS/表示enqueueの許可を、同じ短いローカル境界で順序付ける。forget通知で既存のUI投影・メモリ内cache・未送出queueも無効化し、古い画面snapshotの再描画から復活させない。ネットワーク中にDB lockを保持しない。tombstoneより前に既に外部へ渡した処理はin-flightとして取消・結果破棄を追跡し、後の新規受付は拒否する。この境界を受入試験の起点とし、ネットワーク内の既送信bytesや再生済み音声を消せるとは保証しない。

HTTP取消の送信を確認できればLARM登録削除へ進めてよいが、遠隔generationの停止確認とは別状態で追跡する。実行状態が不明な間は同じ外部操作を再試行しない。forget outboxは対象登録incarnationと各段階を永続化し、どこで落ちてもtombstoneから再開する。登録成功の遅着・再試行がないことも照合してから完了にする。LARM Context機能OFF中はDELETEがno-opになり得るため、HTTP成功だけでcleanup完了としない。有効な登録一覧/状態で不在を確認できなければpendingを維持し、機能再開時にも再照合する。

削除依存をsource参照だけでなく、派生pack、回答artifact、extractor入力、export単位まで追う。影響を証明できない場合は、そのrunの全入力依存を保守的に対象にする。登録DELETEの成功は物理消去を意味しない。管理対象外backupを含む完全消去とは報告しない。

忘却対象がbase messagesにだけ含まれていた場合もsnapshotの再利用制限へ反映する。現在の認定/公開APIで影響snapshotを確実に無効化または回避できると確認するまで、その影響範囲のmanaged推論は再開しない。Context登録を新しくしただけで古いbase由来の情報が無効化されたと推測しない。source-rebuildへの切替方法が外部契約にない場合はP1-00の依存未完了として扱う。

## 12. 起動・無効化・診断

起動時はmigration検証後、tombstone/policyとforget outboxを先に適用し、current projectionを履歴から再構成できることを確認する。元sourceが失われた項目はstale/invalidatedとする。古いView IDは復元せず、実行台帳照合後に必要な新Viewを作る。

`SAAA_MEMORY_ENABLED` は抽出とPersonal Stateの追加投影を制御しdefault OFFを維持する。OFFでも既存27B回答経路、2B shadow、Runtime取消、原文保存と既存recallは動く。既に作られた状態・登録物のforget/認可はOFFにしない。LARMのContext/snapshotスイッチと同一視しない。

rollbackは追加投影と抽出を停止して既存経路へ戻す。DB down migrationや旧snapshot復帰で忘却を巻き戻さない。起動時適用flagなら再起動が必要と表示し、即時停止したふりをしない。

診断は状態revision/input epoch、pending件数/bytes/最古時刻、出典・分類、登録quota、View作成/省略/失効理由、materializationとgenerationの別結果、snapshot hit/miss、cancel/cleanup状態を示す。本文は本人向け認可ビューに限り、通常会話に内部IDを露出させない。

## 13. 作業順序と成果物

| 作業 | 実装対象 | 完了条件 |
| --- | --- | --- |
| P1-00 契約固定 | 現状調査、LARM配送/Allocation/認可/取消、baseline | 第2節の契約表と不足依存を固定。2B・KV前提を推測しない |
| P1-01 コア | 新crate、型/reducer/依存/認可 | offlineで競合・忘却閉包・対象分離・投影試験が成功 |
| P1-02 保存 | control_plane差分migration、projection、outbox | 旧データ移行、再起動再構成、source全件対応、冪等性が成立 |
| P1-03 LARM接続 | 原文の範囲loader、source配送・登録・View・GenerationManifest | 契約fixtureでone-shot、required、予算、release変化、source再構築を検証 |
| P1-04 Runtime/応答 | 委譲・定型通知・出力fence・取消・forget | 2Bの役割逸脱なし。影響run停止、遅延出力拒否、remote cleanup追跡が成立 |
| P1-05 抽出 | 27B worker、初期取込・分割最終化、coverage、retry、CAS | 有限jobと未反映訂正の両経路を検証。並行generationを暗黙に増やさない |
| P1-06 診断 | 設定/診断/必要なIPC | 本人向け状態・根拠・失敗・削除進捗を確認できる |
| P1-07 受入 | 固定corpus、offline/live runner、evidence | 第14節の全必須gateを満たす |

P1-00 → P1-01/02 → P1-03/04の最小経路 → P1-05 → P1-06/07の順に進める。P1-03の配送・View経路はP1-04の認可・出力fence・forgetが成立するまでは合成fixture/隔離データで接続し、実ユーザーsourceを送らない。各段階の試験は実装時に行い最後へ先送りしない。source配送が別作業ならP1-03をその完了に依存させる。既存機能と同等の新基盤は作らず、足りない契約だけを追加する。

## 14. 受入行列

下記は計画上の基準である。認可・整合性違反は許容0件。モデルの意味精度と性能は別に判定し、live未実施をoffline成功で代用しない。

| ID | シナリオ | 合格条件 | lane |
| --- | --- | --- | --- |
| C1 | 明示制約・訂正・撤回のView投影 | activeな必須項目の欠落0、撤回復活0。任意項目の省略理由を保持 | offline＋日本語 |
| C2 | 重複/out-of-order/新input epoch/lease取消後/Task同梱patch | 二重適用・古い上書き0。未知/未来source・循環依存を拒否。根拠allowlist外を採用しない | offline |
| C3 | 同scopeの別task/request、別principal、別用途 | 条件・本文・結果の漏洩0。権限緩和0 | offline＋日本語 |
| C4 | sourceの途中削除、派生の派生、一部根拠の喪失、valid_until到達 | 推移的な失効漏れ0。残存根拠による再評価と無断復活を区別 | offline |
| C5 | forgetの各段階直前直後のcrash、登録送信中/生成中/送出直前削除、baseだけに存在する根拠の削除 | tombstone後にSAAAが新規許可する本文保存・出力・再登録・dispatchが0。先行in-flightを取消追跡。cancel/cleanup未確認を完了と表示しない | offline＋live |
| C6 | DB再起動、projection再構成、旧backup復元 | 有効状態が一致しtombstone再適用前の利用0 | offline |
| C7 | View期限切れ/consume再送/作成replay/release/Allocation/lease変更 | 不正binding採用0、View再利用0。新Viewで再構築 | offline＋live |
| C8 | 125,000境界、byte上限、512 items、20M quota | 必須の黙示省略0、上限超過0。登録解除とforgetを混同しない | offline＋live境界 |
| C9 | snapshot hit/miss/破損/無効化、source version変更 | 正しいsourceと状態で再構築。hit/missで権限・必須意味を変えない | offline＋live |
| C10 | 2Bの自由文・事実回答・約束・更新・曖昧routing | 禁止出力の利用0。未認定はshadow/定型へ戻る | offline＋日本語 |
| C11 | materialization成功後のgeneration失敗/長さ上限/取消 | Task成功の誤記録0。応答・操作を無条件再実行しない | offline＋live |
| C12 | Memory OFF、ContextStill停止、LARM不通、LARM Context OFF中のDELETE | 既存経路/定型通知とforgetを維持。不完全状態をhealthyと偽らない | offline＋live |
| C13 | 登録解除後の再登録・登録と解除の競合・遅い成功replay | incarnationを区別し、active誤認・孤立複製の見落とし0 | offline＋live |
| C14 | 同runのtool往復/回答/抽出/再試行、途中の進捗質問 | generation混同0。有効なTask結果を無関係な質問で失わず、忘却結果の本文は保存しない | offline＋日本語 |
| C15 | 初回ON、400件超の履歴、長文の後半否定/中間訂正、編集での版更新、連続入力によるpending超過 | export/抽出の黙示欠落0。旧sourceで現在の訂正を巻き戻さず、不完全性を明示 | offline＋日本語 |
| Q1 | 否定・引用・仮定・局所条件・未決・複数根拠の抽出 | 下記固定corpusの品質閾値を全反復で満たす | 日本語 |
| P1 | 受付、27B応答、worker、ASR/TTS干渉 | 下記遅延・資源gateを満たす。未認定同時generation0 | live |

### 日本語corpusと比較

最低60シナリオを用意する。否定・引用・仮定・訂正/撤回・局所条件/同名対象・未決・複数根拠/失効・2B役割境界の8カテゴリを各最低5件含め、残りで割り込み・再開・連続訂正を組み合わせる。人が正しいassertion、対象、status、sourceの支持範囲、禁止される出力、各チェックポイントの評価期限を固定する。ASRは確定テキストから評価し、音声認識誤りと分ける。各シナリオを3回実行し、反復ごとに評価する。

必須目的/制約/未決の抽出recall95%以上、採用assertionの根拠支持precision95%以上、未抽出率5%以下、出典ID/版/範囲一致100%。採用決定の捏造・撤回復活・権限昇格・2B禁止応答の採用は0件。抽出済みの正しい必須項目をComposerが落とすことは0件とし、抽出品質の95%とは分ける。goldがactiveを要求する項目をcandidateへ留めた場合はrecallの正解数に含めない。gold自体がcandidate/pendingを要求する場合は正しい不確実性の表現として評価する。

評価単位は固定チェックポイントの(kind、対象、値、期待status、支持source範囲)とし、goldと一対一で照合する。recallは正しく一致した必須項目数/gold必須項目数、precisionは正しく支持される投影項目数/投影した全項目数とする。未抽出率は期限内にcoverageを確定できなかった評価対象source数/対象source数として別計測し、部分処理は未完了に数える。fixtureで許可する同義表現・支持範囲も事前に固定し、モデルの自己採点は合否に使わない。

同じ2B/27B、release、原文、View予算、Task条件で「契約準拠のSource/View利用のみ」と「同構成＋Personal State」を比較する。短い旧Contextとの比較だけで有益と結論しない。訂正・再開・forgetの固定チェックポイントで品質を維持し、少なくとも一つの事前指定した継続性指標を改善する。既存構成が全て満たす場合は追加抽出を省く選択を含めて再評価し、無意味なworkerを必須にしない。その場合は範囲とgateの変更理由を文書へ反映してから完了判断する。

### 遅延と資源

受付のfirst playable audio、27B first token、最終応答、抽出開始待ちを別にp50/p95で記録する。snapshot cold/rebuildとhitを混ぜず、意味試験の3回だけでp95性能を認定しない。条件ごとに最低100要求の有限runを用意し、外来workloadの混入・失敗数も残す。正常系のtimeout/予期しない失敗は0件をgateとし、成功分だけのp95で合格にしない。意図的な取消・障害試験は正常系のlatency集計と分離する。

SAAA管理のLLM generation（2B/27B/抽出を含む）は同時1、worker1 job/1 call/2,000 output token/30秒を上限とする。foreground復帰からcancel送信までのp95目標は100ms以内とし、遠隔停止時間とは分けて測る。追加機能ON時の受付・27B TTFTのp50/p95は、同じcold/warm条件のbaselineに対して10%以内の悪化を初期上限とする。数値は提案gateであり、P1-00で実機条件と必要な絶対上限を事前固定する。結果を見た後の緩和で合格にしない。

peak RAM/VRAM、CPU、token/sec、Source Set token/bytes、snapshot bytes、ASR RTF、TTS first playableも記録する。releaseの認定資源上限を超えない。上限が未定義ならP1-00で固定し、性能認定を保留する。並列化はこのP1のdefaultに含めず、別mixed認定で判断する。

## 15. 検証コマンド・記録・リリース

実装後のoffline対象:

```sh
cargo test --locked --manifest-path crates/personal-state-core/Cargo.toml
cargo test --manifest-path src-tauri/Cargo.toml memory::
cargo test --manifest-path src-tauri/Cargo.toml runtime::
bun run test:rust-packages
```

新crateを標準test/format/clippy対象へ登録する。LARM/Provider Adapter試験が上記から漏れる場合はP1-03で正確な追加コマンドを記録する。期待結果はC1〜C15のoffline項目成功であり、失敗時は該当Adapter/reducerを修正する。

full checks:

```sh
bun run check:local
bun run build
bun run desktop:smoke
bun run spec:check
```

IPC変更時は `bun run ipc:generate`、system context変更時は `bun run s11tnext:build` も実行する。full check失敗はbaselineとの差を区別し、本変更の未解決失敗を残して完了しない。

P1-07で有限の日本語/live runnerを作り、起動コマンド、corpus版、モデル/release、policy/prompt/schema、token/byte予算、sample数、timeout、結果、baseline差、未実施理由を `spec/evidence/personal-state/` へ保存する。実ユーザーの本文・credentialはfixture/evidenceへ書き出さない。

live不足・外部契約不足は未検証/依存未完了とする。snapshot OFFで正しさを確認し、その後ONで再利用・遅延を測る。機能コードを実装してから方向性を比較し直すのではなく、P1-00で接続可能性と受入条件を固定して進める。

全gate通過後もisolated canaryとdefault OFFを維持する。default変更は別release判断とし、実装commit・受入結果・残る制約を両文書へ反映してP2の詳細計画へ進む。この文書改訂だけで機能を有効化しない。
