# SAAA → pi インターフェース・ツール実装計画

2026-09-13。状態: production実装と決定的試験を追加。実モデルの受入ゲートは未完了。今回の実装範囲については本書を正本とし、[旧Mission Pilot計画](pi-mission-pilot-interface-implementation-plan.md)の範囲・実装順序・暫定Codex既定案に優先する。

## 1. 目的と完了条件

ユーザーがSAAAへ実装を依頼すると、従来のローカルQwenがpi用ツールを呼び、SAAAがpiへ依頼を送る。SAAAは結果を保存してQwenへ返し、ユーザーが追加依頼すると同じpi sessionで続けられる。推論モデル・既存会話のprovider設定は変更しない。Codex SDKへの直接依頼で代用しない。

最小の一周は次の通り。

1. ユーザーが対象workspaceを指定し、SAAAへ実装を依頼する。
2. Qwenが`coding_start`を選び、SAAAが依頼を保存してpiを起動する。
3. SAAAがjob IDをQwenへ返す。会話を離れてもアプリが動いている間は作業を続ける。
4. piの終了をSAAAが観測し、今回の結果とsession参照を保存する。Qwenは`coding_inspect`で結果を取得して説明できる。
5. ユーザーの追加指示でQwenが`coding_continue`を呼ぶ。前のpiプロセスを終了した状態から同じsessionを開いて続行する。
6. ユーザーが停止を求めた場合は`coding_cancel`で停止し、停止確認と受付を区別して表示する。

**完了判定は「実際のSAAA会話 → 既存Qwen → ツール → pi実物 → 結果保存 → Qwenによる結果取得」が通ること。** CLI単体試験、SDK直接実行、IPCだけの呼出成功では完了としない。

## 2. 今回含むもの・後回しにするもの

含むものは、4ツール、共通application service、最小のjob/run台帳、pi RPC Adapter、session再開、取消・起動時照合、設定、最小の進捗表示と実導線試験。SQLiteはSAAAの既存DBとwriterを利用する。

Missionの自動計画・成果採否・自動追加依頼ループ、Personal Stateの完成、汎用worker基盤、複数job同時実行、worktree自動作成、merge/push/deploy、pi本体改修、Codex SDK Adapterは含めない。今回は明示依頼と明示追加依頼を通す。完了したrunは「実行終了」であり、目的の達成を自動認定しない。

SAAA停止中の継続実行は保証しない。NightWorkersの代替判断は別段階。削除済みの`../bbs`をこの計画作成中に再作成・再実行しない。

## 3. 現状と実装する接続箇所

| 現状のコード | 現状 | 必要な変更 |
| --- | --- | --- |
| `providers/stream/dispatch.rs` | tool公開と実行dispatch | 4ツールを共通serviceへ接続。recallと別に委任の呼出予算を管理 |
| `runtime/agent_tools.rs` | tool schema・名前の検査 | 新ツール名、引数schema、ストリーム組立時の許可名を整合 |
| `providers/agent_session/sse.rs` | UI専用の制御応答を解析し、UI toolだけを実行 | 現行Qwen接続でもcoding toolを往復できるbridgeを追加 |
| `runtime/conversation_controller/mod.rs` | reasoning MCPから最終回答を受け取る経路 | 現状は委任実行を持たない。利用中経路を確認し、無対応時に実行したと回答させない |
| `runtime/turns.rs` | coding modeはCodex read-only固定 | この分岐をpiへの入口に流用せず、通常会話のtoolから呼ぶ |
| `scripts/pi-interface/*` | PI-00の試験専用client | wire挙動の参考にする。productionから試験scriptを起動しない |

pathは`src-tauri/src/`からの相対表記。実装開始時に最新HEADと利用中providerを再確認する。前回確認したローカル設定はAgent Session経路だったため、OpenAI互換tool一覧だけを更新して完了としない。

### Qwenとのツール往復

native tool-callを扱える経路では既存`available_agent_tools` / `execute_agent_tool`へ統合する。Agent Session経路は現行契約を調査し、native toolイベントが利用可能ならそれを優先する。利用できなければ、既存UI bridgeと衝突しないcoding専用の型付き制御応答を追加し、同じapplication serviceへdispatchする。

制御応答は通常の文章からkeyword抽出せず、ホスト発行のturn marker、許可されたtool名、完全なJSON schemaの一致を必須にする。引用文やtool結果内のJSONを再帰実行しない。制御応答をユーザー向け本文として先行stream表示しない。tool結果はデータとしてQwenへ返し、その中の命令に新たな権限を与えない。

副作用を伴う受付が確定した時点でprovider fallbackによる再実行を禁止する。会話の再試行、同一tool callの再配送も台帳で重複排除する。音声reasoning等の別経路がtoolを扱えない場合は能力未対応を明示する。動作確認のためにローカルQwenをCodexや別providerへ切り替えない。

## 4. ツール契約

| ツール | Qwenが渡す値 | 戻り値と意味 |
| --- | --- | --- |
| `coding_start` | workspace ID、依頼本文 | job ID、run ID、revision、`queued`。作業完了ではない |
| `coding_inspect` | job ID、event cursor、limit | 現在状態、今回の結果要約、変更・tool実行の観測、session参照、次cursor |
| `coding_continue` | job ID、expected revision、追加依頼本文 | 同一jobの新run IDとrevision。保存済みsessionを再開 |
| `coding_cancel` | job ID、expected revision、理由 | `cancel_requested`または既存の終端状態。停止確認は後続状態 |

conversation ID、ユーザー依頼source、呼出元run ID、冪等キー、実行path、session path、provider/model、権限はホスト側で付与・解決する。Qwenへ任意argv、任意RPC、認証値、session pathを生成させない。

workspaceはユーザーが選択した実在するローカルGit directoryをcanonicalizeして登録する。モデルが作ったpathをそのまま信頼しない。対象未選択なら`workspace_required`を返す。同じ会話の再接続から参照できるが、無関係な会話から別jobへアクセスできないよう関連を検証する。

依頼本文は最大32,000文字。tool戻り値は最大32KiB、イベントは最大100件に制限し、切詰めの有無と続きを取得するcursorを返す。実行中のcontinueは`busy`。一度に実行するpiは1つ。待ち行列の自動スケジューラは作らない。

冪等キーは元のユーザー入力IDとtool call ID等からホストが発行し、同一キーで異なるpayloadは拒否する。異なるtool call IDでも同じ入力から二重startが起きないよう、初期版は入力ごとにstart一件を制約とする。revision不一致は`stale_revision`として返し、勝手に再送しない。

## 5. application serviceと最小台帳

新設候補は`src-tauri/src/coding/`にcontracts、service、repository、tools、recovery、commandsを配置する。UI IPCとQwen toolは同じserviceを使う。状態の正本をfrontendに作らない。

| テーブル案 | 保存するもの |
| --- | --- |
| `coding_jobs` | conversation/source参照、workspace、provider/model/profile、revision、session binding、現在状態 |
| `coding_runs` | 不変の実行payload、digest、冪等キー、配送状態、開始entry境界、結果参照、開始終了時刻、停止理由、process ownership |
| `coding_events` | 単調sequence、job/run ID、受付・起動・終了・停止・異常の記録。delta全文は保存しない |

最新schemaに追加migrationを作り、既存runtime_runsやconversation本文へ実行状態を二重保存しない。会話のsource参照と配送前payloadは役割を区別する。transaction中にpiやQwenを待たない。

配送は`prepared / sending / accepted / rejected / unknown`、runは`starting / running / stopping / settled / failed / interrupted / outcome_unknown`を区別する。再起動時に`sending`以降の不明な依頼を再送しない。piのRPC idは応答相関であり冪等性を保証しない。

pi session JSONLはSAAA app data配下へ保存し、全文をDBへ複製しない。対応するユーザーsourceが削除・失効した場合は継続を止め、削除済み内容を含むsessionへ黙って追加依頼しない。

## 6. pi Adapter

`src-tauri/src/runtime/pi/`へprocess、protocol、session_reader、probeを追加する。pi 0.85.1を初期の検証対象とし、実行pathとversionを起動前に確認する。認証はpi側の既存仕組みを使い、Codex SDKの認証tokenをコピーしない。

1. 明示委任とworkspaceを検証し、依頼をDBへ保存して実行枠を確保する。
2. shellを介さず`pi --mode rpc --session <host-owned-path>`をcwd固定で起動する。暗黙extension等を無効にした検証済みprofileを使う。
3. `get_state`でready、session ID/path、provider/modelを照合し、開始entry境界を取得する。
4. `prompt`をstdinへ送る。受付成功で`accepted`とする。
5. `agent_settled`まで観測する。`agent_end`や最後の自然文だけで成功判定しない。
6. 今回のentryとmodel/toolエラーを回収する。tool失敗も結果に残し、runの終了と実装成功を混同しない。
7. stdin EOFで終了し、stdout/stderr排出と子process終了を確認して永続状態を確定する。

追加依頼は前のprocessが終了済みであることを確認し、同じsessionを新processで開く。session消失、header/cwd不一致、壊れたJSONは起動前に拒否する。`get_entries(since)`は応答上限を超え得るため、停止後のstreaming session readerで照合し、完全性を確認できない結果は不完全とする。

JSONLはLFで区切り、分割UTF-8とUnicode改行を正しく扱う。初期上限は1行8MiB、run全出力64MiB、起動/受付30秒、実行30分。上限超過は制御された停止と診断にする。

取消は新送信凍結 → clear_queue → abort → 結果回収 → EOF/終了確認の順。無応答時は期限付きでprocess groupを停止してreapする。アプリ終了時も同じ停止経路を使う。ウィンドウ非表示や購読解除は取消にしない。アプリ異常終了後はPIDと起動identityを照合し、旧processが残っている間は二重起動しない。所有が確認できないPIDをkillしない。

piのcwd指定はsandboxではない。初期版はユーザーが明示委任した信頼済みローカル作業を扱い、workspace外書込の強制隔離を保証しない。

## 7. 設定と最小UI

coding専用設定に、enabled、pi実行path、検証version、provider/model、resource profileを置く。会話用Qwen設定とは独立させる。初期enabledはOFF。設定画面でpiの起動・認証・model利用可否を表示し、認証未設定なら実行を開始せず`authentication_required`を返す。

pi側model候補はユーザー指定の`gpt-5.6-luna`を維持するが、pi providerでの利用可否は未実証。認証とmodel確認を通るまで「利用可能」と表示しない。未対応なら具体的な不足を報告し、SDKへの自動fallbackは行わない。

会話にjobカードを表示する。項目はworkspace、状態、最新結果、停止ボタン。追加依頼は会話の明示指示から行う。結果到着は永続eventからカードへ反映し、SAAAはユーザーの「結果を教えて」で同じjobをinspectできる。自律的な評価・再実行のwake-upループは今回作らない。

## 8. 実装順序と受入ゲート

| 段階 | 作業 | 完了条件 |
| --- | --- | --- |
| IF-01 | 現行Qwen経路の確認、4ツールschemaとservice境界、fixtureによる往復 | 実際に利用するprovider経路からhost serviceへ到達。架空の成功値を返さない |
| IF-02 | 台帳・revision・冪等・単一実行・取消契約 | 二重start、別会話、stale revision、再起動をモデルなしで検証 |
| IF-03 | Rust pi Adapterとsession reader | pi実物で送信、結果、終了、再起動後continue、error/abortを確認 |
| IF-04 | Qwen tool dispatch、Agent Session bridge、設定・IPC・jobカード | 会話→service→piを接続。UI切断後もDBから結果復元 |
| IF-05 | ローカルQwen＋pi実モデルのSAAA E2E | 下記シナリオがproduction経路で全て通る |

IF-01のfixtureで通っただけでは実装完了としない。IF-03のCLI合格でIF-04/05を省略しない。Agent Session側に必要な契約が足りなければ、その不足と最小変更を先に確定する。推論基盤の置換で迂回しない。

### 決定的試験

tool引数不正・workspace未選択・無権限job、同一入力の重複配送、取消との競合、accepted後のmodel error、agent_end後retry、部分JSON・巨大出力、session消失/破損、送信直後切断、旧process生存、UI再購読を検証する。副作用受付後のprovider fallbackが再startしないことも必須。

### 実導線での試験

専用の一時Git workspaceをユーザー操作でSAAAに選択し、通常会話から小さなPythonファイルの作成を依頼する。Qwen tool call、SAAA job/run ID、pi PID/session/entry ID、実ファイルを記録する。Qwenがinspect結果を説明でき、SAAA再起動後に追加指示から同じsessionで変更できることを確認する。別シナリオで実行途中の取消、piの認証なし、実装失敗を確認する。

試験のためにホスト側scriptからpromptをpiへ直接送らない。SAAA側で固定成功回答に差し替えない。実モデルレーンは明示実行とし、通常CIには入れない。ただしこの機能の完成判定には必須とする。

段階ごとに該当Rust/TypeScript試験を実行する。IPC変更時は`bun run ipc:generate`と`bun run ipc:check`、fixture変更時は既存生成commandを使う。統合時は`bun run check:local`、`bun run test:rust-packages`、`bunx --no-install spec-html check spec/docs --warnings-as-errors`を実行する。既存PI-00の`bun run pi:canary`は補助試験として残す。

## 9. 今回の計画化の結論

最優先は新しいMission Pilot全体ではなく、既存Qwenの会話から4ツールを通じてpiを操作できる導線である。DBと型付きIPCへの必要な最小追加は実装範囲に含める。既存の会話推論、pi本体、認証方式の改造は含めない。今回の成果物は計画のみであり、BBS依頼の再送やproduction実装は行っていない。


## 10. 実装作業記録（2026-09-13）

第9節は計画作成時点の結論。今回、`src-tauri/src/coding/`と`src-tauri/src/runtime/pi/`を追加し、通常会話のdispatch、Agent Sessionのcoding専用bridge、設定画面、会話のworkspace選択・job表示を接続した。会話用provider設定は変更していない。初期enabledはOFFのまま。

DB schemaは16。既存writer上でjob/run/event、host sourceに結び付く冪等結果、workspace登録、coding設定を保存する。同じsourceからの二重startは既存jobへ集約し、異なるpayloadは拒否する。continueには新しいユーザー入力と一致するrevisionが必要。sourceを削除したsessionへの追加依頼は拒否する。再起動時の不明な配送は再送せず、生存PIDまたは起動直前の所有不明状態があれば実行枠を保持する。所有不明のprocessへsignalは送らない。

pi Adapterはshellを介さず起動し、host側session、provider/model、versionを照合する。LFで区切ったJSONLを上限付きで読み、`agent_settled`とprocess終了後に今回のentry境界以降を回収する。モデルエラーとtoolエラーを結果へ残し、モデルエラーはfailedにする。取消受付と停止確認を分け、終了時にも停止要求を送り、期限内に終了しなければ所有process groupを停止してreapする。

| ゲート | 確認した内容 | 残る確認 |
| --- | --- | --- |
| IF-01 | coding専用制御応答から共通serviceへ到達し、実際のworkspace_requiredを次turnへ返すfixture。分割受信・不正schema・別marker・引用文を検査 | 実際のQwenによるtool選択 |
| IF-02 | 単一実行枠、source・会話の関連、revision、重複payload、event cursor、再起動後の不明配送を検査 | 実アプリ異常終了を伴う確認 |
| IF-03 | production serviceとRust Adapterで試験用RPC相手への開始・再開・取消・model errorを検査。実物pi 0.85.1の起動probeも実施 | 実物piと実モデルによる送信・結果保存・再開 |
| IF-04 | native dispatchとAgent Session bridge、coding設定IPC、workspace選択、DBから再取得するjob表示を実装。受付後のprovider failureでfallbackが許可されないことを検査 | 実アプリでのUI再購読・再起動確認 |
| IF-05 | 未完了 | 既存Qwenから実物pi、実ファイル、結果取得までの全シナリオ |

実物piのprobeは`authentication_required`を返した。`--list-models gpt-5.6-luna`も`No models available`を返している。認証情報はコピーせず、provider/modelの自動切替も行っていない。このため、本計画の完成条件を満たしたとは扱わない。

検証commandは`bun run ipc:generate`、`bun run ipc:check`、`bun run check:local`、`bun run test:rust-packages`、`bunx --no-install spec-html check spec/docs --warnings-as-errors`。補助の`bun run pi:canary`も通過したが、その記録は`liveModel: not-run`であり、IF-05の代用にはしない。新設moduleのsize baselineを登録し、`runtime/mod.rs`はmodule登録行の増加分を反映した。

全体チェック中、既存の`crashed_owner_releases_lock_without_removing_the_lock_file`が1回`AlreadyOwned`で失敗した。原因は未特定。該当試験の単独再実行と、その後の`bun run check:local`全体は通過した。

利用開始時は設定画面の「piによる実装」で絶対実行pathとpi側provider/modelを保存して接続確認し、会話上の実装先に一時Git workspaceを選択する。pi側の認証とmodel可用性が整った後、通常会話から第8節の実導線試験を実施する。

## 11. Codex SDK接続への追加変更（2026-09-13）

ユーザーから「pi側もCodex SDKで設定してほしい」と追加依頼があったため、第9節のSDKを含めない前提を変更した。会話用Qwen設定は維持し、実装専用の`codex-sdk-v1`を追加した。pi 0.85.1に`--extension`で`/Users/y.noguchi/Code/SAAA/scripts/pi-codex-sdk/index.ts`を明示し、providerは`saaa-codex-sdk`、modelは`gpt-5.6-luna`とする。SDKは既存依存の`@openai/codex-sdk` 0.144.4と既存のCodexログインを使う。piの認証ファイルへの資格情報コピーは行わない。

実行経路はSAAA共通coding service → pi RPC → 専用provider拡張 → Codex SDK。piの外側のtoolは無効化し、SDKがworkspace-write・approval never・ネットワーク無効・web search無効でファイルとコマンドを操作する。SDKが読み込む既存MCPサーバーは、このprocessだけenabled=falseで上書きする。ホストのMCP設定ファイルは変更しない。piの自動retryと自動compactionも無効にし、意図しない依頼の再実行を避ける。

pi sessionのcustom entryにSDK thread ID、cwd、modelを保存し、再開時に一致を検査してresumeThreadへ渡す。SDKのファイル変更とコマンド結果は件数・文字数を制限してpi sessionへ保存し、coding_inspectのtool観測へ取り込む。SDK取消にはAbortSignalを渡し、期限超過時は従来の所有process group停止を使う。

設定画面には接続方式と拡張pathを追加した。pi実行ファイルと同じディレクトリーを子processのPATHへ追加し、Finderからの起動でも隣接するNode/Codexを解決する。拡張はこのcheckoutのnode_modulesを参照するため、checkout移動時は設定pathの更新と依存関係のインストールが必要。

検証には`coding_service_codex_sdk_live`という明示実行のignored testを追加した。一時Git workspaceと本番共通service/Rust Adapterを通して、新規ファイル作成、別ユーザー入力による再編集、pi session IDの継続、SDK thread IDの継続、観測entryの永続化を検査する。SDKを直接呼ぶ診断probeは、このservice経由検証の代用にはしない。実アプリUIとQwenによるIF-01/04/05、および実モデル実行中の取消は引き続き別の受入確認が必要。

最終profileによるlive testは通過した（開始、実ファイル作成、同一session/thread再開、再編集、観測保存）。ローカルDBのcoding_settingsはenabled=trueで保存済み。保存前のcoding設定は同じApplication Supportディレクトリーへ退避し、schema更新は既存SqliteWriterのバックアップ・migrationを使った。保存前後でsettings_documents全体のハッシュが一致し、会話用設定が変わっていないことを確認した。

SDK拡張のunit test、フロントエンド全348件、型検査、フロントエンドbuild、lint、spec検査は通過した。全体のformat/size checkは同時進行のpersonal-state関連変更で未通過。coding関連の新設moduleだけを既存baseline値を維持して登録した。全体の完成判定と実アプリUI受入は、この追加接続の実機確認とは区別する。

Rust全体は553件、IPC契約3件、SQLite構造1件、音声IPC契約1件が通過した。live試験とローカル設定書込みはignored testを明示実行した結果として別に記録する。

## 12. Qwen 3.8によるBBS実導線の検証（2026-09-13）

ユーザーの追加指示により、通常会話の接続先をlocal LLMのQwen 3.8へ変更した。開始時のrouting.tasksは2026-09-06 13:08 JST更新でMuseをprimaryに指定していた。変更した操作主体まではDBから特定できない。Museへの最初の実依頼はsubscription quota exhausted（HTTP 429）で終了し、coding_startは未発行だった。

現在の設定はHarness経由のlan-llm-dynamic、会話timeout 240秒、reasoningEffort low。接続窓口の公開名はcoding-defaultで、gnosis上の実モデルと応答model IDはQwen3.8-27B-ROCmFP4-FAST.ggufと確認した。実装担当のpi/Codex SDK設定は変更していない。

実導線でHTTP conversationの不足を修正した。coding用workspace/job参照を先頭のsystemメッセージへ統合し、Qwenが拒否する途中のsystem追加を避ける。公開aliasと具体model IDの文字列一致は要求せず、実際に返されたmodel IDが同じcompletion内で変わる場合を拒否する。架空のmodel alias対応表は追加しない。

現在のLAN gatewayはQwenのSSEをgateway_stream_protocol_error / invalid_chunkで途中終了させる。そのためagent-connectionによるHTTP実行はJSON completionを使い、同じtool定義・validation・dispatch・永続化を維持する。JsonProbeは従来どおりtoolを送らず、新設JsonToolsだけが通常tool loopを実行する。このLAN経路は応答完成後にテキストを表示する。reasoningEffortはlowを送るが、モデル固有のthinking templateを強制無効化しない。実モデルはenable_thinking=falseで単純な質問にも無関係な回答を返し、通常templateでは正答することを確認した。接続やmodelを別providerへ自動fallbackする変更ではない。

`coding/e2e/tests.rs`は明示実行のignored testで、実アプリDBのwriter所有権を取得し、通常会話runtimeのexecute_turnを起点にする。workspace登録後に実会話モデルがcoding_startを発行したことをhost_run_idで照合し、piの終了結果を待つ。SAAAのUI操作を起点とする試験ではない。実装内容をテスト側から書くことも、coding_startを注入することも行わない。

2026-09-13の実モデルE2Eは未成功。通常DBのrun `bbs_e2e_1789276405418871000_6` と、同じ接続・coding設定だけをコピーした空DBのrun `bbs_e2e_1789276689693364000_6` で、Qwenが2,048 token上限に達しcoding_startを返さなかった。空DBでも検索tool表記の反復が推論欄に現れ、正式なtool_callsは空だった。gateway audit requestは `req_5011c5db-5f00-423e-9fa6-9ab92437b80d`。文字列からtoolを抽出・実行する回避はしていない。../bbsにはGit初期化だけがあり、アプリ生成・ブラウザ検証は未完了。

HTTP側には実装要求をpiへ委譲するsystem instructionを加え、parallel_tool_calls=falseを送る。HTTPテスト12件、codingテスト7件、module-size、Rust format、spec checkが通過。全体Rustテストはconversation_route_falls_back_and_persists_completed_messageとlarm_turn_commits_before_events_and_keeps_success_when_release_failsで停止し、今回の実行を終了したため全体合格とはしない。

E2E fixtureはSAAA_BBS_SETTINGS_SOURCEを指定すると空DBに設定のみをコピーできる。通常DBの会話履歴は変更せずに同じruntime::turns::execute_turn経路を検証するための機能で、既に会話があるDBへの設定コピーは拒否する。
