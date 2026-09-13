# Personal State P1 — 2B会話フロント・27Bタスク担当間の継続状態 実装計画

状態: レビュー用実装計画。2026-09-13。全体方針は[ロードマップ](personal-state-architecture-roadmap.md)を参照。

## 1. 完了時の成果

単一のSessionless会話において、現在の目的、有効な制約、決定、未決事項、訂正を、Window切り替えとアプリ再起動後も引き継ぐ。後から届いた背景処理が訂正を巻き戻さず、保存・抽出失敗を欠落として隠さない。

2B級モデルは会話の受け付け・確認・進捗伝達、27B＋20M KV:memは詳細文脈を使ったタスク遂行を担当する。両者はSAAA管理の現在状態を用途別に参照し、KV自体を共有しない。2Bが会話を続けている間の訂正や取消も、27B側の作業へ対応付ける。

例: 「VoiceMem導入を検討」→「採用は未決」→「独自コア案で計画を作る」という会話を、次のWindowでVoiceMem採用決定として再開しない。

P1をP1-Aの決定的な状態基盤と、P1-Bの実際の差分抽出・Context接続に分ける。P1-Aだけでは対話の継続性を実現した完了報告をしない。P1-Bにはworkerだけでなく委譲・返却・三経路のContext接続を含む。両方とisolated canaryを終えた状態がP1完了である。

## 2. 範囲

含む: 既存委譲・返却経路への接続、KV受領確認・容量切替・回復、独立Rustコア、既存SQLiteへの差分拡張、source対応、単一scopeのstate、永続worker、モデル出力の検証、Context投影、起動回復、失効・削除、最小health表示、回帰・日本語fixture。

含めない: 新しいSession UI、複数scopeの自動推定、World Model全体、感情・人格の自動推定、ContextStillへの新規送信、音声保存、embedding、汎用RAG、長期Goal管理、Taskの自律生成、VoiceMemの変更。

ContextStill recallは既存動作を維持する。P1のstate更新はContextStillを呼ばずに成立する。

### 20M LARM経路を先に成立させる

ユーザー提供の既実装前提と保証未確認事項は[ロードマップ第7節](personal-state-architecture-roadmap.md)に従う。P1の優先順序を「既存LARM継続契約の確認 → 長大Contextを壊さない接続 → 必要最小限の永続状態と引き継ぎ」とする。LARM内部のKV基盤を新規実装する計画ではない。

P1-00で、モデル/profile識別、cache identityとsource coverage、実使用量と上限単位、追加投入とprefix変更、再接続・モデル切替・削除時の挙動を確認する。未提供の契約はSAAA側で推測せず不足として記録する。P1-00の成果物は契約表とし、各項目に既存API/コード根拠、保証範囲、試験、担当Adapter、未対応時の動作を付ける。受領確認・容量計測・新Context作成・実行照会が不足する場合は接続部分を未完了とし、モデルなしのコア作業のみ進められる。LARM側の契約追加が必要なら別作業として切り出し、短いContextへの退避を20M連携の完成とは扱わない。27Bタスク担当の既存KVを保ったまま状態差分を使えるかを、worker構築より先に検証する。

P1-05は、通常の2B会話用投影、27Bタスク用KV継続、cache喪失等からの再開の三経路を持つ。LARM継続時は既存の詳細文脈と新規差分を使い、同一sourceの再投入や毎回の履歴要約・prefix再構成を避ける。他モデル、cache喪失、容量切替時は以下のbounded Projectionを使う。モデル変更でKVが移るとは扱わない。状態抽出はタスク用cacheを汚染しない分離呼出しとし、全20Mの定期再処理を避ける。

容量切替は上限を越える前に開始し、引き継ぎstateのrevision、処理済みsource、未反映差分、必要な詳細出典を記録する。削除済みsourceを含むcacheは再利用対象から外す。Memory OFFはPersonal Stateの停止であり、独立したLARM KV継続機能を暗黙に停止する設定にしない。

### 最小の委譲・返却契約

既存のreasoning / task起動・結果返却経路をP1-00で特定し、以下の不足だけをAdapterとして追加する。新しい汎用タスクエンジンは作らない。

| 契約案 | 内容と扱い |
| --- | --- |
| TaskInput | 既存task/run ID、request revision、scope、確定入力source、state revision、未反映訂正、既存権限参照。原文を保持し2Bの要約だけで委譲しない |
| TaskUpdate | task/run ID、request revision、event sequence、台帳状態参照、結果・成果物参照、確認要求、根拠、任意StatePatch。進捗と意味状態のcommitを分ける |
| FrontProjection | 2B向けの現在目的・制約・未決、最新訂正、台帳進捗、伝達すべき結果・確認要求、根拠・鮮度・容量不足。27Bの詳細文脈全体を入れない |
| TaskContextBinding | model/profile、cache identity/generation、投入済みsource、受領確認、容量情報。state抽出frontierとは別に追跡する |

これらは既存型の拡張案であり、同等契約があれば再利用する。P1は単一scopeだがtask/run IDは省略しない。複数実行が既存Runtimeに存在する場合も相互の結果を混ぜず、任意の一件へ上書きしない。

SAAAが入力保存後に冪等な委譲要求を発行し、Runtimeの受付を確認する。2Bの受付発話とタスク開始確認を区別する。返却イベントは既存台帳へ記録された順序で投影し、旧request revisionの結果は現在の成果として採用しない。再起動後は台帳と実行状態を照合してから再接続し、未確認の実行を自動で重複起動しない。

取消と明示訂正は保存・Runtime通知・次の2B投影へ即時反映し、30秒のidle抽出を待たない。停止要求が届いても既に終わった操作は取り消せないため、実際の到達状態を返す。次の操作は既存Runtimeの実行境界で依頼版とPolicyを確認する。モデルへの訂正追加だけで取消完了を保証しない。

## 3. 実装開始時に確認する現行基盤

- `memory/context_window.rs`: bounded Contextとcontinuity groupがある。古い会話の先頭・末尾抽出を意味的なstateと混同しない。
- `memory/control_plane/{mod,items,migrate}.rs`: capsule、working state、source、jobsの土台がある。書き込みの一部が `#[cfg(test)]` のため、単にフラグをONにしてもworkerは動かない。
- `persistence/schema.rs`: 起動時に未処理jobを取り消す現在処理がある。新workerのjobまで無条件に取り消す動作を変更する必要がある。
- `runtime/conversation_inputs.rs`: 一貫したread snapshotからContextを読む。新stateも同じ読み取りへ組み込む。
- `runtime/turns.rs`: 実行と完了の永続化。推論中にここで長いwrite lockを持たない。

実装開始時のHEAD、schema version、dirty状態、既存テスト結果を記録する。schema番号は実装時の最新値から採番し、本計画では15等に固定しない。既存migrationを遡って変更しない。

## 4. 状態モデル

コアには次の型を持たせる。命名は案だが、責務は契約とする。

| 型 | 必須内容 |
| --- | --- |
| ScopeId | P1ではprimary会話用の単一安定ID。日付・モデル・Windowで変えない |
| StateItem | id、kind、semantic key、値、status、source refs、revision、対象task/requestまたはscope共通、observed/effective/recorded time、任意のvalid_until |
| SourceRef | source種別、正本ID、版/digest、利用可否、時刻、会話ならroleと必要な本文範囲。Runtime event/artifactは既存正本を参照 |
| StatePatch | schema version、patch ID、base revision、input epoch、対象sourceと範囲、operation群、source coverage。frontierはSAAAが導出 |
| StateSnapshot | scope、revision、処理済みfrontier、active items |
| Projection | 必須項目、任意項目、未反映source、容量・鮮度・根拠のhealth |

kindは `objective`、`constraint`、`decision`、`pending_decision`、`open_loop`、`active_referent`、`progress_ref`。statusはcandidate / active / resolved / superseded / staleを基本とし、削除は別のsource無効化として扱う。

同じsemantic keyの更新は、追加・置換・解決・失効を明示する。semantic keyだけで自然言語の同義性が確定するわけではない。既存IDを提示した上でモデルが対象を選び、更新・削除で未知IDを参照したpatchは拒否する。既知IDでも意味的に曖昧な置換は元項目を維持しcandidateとして保持する。新規追加のIDはSAAAが採番する。

P1の単一scope内でも、task/requestに属する条件とscope共通条件を区別する。対象不明の局所条件を全Taskへ配布しない。2Bが「それ」の対象を確定できなければ確認するか27Bへ原文付きで問い合わせる。P3まで延期するのは複数作業scopeの自動整理であり、実行IDによる混同防止ではない。

## 5. 指示・権限の扱い

現在の発話はそのまま応答モデルへ渡す。過去の有効な制約は出典付きの継続情報として保持し、system/developer policyや現在の明示変更に優先しない。

モデルが出す `authority=explicit` のような値を信用しない。sourceのrole・実際の内容・対象scopeとの対応をSAAA側で検証する。引用やツール応答中の命令は過去のユーザー指示へ変換しない。

出典がユーザー発話であることだけで、内容の意味が正しく抽出された保証にはならない。特に引用、仮定、否定、assistant案への保留を日本語fixtureで確認する。操作許可の根拠は既存Policy側に残し、このstateを新たな実行権限として使わない。

## 6. 保存とsource coverage

既存working stateを項目の正本、capsuleを版付き投影とする。両方が別々に現在の決定を更新する構造にしない。コアが決めた同じ差分からtransaction内で更新する。

差分migrationでscope、revision、処理frontier、source coverage、冪等キー等の不足列・関連表を加える。既存制約にないkindを追加する場合は、新表への移行と件数・参照検証を行う。

Raw本文を新しい履歴tableへ複製しない。source windowの先頭・末尾だけでは途中の削除を検知できないため、利用したmessage IDを対応表で列挙する。必要な列はwindow_id / message_id / digest / role / availability / position。会話本文は参照元にのみ残す。Runtime event/artifactのsource対応は同じ参照契約の別種別として持ち、message IDを偽造して会話表へ押し込まない。state itemからsourceへの依存も列挙し、削除時に依存項目を逆引きできるようにする。

現在の時刻とmessage IDのcheckpointをそのまま十分とみなさない。遅れて確定する応答、割り込み、同時刻の入力を順序付けできるsource sequenceまたは同等の厳密順序を導入する。未処理sourceを飛び越えてfrontierを進めない。

移行時の既存active項目は根拠と整合性を再検査する。根拠不足はstaleとし、自動的にユーザー決定へ昇格させない。sourceを復元できない旧capsuleは元会話から再構成対象にする。古いreflection jobは新versionと区別し、対応できないものだけ明示理由で取り消す。

### 入力版と未解決source

state revisionに加え、確定入力・訂正・削除の保存で増えるinput epochを持つ。抽出開始後に新しい入力が入った場合は、state revisionが同じでも旧patchを適用しない。P1はscope単位で保守的に拒否して再評価する。取消されたjobはlease generationも変更し、遅い結果がcommitできないようにする。抽出済みfrontier、Taskへの投入確認、Runtime event sequenceは別々の進捗であり相互に代用しない。

source単位のcoverageは処理範囲を記録する。長いmessageは版とUTF-8境界の範囲で分割し、全範囲を検証するまでは全文処理済みにしない。未解決sourceが残る場合、連続frontierはそこで止めるが、後続sourceの処理結果は個別に保持できる。pendingの取得はfrontierだけでなくcoverageも参照し、処理済みsourceを無限に再処理しない。未解決件の破棄や保持期限による暗黙のNO_CHANGEは禁止する。

取消・訂正の意味解釈が曖昧な場合、保存と原文の伝達だけは即時に行い、対象や新しい値を推測で確定しない。対象不明の変更は変更候補として表示し、関係し得る作業の次の副作用を伴う操作を既存Runtime境界で保留する。確認で対象が決まってからrequest revisionを更新する。明確な取消操作は既存の取消経路へ直接送る。

## 7. P1-A: コアと永続化

1. `crates/personal-state-core/` を追加する。serde等の最小依存で型・純粋なreducer・projection規則を実装する。時刻、ID、予算は呼び出し側から渡し、テストで固定できるようにする。
2. SAAA側にstore Adapterを置く。既存writerとreaderを利用し、LLMやHTTPをコアに持ち込まない。
3. patchの構造、source membership、base revision、状態遷移を検証する。同一patch再適用はno-op、同一IDで異なる内容はconflict。
4. source削除・失効・取消をactive stateへ反映する。SQLite transactionで項目・capsule revision・frontier・job結果をまとめてcommitする。
5. fixtureからpatchを適用し、再起動・重複・競合・削除・容量を検証する。この時点ではモデル呼出しを必要としない。

新crateのtestとlintを既存package検証へ登録する。crateを追加したのに通常checkが検証しない状態にしない。

## 8. P1-B: 差分抽出worker

### enqueue

確定したユーザー入力の保存時に処理待ちsourceを記録する。assistantが中断・失敗した場合もユーザーの訂正を落とさない。応答の確定時には補助sourceを追加し、途中の生成断片は決定の根拠にしない。

入力保存とenqueueは同じwriter transaction、または同じ正本から確実に再検出できるdurable outboxで行う。メモリ内の通知だけを永続処理の根拠にしない。

### claimと実行

単一workerが一jobずつclaimし、lease・attempt・次回実行時刻を保存する。同じscope・input epoch・source版/範囲集合・抽出器versionの重複jobだけをまとめる。frontierが同じでもcoverageが異なるjobを同一扱いしない。foreground run中、会議capture中、ユーザー入力直後は新規claimしない。

既存MVP 3のidle 30秒、job上限30秒、LLM一回、出力2,000 tokenを初期値とする。再試行は初回に加えて最大3回（最大4試行）、30秒・2分・10分を初期backoffとし、foregroundが終了しidle条件を再び満たしてから再開する。これらは設定上限であって性能実測値ではない。入力更新・foreground割り込みによる中止はモデル失敗の再試行回数を消費せず、新しいepochの処理へ移す。ただし連続中止の回数とpending経過時間は診断する。意味未解決はfailedと分け、同じ入力の無限再試行を避けて確認・追加根拠を待つ。

意味抽出は27B側の能力を使う設計とし、2Bフロントへ全履歴の整理を任せない。タスク完了時に得られたStatePatch候補も同じ検証契約を通す。追加抽出が必要な場合のモデル経路は既存Provider呼出し基盤を再利用し、用途を `personal_state_extract` として明示設定する。設定がない場合はworkerを停止し、勝手に会話モデルやクラウドへfallbackしない。資格情報は既存管理から受け取り、DBやjob payloadへ保存しない。

音声優先のキャンセルはclaim停止と既存provider cancellationを使う。返却が遅れた結果もrevisionで拒否できるようにする。cancel不能なモデルの呼出しを繰り返し並列化しない。

タスク担当27Bと同じ計算資源を使う場合は実タスクを優先する。cache分離・資源予算が確認できなければ背景抽出を延期し、pending sourceを保持する。構造化されたRuntime進捗・取消・失敗の反映はLLMを必要とせず、その間も継続する。

### 入力と出力

SAAAがpatch ID・base revision・input epoch・lease情報を呼出しに結び付け、モデルに任意の最新値を申告させない。

入力は対象scopeのactive state、まだ処理していない確定source、変更対象項目の根拠。出力はStatePatchのみ。source IDは入力のallowlistから選ばせる。説明文だけの出力、未知field、過大な文字列、未知enum、未知sourceを拒否する。

最大32操作、各値最大2,000 UTF-8 bytes、patch全体最大16KiBを初期の構造上限とする。モデルの出力token上限が先に効く場合は小さい方に従う。provider input上限を超えるsourceは境界を明示して分割し、複数jobで処理する。切り捨てた本文を全文処理済みとして扱わない。

各sourceを「stateへ反映」「変更不要」「未解決」に分類する。検証済みNO_CHANGEはfrontierを進められるが、例外や未解決をNO_CHANGEへ置き換えない。

### commitと回復

推論後の短いtransactionでbase revision、input epoch、job lease generation、有効フラグ、source availability/digestを再確認する。一致すれば原子的に適用。不一致なら結果を破棄して最新stateから再評価する。LLM出力を自動的に文字列マージしない。

起動時は期限切れleaseをqueuedへ戻す。新workerのjobを `cancel_unhandled_jobs` で一括取消ししない。適用済みpatchは冪等キーで検出する。最大試行失敗時はfailedとして保持し、手動再試行または設定変更後の再試行対象を明示する。

## 9. Context接続と未反映入力

`conversation_inputs` の一つのread snapshotで、active state、frontier、未反映source、現在発話を取得する。state更新完了を待つために通常会話を毎回止めない。

背景処理はユーザーが話し続けると追い付かない可能性がある。pendingの件数・bytes・最古時刻を計測し、投影予算を超えたら容量不足を明示する。タスク用KVが利用可能なら27Bが保持済み文脈と差分から回答し、追加抽出が止まっているだけで正常なタスクを止めない。操作の保留は未解決の変更や必要な条件の欠落がある対象に限定する。その間はcheckpoint後の未反映sourceを、stateより新しい証拠として渡す。新たな訂正がpendingなら、関係する古いstateをcurrent確定値として提示しない。

以下は2B向けFrontProjectionと再開時のbounded Projectionに適用し、27Bの既存prefixを毎回並べ替える規則にはしない。必須順序は、現在発話とpolicy、有効な継続制約、未反映の訂正候補と差分、目的・未決、最近の会話、補助情報とする。同じsourceの二重投入は防ぐ。正確な順序・容量判定をコアのprojection testsにする。

必須項目が予算に収まらない場合は `continuity-incomplete` を返す。黙って落としてhealthyにしない。現在発話と既存の安全な会話投影は維持し、通常の質問には応答できるようにする。ただし欠けた過去条件を前提に「そのまま実行」と続ける判断は保留し、必要な元会話の取得・再確認へ進める。詳細取得は既存 `recall_conversation` を使う。

Context policyは継続状態の役割を明記する最小変更に限る。外部資料や履歴を新しいsystem命令として埋め込まない。

2Bの回答に必要な詳細が投影にない場合は、既存の委譲経路で27Bへ問い合わせる。回答待ちの間も2Bは受付・確認・台帳上の進捗を伝えられる。27Bへの確認が必要な応答の待ち時間と、2B単独の会話応答時間は分けて測る。27B停止時は受付済み・実行未開始・結果不明を正確に伝え、2Bをタスク代替モデルへ暗黙に切り替えない。

### KV投入と容量切替

KV追加はcache generationとbatch ID、source版・範囲を記録して送る。受領確認後だけ投入済みにする。timeout時は同じbatchの状態を照会し、重複排除が保証される場合だけ同一IDで再送する。確認不能ならbindingをunknownにし、新規の作業実行を進めず再接続・再構築へ移る。HTTP成功やlease更新だけをKV保持の証拠にしない。

容量判定は `現在使用量＋次の入力＋出力予約＋制御・引き継ぎ予約 <= 実効上限` とし、同じtokenizer/単位で比較する。2Bの予算、27B総容量、抽出呼出しの予算を別設定にする。上限・使用量が不明なら継続可能と推測しない。P1-00で予約値と計測方法を固定する。ツール結果も入力予算に含め、過大な結果は既存artifactとして保存して必要範囲を渡す。

切替は以下の順で行う。

1. 対象bindingの新規投入と次の実行操作を止め、既存runを安全な中断点または終了まで追跡する。2Bの会話受付は継続する。
2. cutover source位置、state revision、未解決coverage、task/run実状態、必要な成果物・詳細source参照を回復manifestへ保存する。本文を別のRaw台帳へ複製しない。
3. 新generationへ有効な状態と必要な詳細を投入し、受領を確認する。切替中の新着入力・訂正はdurableに保持し、新generationへ順序付きで渡す。
4. 削除・input epochとrun状態を再検査し、active bindingを原子的に切り替える。切替済みcutover位置より後の新着入力は次の差分として残し、全入力が止まるまで切替を待つ方式にはしない。切替直前までの削除と対象Task変更は再検査して反映する。旧generationからの結果を現行結果として公開しない。

新Contextの準備失敗は切替未完了として保持する。旧Contextへ戻せるのは容量・source有効性・実行状態が確認できる場合だけである。旧・新の20M Contextを同時保持できるとは仮定しない。メモリ予算が一つ分ならmanifestを先に永続化して旧cacheを解放してから新規作成し、その区間はタスク再開待ちとする。

KVの喪失からは明示状態・保存済み成果物・必要な出典を再構成できるが、KVだけに残った内部の推論状態や20Mの全詳細を無損失復元できるとは保証しない。回復に必要なタスク成果物・進捗は既存Runtimeの保存先へ適宜確定する。

## 10. 削除・無効化・機能OFF

sourceの中間messageを削除した場合も、対応表を通じて影響項目を投影対象から外す。複数sourceの一部だけが失われた場合は根拠不足としてstaleにし、残存sourceから再評価する。

処理中sourceが削除された場合はcommitを拒否する。依存する2B投影・送信待ち結果・KV bindingも無効化し、生成中またはTTS送出待ちの旧内容を停止する。既に出力した音声を取り消せるとは扱わない。P1は影響cache全体の再利用を止める方式を基本とし、遠隔の物理消去保証はLARMの確認結果に従って別途表示する。Memory OFFでもsource削除の無効化は停止しない。過去revisionをそのまま再有効化せず、現在の削除台帳・source可用性を確認する。source本文を含むjob payloadを別途永続化しない。

既存 `SAAA_MEMORY_ENABLED` をmaster switchとして使い、workerとprojectionを同時に制御する。OFF時は新claim・状態投影を止め、進行中結果の採用も防ぐ。既存flagが起動時適用なら、必要な再起動をUIへ明記し、即時停止したように見せない。Raw会話と既存recallは維持する。既存の2B→27B委譲、Runtimeの進捗・取消、LARM KV継続はこのflagから独立して維持し、Personal State由来の追加投影だけを外す。

## 11. 最小の確認・診断UI

大きなMemory画面は作らない。既存の診断または設定の導線に、状態revision、処理frontier、pending件数、最終成功・失敗理由、継続性healthを表示する。診断では2B/27Bの役割別接続状態、task/runと依頼版、cache generation、投入済みsource、容量、返却の鮮度も追えるようにする。内部診断の追加は利用者の通常会話へ実装用語を露出させない。

activeな目的・制約・未決事項と根拠を確認できる読み取りビューを用意する。訂正は通常会話で行い、P1で任意JSONの編集UIは作らない。ユーザーの訂正がまだ反映されていないことを表示できるようにする。

## 12. 変更対象と作業分割

| 作業 | 主な変更箇所 | 完了条件 |
| --- | --- | --- |
| P1-00 現状・契約固定 | 本計画、既存reasoning/Task経路、LARM接続、evidence | 2B/27Bの接続・委譲・KV継続・容量契約と不足を記録。既存構成のbaselineを固定 |
| P1-01 コア | 新crate、Cargo依存、package検証 | fixture reducer/projection、format、clippyが成功 |
| P1-02 保存・移行 | memory/control_plane、persistence/schema | source全件対応、冪等、移行、削除の試験成功 |
| P1-03 source・委譲・返却接続 | 入力保存、既存Task/Runtime、reasoning Adapter | 原文・依頼版・実行IDで委譲し、重複と旧結果を拒否。訂正・取消・進捗をidle待ちなしで反映 |
| P1-04 worker | 新memory workerとprovider Adapter | lease、取消、再試行、NO_CHANGEが機能 |
| P1-05 三経路のContext接続 | context_window、conversation_inputs、既存LARM Adapter | 2B投影・27B差分投入・再開を分離し、容量境界と未反映訂正を扱う |
| P1-06 最小診断 | settings/diagnosticsと必要なIPC | 状態・根拠・失敗が確認できる |
| P1-07 受入とcanary | 日本語fixture、契約試験、evidence | 下記受入条件と実機検証を満たす |

実装順はP1-00 → P1-01/02 → P1-03 → P1-05の委譲・KV継続の最小接続 → P1-04 → P1-05の回復完成 → P1-06/07とする。番号順でworkerを先に作らない。P1-00で確認した既存契約は再実装せず、未充足項目だけを各作業へ割り当てる。

既存test-only関数を一括でproduction化しない。必要な機能をコアとstoreへ移し、試験も公開契約を通す形へ整理する。module size・generated IPC・system contextの既存ルールに従う。

## 13. 受入条件

### 決定的試験：モデルなしで必須

- 同一patchを2回適用しても項目・revisionが余分に増えない。
- state revision不変でも新入力・削除・job取消後の旧patchは拒否される。未解決sourceの後続処理、長文部分coverage、繰り返し再開で欠落・無限再処理がない。
- 同じbase revisionからの競合更新は片方だけ適用され、古いpatchが新しい訂正を消さない。
- assistantの提案だけでユーザー決定や操作許可が生成されない。
- 再起動後にactive stateとpending入力が復元される。適用直後の停止でも二重適用しない。
- source window途中の削除、job実行中の削除、失効が次回投影へ反映される。
- response失敗・キャンセル後も、その前に確定したユーザー訂正を処理できる。
- 必須情報の容量超過をhealthyと報告しない。未処理入力を処理済みとして飛ばさない。
- Memory OFFとContextStill停止時に従来の会話・recallが動く。

### モデル間連携試験

- 委譲には原文sourceと依頼版が残り、2Bが落とした否定・条件を27Bへ渡せる。
- 進捗照会は台帳の状態を返す。27Bの完了文だけで成功を記録しない。
- 実行中の訂正・取消はidle worker停止中も伝わる。古い完了通知・重複通知・順序逆転で現在状態が戻らない。
- 2BのWindow更新後も実行中taskと確認事項が残り、27BのKVは継続する。
- 27B停止時に2Bが完了を捏造しない。再接続・SAAA再起動で副作用を伴うTaskを二重起動しない。
- FrontProjectionの容量上限は2Bの実モデル契約で判定し、20Mを流用しない。タスク用cacheと抽出用呼出しを混同しない。

### 日本語の意味試験：採用モデルで必須

少なくとも30本の固定シナリオを作り、各々に正しいstateと禁止されるstateを人が明記する。否定・訂正・未決・引用・仮定・一時条件・同名対象・応答中断に加え、2Bによる受付、27Bへの委譲、作業中の訂正、進捗質問、結果伝達を含める。音声は確定ASRテキストとして入力し、音声モデルの誤認と分離する。

保持率は人が定義した必須項目のうち、各チェック時点で正しい値・対象・状態として投影された件数の割合とする。余計な決定の追加は正解数に含めない。出典ID一致に加えて、引用された内容が主張を支持するかを人のラベルで判定する。各反復で閾値を満たし、平均だけで失敗を隠さない。

初期gateは、採用決定の捏造・撤回の復活・操作許可への昇格が全試行0件、必須目的/制約/未決の保持率95%以上、情報の出典ID一致100%。各シナリオを3回実行し最悪結果も残す。この閾値は計画上の受入基準であり、達成済みの主張ではない。

比較対象は現行の短いContext投影だけでなく、既存20M LARMを継続使用する構成も含める。同じ2Bフロント・27B担当・履歴・委譲条件で「既存2B/27B連携＋LARM継続」と「同構成＋Personal State」を比較し、他モデルへの引き継ぎと容量到達後の再開は別条件で評価する。実使用Context量、cache再利用・再計算、会話遅延、抽出コスト、詳細情報の保持を記録する。短いContextとの比較だけでメモリの優位性を結論しない。受入前に境界・復旧シナリオを固定し、既存構成が失敗した必須項目の少なくとも一件を改善し、他の必須項目と禁止事象を悪化させないことを差分gateとする。既存構成が全項目を満たす場合は追加抽出の必要性を再評価し、そのままP2へ進めない。モデル出力をfixturesへコピーしただけの自己一致評価はしない。

### LARM継続・境界試験

cache再利用、重複投入防止、再接続時の保持有無、モデル変更、上限手前での引き継ぎ、通常訂正の差分反映と削除後の旧cache無効化を確認する。境界ロジックは小さい模擬容量でも決定的に試験するが、それを20M実機検証の代用として報告しない。20Mの実容量と長距離参照品質、継続時の速度は別々に記録し、容量があるだけで全履歴を正しく使えると判定しない。

KV送信直後・受領記録直前・active binding切替直前直後の停止を試験し、旧結果の採用・入力欠落・Task重複起動が0件であることを確認する。切替中の訂正と削除も含める。

### 実機canary

isolatedデータと明示モデル設定で、連続会話、idle整理、foreground復帰、アプリ再起動、provider失敗を通す。2B単独で答えられる通常会話のcritical pathへ抽出目的のネットワーク呼出しが増えていないことをtraceで確認する。

worker停止待ちや資源競合による会話遅延も、同じ端末・モデル・履歴のbaselineと比較して記録する。速度の数値目標はbaseline採取後、canary開始前に追記し、結果を見てから都合よく決めない。

## 14. 検証コマンドと証跡

P1実装後のtargeted checks:

```sh
cargo test --locked --manifest-path crates/personal-state-core/Cargo.toml
cargo test --manifest-path src-tauri/Cargo.toml memory::
```

委譲・返却の変更には `cargo test --manifest-path src-tauri/Cargo.toml runtime::` と `bun run test:rust-packages` を追加し、既存reasoning/LARM契約試験も通す。

必要なfull checks:

```sh
bun run check:local
bun run test:rust-packages
bun run build
bun run desktop:smoke
bun run spec:check
```

意味試験・実機canaryのrunnerはP1-07で作成し、正確な起動コマンド、fixture版、両モデル設定、timeout、出力先をevidenceへ記録する。runner未作成・資格情報不足・実機未実施は合格とせず未検証とする。offline契約試験の成功をlive試験の代用にしない。

新crateを `test:rust-packages` と適切なformat/clippy対象へ追加する。IPC型を変えた場合は `bun run ipc:generate`、contextsを変えた場合は `bun run s11tnext:build` を実行する。providerを使う意味試験は通常offline suiteへ混ぜず、明示設定したcanary laneに分ける。

証跡は `spec/evidence/personal-state/` 以下に、実装commit、モデル識別子、設定、scenario版、入力予算、結果、失敗理由、baseline差分を保存する。認証情報や実ユーザーの会話を試験artifactへ書き出さない。

## 15. 失敗時とリリース

構造・競合・削除試験の失敗は実装を修正して同じscenarioを再実行する。意味試験の失敗はstateを自動採用する範囲を狭めるか抽出器を修正し、回帰群を通す。通らなければP1未完了とする。

不具合時はMemoryをOFFにし、Personal Stateの追加投影を外した既存2B/27B経路へ戻す。削除無効化とRuntimeの取消・実行照合は維持する。DBのdown migrationは行わない。更新前backupは破損回復用に保持するが、旧backupへ戻す場合はその後の削除・訂正・新着会話を失わない回復手順が必要である。

isolated canaryが通ってもdefault OFFを維持する。default変更は別のrelease判断とする。P1完了後に[ロードマップ](personal-state-architecture-roadmap.md)へ結果を反映し、P2 World State v1の詳細計画を作る。

## 16. レビューの収束と残る検証

第1回は責務・競合・KV境界をレビューし、input epoch、task適用範囲、部分coverage、KV受領確認、回復manifestと切替を追加した。第2回は更新後の異常系を再点検し、job重複キー、割り込みと失敗再試行の区別、Runtime source参照、一つ分のKV容量での切替、機能OFF時の境界を修正した。最終確認では型・更新条件・作業順序・受入試験とロードマップの対応を確認した。

残る項目は実装前契約確認（P1-00）、抽出精度・遅延・容量の実測（P1-07）であり、文書上の未定義動作として残さない。外部契約が満たせない場合の未完了条件は第2節、実機未実施の扱いは第14節に従う。レビュー終了は全環境での性能・正しさの保証を意味しない。
