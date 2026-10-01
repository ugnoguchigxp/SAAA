# SAAA Gemma4の3セッションとバックグラウンドタスク実行計画

2026年10月1日。設計案。今回は計画書の改訂のみで、実装・設定変更・実サーバー接続は行わない。

256kの1セッションをユーザーとの会話用に予約し、64kの2セッションを汎用バックグラウンドタスクの実行に使う。メインの推論中も2ワーカーは独立して進行する。タスクは永続キューへ蓄積し、優先度に従って取り出す。完了後は結果と報告待ちの情報を保存し、依頼元への受渡しを再開可能にする。受渡しに失敗した場合も、結果を残して失敗理由を確認できる。

本計画の対象は、タスクの受付、実行、取消し、復旧、結果保存、完了報告である。World Modelとmemoryの用途、メンテナンス内容、RAMへの載せ方は対象に含めない。それらの将来の処理は、必要になった時点で同じ実行基盤へ登録する。

## セッションの構成

| 論理セッション | ContextWindowの目標 | 同時推論 | 役割 |
| --- | --- | --- | --- |
| conversation | 256k | 1 | ユーザーとの会話、タスクの依頼、結果の採用、最終回答 |
| worker 1 | 64k | 1 | キューから取得した有限タスクを実行 |
| worker 2 | 64k | 1 | 同じキューから別の有限タスクを実行 |

2ワーカーは同じ能力を持つ共用枠とする。タスク種別に固定せず、空いたワーカーが実行可能な仕事を取る。会話用の枠はバックグラウンドへ貸し出さない。新しい会話や読み上げが始まっても、独立した背景タスクを一律に中断しない。

3枠の所有者はアプリ内で一つの資源管理器に限定する。旧worker、接続probe、先読み、報告生成も、同じGemma4配備へ推論を送るならこの管理器を通す。未移行の旧workerはこの配備を利用する並行経路として起動しない。用途は移行しなくても、競合する実行経路の停止・接続制御は本計画の範囲に含める。ASR・TTSが使う非LLM資源は保持する。

起動時は同じ保存領域に対して単一のruntimeだけがqueueを所有することを確認し、残存allocationと遠隔requestの照合が終わるまで新しい枠を追加取得しない。複数のアプリinstanceや別クライアントまで含む物理上限は、LARM側の割当て契約で保証する。ローカルSemaphoreだけをサーバー全体の容量保証にしない。

256k、64k、最大3セッションはユーザーから提示された配備条件であり、現行サーバーでの実測値ではない。正確なトークン数、profile、allocationはLARMのcatalog・claim・healthと照合して固定する。SAAAが名称から容量を推定して上書きしない。

セッションは推論資源の割当てであり、会話履歴やKVキャッシュを保持する契約とは区別する。現行Chat Completions経路では必要なmessagesを各回送る。タスクごとにコンテキストを構成し、前の仕事の履歴を次の仕事に持ち越さない。

## 現行実装と変更範囲

| 現行の所有箇所 | 確認した挙動 | この計画の変更 |
| --- | --- | --- |
| `src-tauri/src/task_queue.rs` | SQLite正本、ownerとgeneration付きlease、重複受付防止、lane別FIFO。finishはownerとgenerationを検査するが、lease時刻をSQL条件に含めない | background lane、優先度、依存、期限、結果保存、報告待ちを追加。背景用更新に有効lease条件を設ける |
| `src-tauri/src/runtime/conversation_check/queue_runtime.rs` | 会話は単一エージェントのツールループ。4つの会話ワーカーループとspeechのループがある | 会話の取消し・受付を維持し、Gemma4推論を会話用1枠へ制限。タスク依頼と結果の受取りを接続 |
| 同ファイルのrun_laneとqueue snapshot | Notifyで起床し、DBの仕事を取得。イベント後に画面が状態を取得する | backgroundの独立した起床と実行ループを追加。結果報告もDBから再取得可能にする |
| `crates/larm-session/src/lib.rs` | Session内のProviderに容量Semaphoreがある | worker別Sessionを管理し、会話1＋背景2のローカル上限とサーバー割当てを照合 |
| `src-tauri/src/providers/larm_resources/profile.rs` | 通常会話のselectorでprofileを選ぶ | 64k枠の選択を明示的に追加。通常会話の設定から背景用profileを推測しない |
| `src-tauri/src/runtime/conversation_check.rs` のcomplete_larm_role_with_events | LARMの容量に加えてローカルのbyte上限で入力を制限する | 256kと64kの契約、tokenizerまたは保守的な計測、ローカル上限の対応を検証。広告容量だけを大きくして利用可能になったと扱わない |

背景タスク基盤は新しい独立モジュールを候補とし、queueのSQLと状態遷移は既存queue側が所有する。タスク固有の処理をconversation_checkへ追加し続けない。既存の領域別workerや保存形式を今回まとめて移行しない。基盤の試験には専用fixtureを使う。

候補モジュールは `src-tauri/src/background_tasks/` とし、contracts、handler registry、runner、resource manager、delivery dispatcher、conversation adapterに責務を分ける。queue側はreceipt、claim、heartbeat、checkpoint、terminal commit、cancel、recover、snapshotを公開する。起動・終了はAppStateと既存shutdown経路に接続し、Tauriの受付・詳細取得・取消し・結果取得・再配達APIを追加する場合は型付きIPCとbindingsも更新する。UIとハンドラーにSQLの更新権限を渡さない。

Gemma4の既存fixtureには225×1024トークンの容量があるため、今回の256k＋64k＋64kの証明には使えない。既存backchannelの分類用途も、この2ワーカーへ無断で読み替えない。

## タスクの受付契約

アプリ内のproducer、メインエージェント、将来のツールは共通の受付APIを使う。メインからは既存のツール認可境界を通じて登録する。外部の別Codexチャットへ送信する機能ではない。

| 項目 | 内容 |
| --- | --- |
| task IDと重複防止キー | 再送しても同じ仕事になる識別子。同じキーで異なる入力は拒否 |
| kindと版 | 登録された実行ハンドラーと入力形式を特定 |
| 入力 | 型付きpayloadまたは保存済み資料への参照。scopeと必要なversionを付ける |
| 依頼元 | producer、ユーザー依頼ID、会話ID、親タスクID、request revision |
| 優先度 | 対話依存、通常、低優先度のいずれか。登録済みproducerポリシーで決定 |
| 実行制限 | ContextWindow、出力上限、総時間、LLM呼出し数、ツール回数、許可された操作 |
| 依存と競合 | 先行タスクID、必要な資源クラス、必要なら直列化キー |
| 報告先と報告方式 | 呼び出し元への結果、画面状態のみ、会話での報告、静かな保存 |

識別子の意味を以下の通り固定する。

| 識別子 | 変更される条件 |
| --- | --- |
| task ID | 受理時に一度発行。再試行・checkpoint・復旧で変えない |
| request revision | 依頼元が訂正・取消しする時だけ変える。queueの再試行で変えない |
| queue generation | claim、再試行、復旧、取消しで進める実行fence。既存generationの背景用の意味 |
| attempt ID | claimされた実行区間ごとに発行。checkpoint後は別区間。heartbeatで変えない |
| provider request ID | LLM呼出しごとに発行し、attempt、allocation、遠隔receiptへ結ぶ |
| wait set ID | メインが結果を待つ一回の境界を識別。同じ依頼が後で別タスクを待つ場合は新規発行 |
| event IDとdelivery ID | 保存済み状態通知とconsumerへの受渡しを一意に識別。再配達で再発行しない |

背景の受付重複キーはscope、producer、client request keyで固定し、正規化したkind・版・入力・依頼元・実行制限・報告指定のdigestと照合する。queue generationを受付キーに含めない。候補の `task_queue_receipts` にtask IDを結び、再送は容量判定より先に照合する。再試行で既存queue行のgenerationを進めても、受付再送から別jobを作らない。ユーザーの訂正は新しいclient request keyとrequest revisionを持つ別タスクとして登録する。

既存queue行のjob_keyには背景task IDを使い、producerのclient request keyはreceipt側で保持する。既存の `UNIQUE(scope,kind,job_key,generation)` や会話入力IDと衝突させない。タスクの取消しはtask ID単位で行い、別laneの同じ文字列キーを一括取消しする既存cancel_keyを背景用APIから直接呼ばない。

client request keyは作成時刻とnonceを含む固定値とし、作成時刻は初回受付で検査する。未来5分超や90日を超えたキーは拒否し、キー内時刻を変える操作は新しいキーとして扱う。receipt削除後にもキーの年齢から期限切れを判定できるため、重複防止用の履歴を無期限に増やさない。

受付時に入力形式、scope、権限、依存の存在、循環、容量を検査する。任意のハンドラー名、自由なコード、権限を拡大するLLM指示は受け付けない。priorityやreport方式もLLMの出力をそのまま信用しない。

入力参照には許可されたsourceとversionまたはdigestを使い、dispatch前・結果採用前・配送前に再検証する。編集・削除・失権で無効になった参照は失敗または報告抑止にする。無関係な追加だけでタスクを無効化しない。UIの一覧・詳細・成果物取得も現在のscopeと権限で絞り、eventには秘密情報や本文を載せない。

SQLiteへの登録commit後にtask IDを返す。受付成功は実行成功ではなく、受理済みの仕事は再起動後も残る。キュー満杯ならcommit前に理由を返す。容量はqueued、running、outcome_unknownの合計で数え、依存待ちとretry待ちも含める。初期案は背景全体128件、P1とP2の合計112件以下、P2は96件以下とし、P0に16件、P0とP1に合計32件を確保する。scope別上限と入力の総バイト数上限も設ける。会話の既存16件上限は維持する。

すべてのタスクに絶対期限を付ける。初期案は受付から24時間以内とし、対話依存タスクは呼び出し元の締切以下にする。期限はretryやcheckpointで延長しない。受付待ち・報告待ちのメタデータにも容量を設け、受付transactionで入力・結果上限・配送行の保存余地を予約する。

| API候補 | 返す内容と保証 |
| --- | --- |
| enqueue | task ID、受付時刻、現在状態、重複防止期限。commit不明なら同じキーで照合 |
| get_taskとget_result | 認可済み状態・結果・報告状態。副作用やackを起こさない |
| list_tasksとsubscribe | scopeで絞ったページ付きsnapshotと変更cursor。イベント欠落時は再取得 |
| cancel | 最新状態と取消しreceipt。すでに完了した処理の巻戻しは保証しない |
| redeliver | 同じ保存結果の受渡しを再開。元タスクの再実行はしない |

API名は実装時の候補であり現行機能ではない。受付に成功したtask IDはすべての表示・ログ・結果取得に共通して用いる。ユーザーが失敗した報告の保持を終える操作はdiscard_report候補とし、対象配送・継続job・結果参照を確認して抑止する。cancelや履歴一覧の操作で代用しない。

## 優先度とスケジューリング

| 優先度 | 用途 | 選び方 |
| --- | --- | --- |
| P0 | 現在のユーザー依頼の継続に必要な仕事 | 次の空き枠で最優先 |
| P1 | 通常のバックグラウンド依頼 | P0の後 |
| P2 | 急がない処理 | 通常はP0とP1の後 |

同優先度では絶対期限順、同期限なら受付時刻とtask ID順に取る。依存待ち、再試行待ち、互換性のないkind、直列化キーの競合を飛ばし、後続の実行可能な仕事を進める。初期版の依存は最大8件の同じscope内の先行タスクとし、全件completedならready、failedなら後続もfailed、cancelledなら後続もcancelled、outcome_unknownなら照合または自身の期限まで待機する。依存は受付後に自由に付け替えない。

低優先度の滞留対策の初期案は、実行可能な下位jobが5分以上待ち、その間に上位仕事を8件連続で開始したら、次の1枠を最古の下位jobへ割り当てる規則とする。P1とP2の両方を対象にし、同じ待ち時間ならP1を先にする。P0の残り期限がそのハンドラーの最大実行時間以内ならP0を優先する。連続数はclaim transactionで全worker共通に更新する。処理時間を事前に断定できないため締切保証はせず、最古待ち時間と期限超過を観測する。

優先度は次のclaim時に効く。初期版は実行中タスクを高優先度の到着だけで強制中断しない。長い処理は実装されたハンドラーが段階に分け、checkpointと継続位置を保存して同じtaskをqueuedへ戻し、枠を返す。この段階では最終resultと完了通知を作らない。総step数・呼出し数・実行時間はtask全体へ累積し、checkpointで予算をリセットしない。任意タスクが自動で途中再開できるとは扱わない。

schedulerはコードで実装し、LLMでqueueを監視・選別しない。Notifyは起床に使い、DBが正本となる。通知を待つ前に取りこぼしを防ぐ登録を行い、期限や再試行時刻に合わせた起床と低頻度の再走査も行う。空のqueueからLLMを呼ばない。更新でハンドラーが失われた既存jobは、互換性エラーとして失敗と報告を保存し、取り出せないまま放置しない。

## タスクを実行する仕組み

実行ハンドラーをkindと版で登録する。ハンドラーは入力検証、コンテキスト準備、実行、出力検証、保存する結果の組立てを担当する。queueのclaimや完了状態をハンドラーから直接変更させない。

初期実装は、試験用の決定的ハンドラーと、型付き入力から有限のLLM推論を行うハンドラーに限定する。将来のツールは同じ契約に沿って追加できる。自律的な無制限ループや任意コード実行を初期機能には含めない。

LLMハンドラーの初期版は資料を読んで型付き結果を返す純粋な推論に限定する。資料取得は既存の認可済み読取り経路を使う。副作用を伴うタスクは再試行安全性、effect ID、結果照合、取消しの限界を登録したハンドラーだけが実行できる将来の拡張とし、汎用LLMへ無制限のツール実行を許可しない。

1. ワーカーは64kのローカル枠を予約し、互換性のある仕事を短いtransactionでclaimする。枠待ちの仕事をrunningにしない。空なら予約を返す。
2. job ID、owner、generation、worker ID、allocation、実行attempt IDを結び、必要な権限と入力参照を再検証する。
3. ハンドラーがタスク固有の指示と資料を組み立てる。資料・ツール結果は指示権限を持たない入力として扱う。
4. `maxTokens - outputReserveTokens - safetyMarginTokens` 以下に収め、モデルの実行直前に検査する。64kを超えた入力は明示的に拒否するか、ハンドラーが根拠を保って分割する。
5. 推論・許可されたツール処理を実行する。DB transactionを外部I/Oや推論の間保持しない。進捗は定義された段階とheartbeatを保存し、根拠のない百分率を作らない。
6. 出力形式、根拠参照、権限、期限、取消し状態、実行予算、finish reasonを検査する。生成が終わっただけでは成功にしない。
7. 必要な処理の成功が確認できた場合だけ、結果保存、仕事の完了、必要な後続タスク、完了報告のoutbox登録を同じtransactionでcommitする。null、false、更新0件は成功扱いにしない。commit後に実行枠を返し、報告処理を起こす。外部の副作用はDBと同時commitできないため、effect ledgerで成功を確認してから結果を採用する。

有限LLMハンドラーの初期予算案は、task全体の実行時間120秒、1回の生成60秒、LLM呼出し4回、資料取得6回、実行区間16回とする。総時間は準備・I/O・検証・retryの実行分を含み、queue待ち・backoffは絶対期限で制限する。遠隔dispatch前に呼出し予算を永続消費し、応答不明の試行も計上する。時間消費はheartbeatで保存し、クラッシュで実測できない区間はその区間の予約時間を消費した扱いにする。leaseの初期案は30秒ごとに更新する90秒leaseで、heartbeatは進捗とは別に監視する。期限はclaim・dispatch・commitで検査し、task全体の上限を超えてretryしない。

ネットワーク待ちやローカル処理にLLMのrequest permitを保持させない。一方、論理workerの担当タスクは初期版では完了またはcheckpointまで1件とし、その間に別タスクを取り始めない。これにより実行中タスクは背景全体で最大2件に保つ。サーバーで生成が継続中ならpermitも解放しない。

メインが依頼したタスクを待つ間は、依頼IDと継続位置を永続保存し、推論リクエストやDBロックを保持せず待つ。ワーカーからメインへ実行を依頼し返す循環と、権限外の子タスク生成を禁止する。任意の会話途中状態を保存・再開する機能が現行に存在するとは仮定せず、明示的な依頼と再開契約を実装する。

監督ループはworkerのpanic、heartbeat停止、正常終了、shutdownを捕捉する。HTTP送信前にprovider request ID、attempt、allocationの対応を永続化し、送信前と送信開始後を区別する。送信開始後のクラッシュは未送信と断定しない。プロセス終了でローカルpermitが落ちても遠隔推論の停止確認にはしない。未確認requestを持つallocationは保存済みの隔離状態で管理し、再起動後も新枠取得より先に照合する。

## 状態と結果の保存

実行状態の正本は `task_queue_jobs` とする。既存stateを活かし、背景処理に必要な `outcome_unknown` をmigrationで追加する。既存speech用のinterruptedは維持する。SQLiteの既存CHECK制約、index、参照、schema versionをtransaction内で更新し、行のコピーと整合性確認後に切り替える。起動時にschema migrationを終えてからworkerを起動する。途中失敗はrollbackし、既存の会話・speech行を保持してコピーDBで再openまで検証する。

| 状態 | 意味 |
| --- | --- |
| queued | 受理済み。実行可能時刻または依存完了を待つ |
| running | ownerと期限付きleaseを持つワーカーが処理中 |
| completed | 検証済み結果と報告待ち情報を永続保存済み |
| failed | 再試行対象外、上限超過、期限切れ、依存失敗などで終了 |
| cancelled | 取消し確定。結果は採用しない |
| outcome_unknown | 副作用または遠隔実行の結果が不明で照合が必要 |

再試行は同じ仕事をqueuedへ戻し、available_at、queue generation、attemptを更新する。claimごとに新ownerを発行する。結果採用、heartbeat、checkpointは現在のowner・queue generation・有効leaseと一致する条件付き更新で行う。taskの実行deadlineと現在の権限も確認する。更新0件なら失効した実行として扱い、result、後続job、通知をcommitしない。既存の一般finish/recoverを背景laneへそのまま流用しない。

| 遷移 | 実行者と条件 |
| --- | --- |
| queuedからrunning | scheduler。ready、権限、期限、資源、直列化キーを同じclaim transactionで検査 |
| runningからqueued | runnerのcheckpointまたは再試行。遠隔request停止確認済み、累積予算内、fence有効 |
| queuedまたはrunningからfailed | 期限、最終エラー、依存失敗。終了resultと通知を原子的に保存。runningなら必要な遠隔停止照合も登録 |
| queuedまたはrunningからcancelled | 認可済み取消し。fenceを進め、取消しresultと通知を保存。遠隔停止の確認は別台帳で継続 |
| runningからcompleted | runner。入力・出力・処理成功を確認し、resultと通知を原子的に保存 |
| runningからoutcome_unknown | supervisor。送信済みrequestの状態や副作用が未確認。fenceを進め、照合要求と状態通知を保存 |
| outcome_unknownからqueued | resolver。純粋な推論の終了確認後、取消しなし、権限・期限・累積予算が有効 |
| outcome_unknownからcompletedまたはfailed | resolver。権威あるreceiptで結果を照合し、独立したresolution lease、入力、権限、request revisionを検査。deadline後の確認は観測eventとして保存し、自動成功採用しない |
| outcome_unknownからcancelled | 認可済み取消し。採用を禁止し、必要な遠隔・副作用照合は継続 |

completed、failed、cancelledの確定結果を後から成功結果へ書き換えない。completed後の取消し要求には「すでに処理完了」を返すが、依頼元のrequest revision変更により未報告の会話出力を抑止できる。取消しは既存の副作用の撤回を意味しない。

outcome_unknownは失敗と断定できない状態である。期限後は自動再実行せず、照合待ちと理由を表示する。外部効果の照合結果は別eventで記録し、古いrun ownerに採用権限を戻さない。解決しない状態を完了履歴の掃除で消さない。

最終resultはtask IDで一意にし、採用したqueue generation、attempt ID、request revision、成功出力、要約、成果物参照、失敗コード、再試行可否、予算使用量、開始・終了時刻を持たせる。checkpointや結果不明の観測は別のstage/event行に置き、最終resultを複数作らない。副作用のreceiptも必要なら参照する。raw出力をそのまま完了報告として使わない。

kindごとに入力・出力・成果物の容量を制限する。初期案は入力1件64KiB、最終結果本文64KiB、依存8件、checkpoint32KiB、親ごとの子タスク8件以内とする。大きな成果物は認可された保存先への参照にし、DBに参照をcommitする前に成果物の保存成功を確認する。queueイベントには本文を流さない。容量不足やDB書込み失敗で結果commitができない場合は遠隔実行記録を残し、生成を再実行せず保存・照合を再開する。

初期の保持案は完了履歴30日、receiptはキーの作成時刻から90日、入力・結果・checkpoint合計64MiB、receipt・event・配送を含む管理メタデータ64MiBである。未配達・未採用結果、依存先として必要な結果、照合待ちをpinし、容量調整で黙って消さない。silentまたは画面表示のみの結果は保存commitから保持期間を開始し、ユーザーが画面を開くまで永遠にpinしない。receiptの有効期間を受付契約に返し、期限内は履歴掃除後も同じtask IDと「結果保持終了」を返す。期限後はキーの年齢で拒否し、明示的な新キーを要求する。保存領域が上限に達したら新規受付を抑制する。

heartbeatは同じattempt行を更新し、状態が変わらないtimer起床ではeventを増やさない。progressはkindが定義した有限段階だけを保存する。参照された結果と成果物は依存・inbox・継続jobの参照が外れるまで保持する。報告失敗した結果を廃棄したい場合は、認可済みの明示的な報告終了操作で継続jobと配送を抑止してからpinを外す。実行中または照合待ちの削除はこの操作で代用しない。

## 完了報告と依頼元への受渡し

タスクのcompletedと、依頼元への報告完了を別に管理する。報告失敗をタスクの再実行理由にしない。成功・最終失敗・取消しは終端状態として、結果不明は照合待ちとして記録し、指定された報告ポリシーに従って届ける。結果不明が解消した場合は別の状態通知として保存し、同じ完了通知の重複とは区別する。

result保存と同じtransactionで、必要な永続outboxへ報告を登録する。候補表は `task_queue_results`、`task_queue_events`、`task_queue_deliveries`。配送行はdelivery ID、task ID、request revision、event ID、result参照、consumer、報告方式、状態、owner、配送generation、試行回数、期限、次回時刻、配送leaseを持つ。event IDとconsumerで重複登録を防ぐ。配送payloadはterminal_resultとstate_noticeを型で区別し、照合中の通知を最終resultとして採用しない。

| 報告方式 | 受取りと表示 |
| --- | --- |
| 呼び出し元への結果 | 登録されたproducerへ型付き結果を渡す。受取りを永続ack |
| 画面状態のみ | result保存とsnapshot更新で成立。画面接続や既読を必要条件にしない |
| 会話での報告 | メインへの継続入力を登録。メインが結果を採用し、ユーザー向け文を作る |
| 静かな保存 | result保存で成立。配送行を作らず、認可されたタスク詳細から参照可能 |

依頼した時点で方式を保存する。内部処理の初期既定は静かな保存、ユーザーに頼まれた独立タスクは画面状態のみとする。ユーザーへ後で回答する会話依頼は、明示的に会話での報告を指定する。方式をタスク結果の自然言語から推測しない。

outboxは呼び出し元への結果と会話での報告に使う。dispatcherは権限と期限を検査し、pending行をowner・配送generation・期限付きleaseでdeliveringへ原子的にclaimする。consumerへは本文の自由なcallbackではなく登録済みの型付き受取り口で渡す。consumerはdelivery IDを永続inboxで照合し、受取りと必要な継続jobを同じtransactionで保存してackする。

送信側のdeliveredは有効な配送fenceとconsumerの永続ackを確認して更新する。ackを受け取る前に停止した場合は、配送leaseの期限後に同じdelivery IDを再送し、consumerは処理済みackを返す。consumer内の外部効果を伴う処理も独自のeffect ledgerが必要であり、inboxだけで副作用のexactly onceを保証しない。Notifyと画面イベントは新着の合図で、受取りの証明にはしない。

メインが推論中でも、結果の受取りと会話継続jobの永続登録は可能とする。ユーザー向けの生成は現在の会話jobが完了・取消し・明示的な待機へ移った境界で開始し、実行中messagesへ差し込まない。依存結果を待つ元のjobはclaimを持ったまま停止せず、依頼状態とcheckpointを保存して枠を返す。待機中は同じjobを通常のready jobとして再claimさせない。

wait setの結果が全件completedになれば、その待機境界を一度だけ再開する。failedまたはcancelledが来た場合もそのwait setを解決し、メインへ型付き失敗を返す。outcome_unknownの通知だけでは成功として再開しない。後で同じ依頼が別の仕事を待つ場合は、新しいwait setを保存する。複数deliveryごとに独立した再開を起動しない。

独立した背景タスクの完了報告は別の報告jobとして取り出し、新着のユーザー入力を優先する。生成が長く待つ場合は結果commit時に確定した報告期限まで待機し、期限超過を報告失敗として画面に残す。wait set自体には元依頼以下の絶対期限を付け、結果が来ない場合も一度だけ失敗で解決する。待機は配送エラーではなく、再試行回数を消費しない。初期版ではLLMによる複数報告の自動統合は行わない。

メイン側はinbox受取り、wait setの解決、必要な会話継続job登録を同じtransactionにし、結果採用と会話本文・speech jobの登録も同じtransactionで確定する。単独報告はdelivery ID、依頼再開はrequest ID、request revision、wait set IDで一意にする。背景resultを新しいuserメッセージとして保存せず、元のユーザー指示と未信頼の結果を分けて渡す。LLMの内部思考を保存する必要はなく、元の入力参照、許可されたツール結果、処理位置、残予算をcheckpointに持たせる。

会話アダプターには背景依頼・結果待ち・取消しの型付きactionを追加し、既存のanswer、web_search等と同じループで検証する。結果待ちactionでは依頼状態を永続化して今の実行区間を終了するが、ユーザー依頼自体をcompletedやfailedにしない。現行process_jobの完了処理とqueue_progressを分岐させ、待機状態から重複回答や進捗読み上げを作らない。背景結果の採用時も現行の会話source・scope・取消し検査を通す。

wait setの登録時には、同じtransactionで現在の背景resultも照合する。結果が先に完了していても待機を取りこぼさず、即座に解決または継続job登録へ進む。待機に移る現在の会話区間jobは終了し、ユーザー依頼のwaiting状態、request revision、wait set、checkpointを保存する。queue jobの区間完了をユーザー依頼の回答完了として表示しない。新しいユーザー入力から取消し対象を探す既存active_previousもwaiting依頼を認識し、関連taskと未確定継続jobを同じ依頼境界で無効化する。

メインの報告生成は現行の会話回答streamingとは分け、初期版では検証・確定後の本文だけを表示してspeechへ登録する。報告中の失敗・再試行で暫定本文や読み上げが二重に出ることを避ける。メイン生成に失敗した場合は継続jobだけを有限予算で再試行し、元の背景タスクは再実行しない。deliveredは受渡し完了であり、報告済み表示は報告messageの確定で判断する。

取消し・修正・scope変更でrequest revisionが古くなった結果は自動会話報告を抑止し、依頼を復活させない。consumerが受け取った後も、生成開始時、報告messageのcommit時、speechの再生前に再検証する。配送がdeliveredでも未確定の継続jobをsuppressedへできる。取消しそのもののreceiptと状態表示は最新の依頼状態へ返し、旧成功結果の会話報告とは区別する。

配送stateはpending、delivering、delivered、suppressed、failedとする。会話報告stateは継続jobとmessageから待機中、生成中、報告済み、抑止、報告失敗を導出する。silentと画面状態のみには配送状態を表示しない。報告失敗後の手動再配達は同じdelivery IDで受取りを照合し、すでに継続jobがあるならそれだけを再開する。期限超過した仕事は権限と新しい報告期限を明示的に検証してから再開し、元タスクや確定済みmessageを再生成しない。

ワーカーは直接ユーザーへ発言・TTS再生しない。会話での報告に音声が必要な場合は、メインが作った確定文を既存speech経路へ渡す。再起動時に過去の読み上げを自動で繰り返さない。タスクの生成完了から会話・音声までの状態をそれぞれ追跡する。

## 取消しと障害からの復旧

待機中の取消しはqueuedからcancelledへ原子的に変更する。実行中はqueue generationを進めて古い実行の採用を止め、対象jobのcancel tokenだけを通知する。ユーザー依頼の訂正・取消しではrequest revisionも更新するが、単なるworker停止やretryでは更新しない。新しい会話が来ただけでは独立タスクを取消さない。同じ取消し要求の再送には保存済み状態を返す。

deadline切れを検出するtimerはworkerの空きと独立して動く。取消し・期限切れが成功commitと競合した場合は、同じ状態行へのtransaction順で一方だけを採用する。遅いworkerや古い配送処理はownerとgenerationの条件で拒否する。schema migration以外のruntime復旧も、実行結果と通知を同じqueue APIで確定する。

| 条件 | 動作 |
| --- | --- |
| 一時的通信障害、429、5xx | kindの再実行安全性を確認し、retry-afterと上限付きbackoffで再試行 |
| 認証・容量・kindの契約不一致 | 最終失敗を保存し、設定修正が必要と報告。別providerへ無断で切替えない |
| worker片方が使用不能 | 残り1枠でready jobを処理。会話用は維持 |
| 背景2枠とも使用不能 | 受理済み仕事は保存して待機。期限到来で失敗を確定し報告 |
| 会話用が使用不能 | 独立背景タスクは継続。会話報告はoutboxまたは継続jobに残す |
| lease切れ、再起動中のrunning | 旧ownerを無効化し、遠隔実行の停止を照合。純粋な推論は停止確認後に予算内で再queue。副作用ありは照合までoutcome_unknown |
| 遠隔生成の停止が未確認 | 該当allocationを隔離し、停止確認または実行停止を保証する失効契約が成立するまで再利用しない |
| 結果commit後にdispatcher停止 | outboxから再配達。タスクを再実行しない |
| consumer停止・削除 | pendingを保持し、期限や配送上限で報告失敗にする。元の結果を失敗へ書き換えない |

純粋な推論の初期再試行案は初回に加えて2回まで、待ち時間1秒・5秒、429ではretry-afterを尊重する。正常checkpointはこのretry回数を消費しないが実行区間数と総予算を消費する。取消しや結果不明を一般的通信失敗として再実行しない。外部副作用は既存tool ledger、認可、idempotencyを通す。停止未確認の枠を再利用して3枠の上限を超えない。

配送も実行とは別の有限予算で再試行する。初期案は合計5attempt、2秒・10秒・30秒・120秒の間隔。deliveredはconsumer側の永続ackがある場合だけ。配送上限に達した場合は報告失敗を表示し、手動再配達を可能にする。詳細状態の閲覧ではackやモデル生成を行わない。

配送と会話報告の初期期限は結果commitから24時間以内とし、producerの期限があれば短い方を使う。メインの報告生成は初回と再試行1回、合計120秒以内に制限する。再試行は同じdeliveryまたはrequest revisionに累積し、受渡しやプロセス再起動でリセットしない。先の自動報告が確定済みなら手動再配達で二重messageを作らない。

正常終了では受付を閉じ、runningのcheckpointまたは停止要求、未配達のoutboxを保存する。終了待ちには上限を設け、停止未確認のattemptと隔離allocationを次回起動へ引き継ぐ。dispatcherやworkerの一時停止は受理済みの仕事を消す操作にしない。

## 実装順と受入条件

| 段階 | 実装 | 完了条件 |
| --- | --- | --- |
| P0 配備と現状確認 | 正確なContextWindow、3枠の割当て、取消し確認、基準性能を固定 | profileとallocationの選択が確定。同条件で移行前後を比較可能 |
| P1 queue契約 | receipt、kind、priority、依頼元、依存、実行・報告予算、schema migration | 世代変更後も受付再送で増えず、容量制限、二重claim防止、コピーDBの移行試験が通る |
| P2 実行基盤 | 資源管理器、旧経路の競合停止、worker別Session、handler登録、2実行ループ、heartbeat、取消し | メイン保留中に背景2件が進行。panic・再起動・停止未確認でも3枠を超えない |
| P3 結果と配送 | 検証結果、outbox、consumer inbox、ack、再配達、snapshot、保持管理 | commit直後の停止でも報告を復旧。重複通知で二重処理せず、容量を守る |
| P4 会話への接続 | 型付きaction、結果待ち、会話継続job、request revision照合、報告生成とIPC・UI | メインbusy中の完了を安全な境界で一度だけ報告。受渡し後の取消しも検出する |
| P5 配備評価 | 実サーバー3件同時生成、障害、再起動、会話・音声回帰 | 独立進行、報告、性能条件が満たされ、保存済み設定を維持 |

実装では新基盤を初期OFFとし、P1からP4の契約試験が揃ってから有効化する。同じ仕事を旧・新の実行ループへ二重dispatchしない。旧DBのコピーまたは隔離DBでmigrationし、既存queueとprovider設定を保持する。切戻しは新規受付を止め、実行中仕事と未配達結果を排出または照合してから行う。旧版が新schemaを読めると仮定しない。

## 検証

fake provider、隔離SQLite、制御できる時計、生成を保留できるbarrierで決定的試験を作る。live provider試験は別に実行する。

| 試験 | 期待結果 |
| --- | --- |
| メイン生成を保留し、背景2件を投入 | メイン終了前に2件とも開始・完了できる |
| 会話4受付ループと背景大量投入 | 推論は会話1＋背景2を超えず、入力受付と取消しは止まらない |
| 背景2件running中にP0とP2を追加 | 次の空き枠はP0。実行中仕事は勝手に取消されない |
| 依存待ちと失敗、循環、低優先度の滞留 | 後続のready jobを妨げず、循環受付を拒否。公平性と失敗伝播が規則通り |
| 同じ受付キーの再送と異なる入力 | 同じ仕事のreceiptを返す。異なる入力は拒否 |
| retry・checkpointでqueue generation変更後の受付再送 | 新規jobを作らず同じtask IDを返す。異なるproducerやscopeのキーは独立 |
| receipt保持終了・結果保持終了後の再送 | 期限内は元task IDと保持終了を返し、期限外キーは拒否。履歴掃除から仕事が復活しない |
| コンテキスト・時間・step・出力容量超過 | dispatch前または該当境界で拒否し、切断出力を成功にしない |
| retry・checkpoint・クラッシュをまたぐ予算 | 呼出し・資料取得・実行時間・区間数・期限をtask全体へ累積し、上限を超えない |
| heartbeat停止、旧ownerの遅い結果、取消し競合 | 旧結果を採用せず、他のjobを巻き込まない |
| 期限切れworkerのheartbeat・checkpoint・finish | すべて更新0件で拒否。result、後続job、eventの部分commitなし |
| 結果commit前後・配送前後のクラッシュ | 完了成果とoutboxが両方存在するか両方未確定。未配達を復旧 |
| ハンドラーの更新0件・未確認成果物・DB容量不足 | 成功扱いにせず、保存・照合を再開。モデルや副作用を無条件に再実行しない |
| 配達後ack前のクラッシュと通知重複 | consumerのdelivery ID判定で二重採用・二重発言しない |
| 配送lease切れと2dispatcherの競合 | 有効ownerと配送generationだけがackを採用し、受取り重複はinboxで排除 |
| メインbusy、待機中の依頼、修正済み依頼に完了が到着 | 安全な境界で再開。古い依頼への会話報告は抑止 |
| 複数依存・失敗結果・同じ依頼の複数wait set、受取り後の取消し | 各wait setを一度だけ解決し、途中失敗・期限切れでも永久待機しない。生成・保存・再生各境界で旧依頼を抑止 |
| wait set登録より先に背景タスクが完了、新着入力で待機依頼を取消し | 登録時のresult照合で即時解決。waiting依頼も取消し対象になり、古い継続jobを起動しない |
| メイン側の報告生成失敗 | 背景タスクは再実行せず、会話継続jobのみ再試行 |
| silent、画面未接続、生成待ち、報告失敗後の再配達 | 方式に応じた状態と保持期限。画面未接続で配送失敗にせず、再配達で二重messageなし |
| worker停止、遠隔停止未確認、副作用結果不明 | 別枠を維持し、隔離枠を再利用せず、無条件の副作用再実行をしない |
| 旧worker・別instance・起動中の残存allocation | 同じ配備へ管理器を迂回した推論を送らず、照合前に4枠目を取らない |
| 正常終了、panic、HTTP送信直前直後の停止 | 保存済みrequestを照合し、未送信を推測しない。受理済み仕事と未報告結果を維持 |
| 容量上限、結果保持、画面再接続、旧DB移行 | 受付前に満杯を返し、未配達結果を失わず、DB snapshotと画面が一致 |
| migration途中失敗・再実行、消えたhandler版 | rollbackと再openで既存行を保持。未対応kindは失敗を報告して永久待機させない |

新設する試験名は次の候補とする。既存試験として扱わず、実装時に実際の試験名と実行件数を記録する。filterで0件成功になった場合は合格としない。

```sh
# 新設する契約試験
cargo test --manifest-path src-tauri/Cargo.toml gemma4_session_pool
cargo test --manifest-path src-tauri/Cargo.toml background_task_queue
cargo test --manifest-path src-tauri/Cargo.toml background_task_runner
cargo test --manifest-path src-tauri/Cargo.toml background_task_delivery

# 既存の回帰試験
cargo test --manifest-path src-tauri/Cargo.toml --test task_queue_contract
cargo test --manifest-path src-tauri/Cargo.toml --test conversation_queue_progress_contract
cargo test --locked --manifest-path crates/larm-session/Cargo.toml
bun run ipc:check
bun run e2e:conversation-queue

# 実装完了時の全体検証
bun run check:local
```

対象試験は上表の条件を満たし、既存回帰と全体検証はエラーなしを期待する。失敗時は決定的な再現ケースを固定し、修正後に影響範囲を再実行する。既知の環境依存で実行できない検証は理由と未検証項目を残し、完了扱いにしない。

live試験では3件の生成が実際に同時進行することを確認する。3Sessionの接続成功だけでは証明しない。共有GPU競合があり得るため、メイン単独と背景2件実行時のTTFT、最終応答時間、生成速度、エラー率、queue最古待ち時間、完了から報告までの遅延を比較する。性能許容幅は結果を見る前に固定する。

音声回帰ではTTSのみの再生がASR発話を作らず、人の割込みがTTS中もASRへ届く両条件を確認する。マイクcaptureを止めたり、playback flagだけで入力を抑制したりしない。AECと音声配備そのものは変更対象に含めない。

## 実装前に固定する値

配備契約として、256kと64kの正確な容量、profileとallocationの取得方法、同時3枠の保証範囲、停止確認、認証・権限を固定する。運用値として、優先度ポリシー、受付容量、実行予算、leaseとheartbeat、再試行、配送期限、結果保持容量、会話の報告既定、性能許容幅を固定する。本書の初期案は調整値であり、サーバー仕様を確認した値とは区別する。

実装には既存SQLite・Tokio・LARMを使い、新しいqueue製品や外部サービスを導入しない。World Modelとmemory固有の用途・処理・保存形式の再設計、全providerの作り直し、権限の拡大を含めない。保存済み設定の初期化、無断のprovider置換、同じ仕事の二重dispatchを禁止する。既存の編集中の仕様書は変更しない。
