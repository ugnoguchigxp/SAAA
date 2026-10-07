# HTTP実行環境・独立テスト・接続制御 実装引渡書

更新日: 2026年10月7日。担当: Claude Sonnet向け。状態: **実装引渡書・受入未実施**（既存media lab等は再利用する）。

## 0. この文書の使い方と完了の定義

この文書と同梱の[ソース一覧](http-runtime-inventory/sources.tsv)、[API一覧](http-runtime-inventory/commands.tsv)、[transport参照一覧](http-runtime-inventory/transport.tsv)、[テスト候補一覧](http-runtime-inventory/tests.tsv)を実装指示とする。会話履歴や他の計画の読解を前提にしない。コードへのリンクは変更箇所であり、実装時には現物を読む。

**目標は、Tauriをビルドせず、既存画面でフルセット・機能単独・診断単独を起動し、接続をON/OFFでき、全業務単体テストを実行できること。** 画像だけ、画面だけ、mockだけ、移管できたテストだけで完了としない。H00からH13まで実施する。段階ごとの完了は全体完了ではない。

本書がHTTP化・起動用途・全単体テスト移管についての正本となる。旧計画の「HTTPはmedia限定」「共通runtime抽出は後続候補」「計測次第で全体化する」という制約は適用しない。一方、他のタスクが進める記憶・Context等の仕様変更を本作業へ取り込まない。並行変更の最新の意味論をそのまま移管する。

### 作業上の固定条件

- `/Users/y.noguchi/Code/SAAA`の既存checkout・現在のbranchで作業する。clone/worktree/巨大コピー、追加依存のインストールを行わない。既存の依存バージョン、共有target、独立manifestの構成を利用し、workspace化は本件に含めない。
- [AGENTS.md](../../AGENTS.md)を守る。利用できる場合だけ`initial_instructions`を会話で一度呼ぶ。未提供ツールは未実行と記録し、作業を架空の実行で埋めない。
- dirty/untrackedの作業を削除・巻戻ししない。共有ファイルは書く直前にも差分を読み、他の変更を維持する。別エージェントへ依頼・送信しない。commit/pushは明示依頼がある場合のみ。
- 全検証・buildは`verify`または既存の承認済みserial入口経由。別targetや直接Cargoでロックを回避しない。テスト削除、ignore追加、baseline緩和で通さない。
- 保存設定、Provider、資格情報を初期化しない。migrationは隔離DBまたは小さい試験用コピーで確認する。本番DBを計測用にコピーしない。
- 実装上の命名・関数分割は判断してよい。下記の完成条件、API互換、停止の意味、データ保全、native同等性、検証範囲を縮小してはいけない。
- 未設定の実Provider・実機権限など環境依存の受入は「未検証」にする。独立して実施できる工程は続け、未検証が残る間は全体完了と報告しない。

### 成果物と完了条件

| ID | 成果 | 全体完了に必要な証拠 |
| --- | --- | --- |
| A1 | フルセットHTTP起動 | 既存10画面が実データで動き、会話・記憶・ツール・作業・音声・media等の対応表に未実装がない |
| A2 | 機能単独起動 | ASR/TTS/LLM/Laya/Laya発話表現/Embedding/画像/楽曲/アバターを選択でき、他の常駐workerが起動しない |
| A3 | 接続ON/OFF | 公開ツール、文脈、dispatch、背景処理、retryのすべてに反映。実行中切替と再接続を含む |
| A4 | 診断だけ起動 | 既存診断画面・checksを利用し、全AppStateや会話workerなしでquick/full/機能別を実行 |
| A5 | 全単体テスト | 移管台帳の全業務ケースをTauri非依存で実行。件数・除外理由・未移管数を報告。未移管は0 |
| A6 | 自動選択 | 日常操作でpackage列挙不要。変更・依存先・利用側を検出し、不明なら広く検証 |
| A7 | Tauri回帰なし | 同じ共通runtimeをTauriも呼ぶ。全体advance/fullとnative実機の受入記録がある |

## 1. 現コードの根拠と全件追跡

調査HEADは`cced8b8177b615b50167841fe760a823c0ecbf8b`、branchは`feat/self-diagnosis-v2`。dirty treeを読み取り、1716ソース、9 manifest、170 command宣言、682 transport参照行、2309 Rust test属性、672 TypeScript test式、48 macro等の要確認行を保存した。[summary.json](http-runtime-inventory/summary.json)に取得時刻と制約、sources.tsvに各ファイルhashがある。**実際の登録API数・実行テスト数ではない。** 重複、cfg、macro展開、parameterized test、動的イベント名は別途機械照合する。取得中のhash変化は0件だった。

実装開始時の差分取得は次で行う。生成器はソースを読むだけでbuildしない。開始時の添付一覧は消さず、変化分を実装台帳に追加する。再採取する場合は開始時一覧を別名で残す（小さいTSVだけ）。

```sh
git status --short
python3 docs/plans/http-runtime-inventory/collect.py
```

[collect.py](http-runtime-inventory/collect.py)は静的候補の収集器であり、合格判定器ではない。最終判定器はH00/H12でverifyへ組み込む。過去snapshotに現れない新ファイルを対象外にしない。

主な結合点:

| 現物 | 現在の責務と移管時の注意 |
| --- | --- |
| [lib.rs](../../src-tauri/src/lib.rs) | setupでDB、資格情報、Situation、記憶worker、生成能力、MCP、mediaを構築。その後、会話queue、coding、到達性監視、自律改善、scheduleを起動。終了処理もここにある |
| [AppState](../../src-tauri/src/app_state.rs) | 全状態、取消、各サービスを一括所有。`AppHandle.state()`を共通サービスへ持ち込まない |
| [command_registry](../../src-tauri/src/runtime/command_registry.rs) | 中央generate_handlerに加え、registry/ASR/辞書/media/回顧のwith_handlerを連結。中央だけを数えない |
| [App.tsx](../../src/App.tsx)、[appRoute](../../src/shell/appRoute.ts) | snapshot必須、10画面。`conversation,memory,work,records,audit,diagnosis,workers,unitTest,ttsDictionary,settings` |
| [診断engine](../../src-tauri/src/diagnosis/engine.rs)、[registry](../../src-tauri/src/diagnosis/checks/mod.rs) | AppStateとAppHandle依存。集計・期限・証拠は再利用し、入力依存と通知のみ切り離す |
| [agent_dispatch](../../src-tauri/src/providers/stream/agent_dispatch.rs) | 記憶・ContextStill・coding・UI・tool selectionの公開と実行。ON/OFFの二重チェック対象 |
| [audio backend](../../src-tauri/src/voice/audio_backend/mod.rs) | VPIO所有、echo、録音と再生。TauriなしでもOS処理は必要 |
| [Web取得](../../src-tauri/src/runtime/web_fetch/content.rs)、[preview](../../src-tauri/src/artifact_preview/mod.rs) | Tauri WebViewへの依存。ブラウザから外部ページをiframe表示するだけでは代替できない |
| [build.rs](../../src-tauri/build.rs) | VPIO C compile、Codex資源、role-routing sidecar、WebFetch資源、tauri-build。HTTP/単体テストから切断する |
| [verify-plan](../../scripts/verify-plan.ts)、[affected](../../scripts/verify-affected-plan.ts) | package発見と既存gate、影響選択あり。shadowや限定allowlistの成功を全領域の自動選択完成としない |

現状のmedia/provider独立crate、HTTP host、launcher、影響選択は再利用する。過去レビューの指摘が修正済みかは現在のコードとテストで判定し、古い不具合一覧をそのまま再実装しない。

## 2. 採用構成と移管先

依存方向は「host → application組立て → domain → contracts/storage/既存独立crate」。domainからapplication・Tauri・別domainの具体serviceへ戻る依存は禁止。domain間の循環は、呼ぶ側に小さいport traitを置き、applicationが実装・注入して切る。全AppStateをtraitに名前変更しただけの巨大portを作らない。

新規crateは必要になる工程で作る。下記名称を採用する。既に同責務のcrateが追加されていた場合だけ再利用し、実装台帳へ名前の対応を残す。旧全runtimeを丸ごと一つのcrateに移して終了しない。

| 移管先 | 現ソースの所有範囲 | 境界・例外 |
| --- | --- | --- |
| `crates/saaa-contracts` | `models.rs`, `ipc_contract`, `voice_asr_contract.rs`のwire型、共通ID/取消/event envelope | serializationと値のみ。tauri IPC receiverテストはhostへ。ドメイン固有型を無理に全収容しない |
| `crates/saaa-storage` | `persistence`, `credentials`, `backup`, `database_backup`, `records`のSQL/保存 | writer/readersとmigrationを一元化。commandsは薄いhostまたはapplication。DBの別実装を作らない |
| `crates/saaa-providers` | `providers`の通信・session・probe・資格情報port、`runtime/provider_unit_test.rs` | 既存`saaa-provider-routing`,`larm-session`を利用。回答採用・tool loopはconversationへ |
| `crates/saaa-memory` | `memory`全体（回顧・忘却・exportを含む） | 既存personal-state-coreを利用。記憶の意味論を改変しない。ContextStillを新規実装しない |
| `crates/saaa-tools` | `tool_selection`, `generated_capabilities`, `generative_ui`, `artifact_preview`の非OS部分、`runtime/web_fetch`の非Tauri部分 | 各moduleの責任は維持。実行・文脈等はportで接続。WebView実装はnative/hostへ |
| `crates/saaa-work` | `coding`, `worker_agents`, `steward`, `schedule`, `adaptive_evaluation/improvement` | 初回は同crate内で既存module分離を維持。会話へはQueuePort、memoryへはreader port。独立テストが重い場合の追加分割は後続改善 |
| `crates/saaa-conversation` | `runtime`の残り、`role_routing`, `task_queue` | core loop/context/queue/回答採用を所有。`command_registry`は除外。media/voice/work/memory/toolsはport経由 |
| `crates/saaa-voice` | `voice`, `voice_behavior`, `voice_text`, `tts_dictionary`の非OS部分 | PCM処理とprofileとTTS/ASR制御。VPIOのC/OS呼出しはnativeへ、純粋echo処理はここへ |
| `crates/saaa-situation` | `situation` | monitorは取消付きtask。OS観測はport。windowの直接参照は禁止 |
| `crates/saaa-diagnosis` | `diagnosis` | checks/aggregate/store/engineを移す。DiagnosisContextで必要portのみ受け取る |
| 既存`crates/saaa-media` | `media_generation`の残る非host処理 | 画像・楽曲の同じ台帳、資格情報、進捗、取消、復旧を両hostから利用 |
| `crates/saaa-application` | `app_state`, `app_paths`, `lib.rs` setup/shutdown、`lib/window_shutdown_grace`の業務調停 | bootstrap、registry、cross-domain transaction、snapshot/usecaseを所有。薄い組立て層に限る |
| `crates/saaa-native` | VPIOのOS部分、OS権限/外部URL/資源、native WebView helper client | Tauri依存なし。単体suiteの依存閉包には入れない。HTTP/Tauriで使うport実装 |
| `services/saaa-http` | 新規full host | 既存`services/feature-lab`から認証/配送を共通化して再利用。media専用DB/設定の初期化は流用しない |
| `services/saaa-native-host` | macOSのWebView等が必要なhelper | Tauriなし。音声・WebViewのOS受入はここを含めて行う |
| `src-tauri` | command adapter、Channel/event adapter、Window lifecycle、Tauri resource解決 | 各commandは共通usecaseへ委譲。テストもIPC/window結合だけ残す |

補助module `util/redact/process_guard/diagnostics`は使用先の最小責務へ移す。純粋共通値はcontracts、DB診断はstorage、プロセス所有はnative/host。`voice_commands`はvoice usecaseとhost adapterへ分ける。`test_state/test_support/tests/ipc_receiver_tests/wasm_host_poc/quality_eval/*_e2e/bin/examples`はテスト移管台帳で全件対応付ける。`window_size`はTauri hostに残す。`.s11tnext`等の`include_str!`は新しい相対位置へ直し、内容を複製しない。

storageがdomainへ逆依存しないよう、domain型を使うrepository関数はそのdomainへ移す。storageは接続・transaction・既存schema/migration順序・共通保存型までを所有する。domain横断SQLはapplicationが借用接続を各repositoryへ渡す。同じSQLをhost別にコピーしない。providers内の回答保存と`runtime/provider_unit_test.rs`内のmedia/voice調停も、通信部分だけproviders、調停をapplicationに分ける。表のディレクトリ単位は所有範囲の既定であり、循環依存を容認する指定ではない。

applicationの直接依存は共通domainとcontracts/storageに限る。nativeの具体実装は両hostが構築してportとして渡す。`ApplicationRuntime::build(profile, resources, ports)`は同期構築とasync startを分け、domain constructorだけでtaskをspawnしない。`stop()`はidempotentとし、start失敗後にも呼べる。これにより診断・unitの依存閉包へnativeを入れずに済む。

### portの最低契約

| port/状態 | 必須契約 |
| --- | --- |
| `Database` | 既存writer/readersを1 instanceで注入。複数domainにまたがるtransactionはapplicationが同一接続で調停。transaction中のnetwork/process await禁止 |
| `CredentialStore` | backend内だけで秘密を解決。既存LARM設定解決・競合検出を維持。OnceLockのアプリ全体固定をinstance所有へ移す |
| `EventSink` | 型付きpayload、sessionId/runId、sequence、終端。同期emitの失敗で業務処理を再実行しない |
| `TaskSupervisor` | 名前、owner、cancel、JoinHandle、開始/停止状態。detachだけのspawn禁止。profile変更・shutdownで全taskを回収 |
| `RuntimeResources` | データ/資源の明示パスと既存sidecarの解決。Tauri BaseDirectoryはhostで処理。build時にProviderへ接続しない |
| `VoiceIo` | 同一VPIOでcaptureとplayback、実render PCM、cancel、status。音声再生フラグで入力を遮断しない |
| `ContentFetcher / PreviewHost` | 既存のURL安全性、期限、取消、navigation policy、成果物tokenを維持。Tauri型を返さない |
| `Clock / Provider / Tool / Memory / Queue` | 各domainが実際に使う操作だけ定義。fakeは単体試験専用、実運用profileへ混入禁止 |

## 3. 起動コマンド・profile・画面の仕様

以下は**これから追加する正式な入口**。現状で動くと報告しない。

```sh
bun run start:http
bun run start:http -- --profile full --data-profile user
bun run start:http -- --profile feature --feature image --data-profile isolated
bun run start:http -- --profile diagnosis --data-profile user
bun run --silent verify unit
bun run --silent verify unit --domain memory
bun run --silent verify unit --affected --explain
```

引数なしでは軽い起動選択画面を開く。この時点では製品DB migration・復旧、Provider接続、microphone、常駐workerを開始しない。選択肢はフルセット、機能単独、自己診断、単体テスト。画面でデータprofile、実接続/fixtureを区別する。既定は実接続、資格情報なしの場合は不足を表示し、fixtureへ勝手に切り替えない。

`data-profile user`はTauriと同じデータディレクトリ・設定を使用。Tauri identifierから現在の標準パスを共通resolverで解決し、文字列の別ハードコードをしない。`isolated`は最小DBを新規作成し、設定の読み込み/必要な資格情報の利用は画面で明示選択する。データや秘密を大規模複製しない。fixtureは受入自動化用の隔離profileに限る。

| profile | 初期化するもの | 開始しないもの |
| --- | --- | --- |
| launcher/tests | UI、認証、検証job manager | 製品runtime/DB/Provider |
| diagnosis | 設定/証跡reader、診断store、checks、必要catalog port | 会話queue、schedule、steward、記憶更新、MCP poll、coding常駐、マイク |
| feature | storage、設定、対象usecaseと必須依存、結果台帳 | 無関係なworkerと自動会話。依存が必要でも常駐workerは別指定 |
| full | 全serviceを登録し、保存済み設定で有効なworkerを起動 | 保存設定がOFFの機能を強制ONにしない。未設定Providerを勝手に置換しない |

profile切替は同一セッション内のstop→join→再組立て。データprofileの変更は旧DBロック解放後に新規sessionIdを発行する。fullからdiagnosisへの変更で旧taskが残っていないことを確認する。全profileに「起動中サービス/worker/子プロセス」「使用中binaryのrevision」「入力ツリーrevision」を表示する。

既存10画面を再利用し、HTTP用の模造Settings/Memory画面を作らない。profile単独のshellは共通画面部品を載せ、App全体のsnapshot必須を回避する。機能ページは[ProviderUnitTestPage](../../src/features/providerUnitTest/ProviderUnitTestPage.tsx)をtransport注入可能に変更する。アバターは描画fixtureとLaya実推論・音声同期のモードを別表示する。

### DB所有と起動順

1. auth/session/resourceを準備。選ばれたdata directoryをcanonicalizeする。
2. lifetimeのプロセス間排他ロック取得。**Tauriにも同じロックを実装**し、HTTPだけのロックにしない。OSの所有ロックを使い、PIDファイル存在だけで判定しない。
3. DBを開く。migration、資格情報instance、writer/readersを共通構築。失敗なら資源を解放し、復旧処理へ進まない。
4. profileに必要なserviceを構築。domain復旧はそのdomainを所有するruntimeを開始するときだけ行う。diagnosisは起動中処理の台帳を勝手にunknownへ書き換えない。
5. 依存順にstartし、snapshotを公開。壊れた一部サービスはfailedとして理由を表示し、fullの準備完了とは区別する。
6. shutdownは新規受付停止→依存の逆順にcancel/drain→台帳確定→join→DB/OS資源解放。SIGINT、SIGTERM、launcher EOF、UI停止を同じ経路へ集約する。

diagnosisで通常アプリがDBを所有中なら、読取専用接続で設定/証跡のquickのみ許可する。migration/復旧/write/full probeは拒否し、実行中アプリの診断画面へ案内する。読取中の一貫性はSQLite snapshotで確保。通常DBへの書込みが可能なfull/featureは常に排他所有とする。

## 4. transportの仕様と網羅性

frontendに`src/lib/transport/{contracts,tauri,http}.ts`と選択入口を追加する。画面は`invoke/listen/channel/binary/preview`の型付き共通口を使う。`@tauri-apps`の静的importはtauri adapterのみに閉じ、HTTP bundleからTauriモジュールをロードしない。偽の`window.__TAURI__`は作らない。

[transport.tsv](http-runtime-inventory/transport.tsv)のfrontend全行を移管する。主要対象は`src/lib/runtime,providerRuntime,serviceRegistry,audioBackend,audioIpc,nativeVoiceCapture,qwenRealtimeAsr,ipcChannels,roleRouting*,scheduleApi,voiceBehaviorRuntime,auditRuntime`と、chat/hooks/artifacts/UI、coding/work/worker/memory/diagnosis/media/設定/辞書/Provider試験のAPIである。単にinvokeを置換せず、イベント解除、binary、stream、resource URLまで対応する。

HTTP契約を次に固定する（Rust側で各型へdecode/dispatchし、任意関数を反射実行しない）:

| 経路 | 用途/必須フィールド |
| --- | --- |
| `GET /api/v1/runtime` | sessionId、profile、revision、service状態、host capabilities。秘密値なし |
| `POST /api/v1/runtime/profile` | profile/dataProfile/features/expectedRevision。切替operationIdを返す |
| `PATCH /api/v1/runtime/services/{id}` | enabled/expectedRevision/cascade=false。競合409、必須依存停止409、無効ID404 |
| `POST /api/v1/commands/{registeredName}` | 既存IPCのtyped argsとrequestId。固定登録表でallowlist。即時結果またはoperationId |
| `GET /api/v1/events` | 認証済みSSE。sessionId/eventId/runId/sequence/type/payload。最終IDから再接続 |
| `GET /api/v1/operations/{id}` | 処理の正本snapshot、終端、取消状態、最後のsequence |
| `POST /api/v1/operations/{id}/cancel` | 取消要求受付。遠隔停止確認とは別state |
| `POST/GET /api/v1/blobs/...` | 音声upload/成果物の既存上限と型。任意ローカルpath禁止 |
| `POST/GET /api/v1/verification/jobs...` | 固定suite IDの起動・結果・取消。任意shellや任意ファイル指定を受け付けない |

既存media HTTP経路とNDJSON clientは互換wrapperとして残せるが、serviceと台帳は一つだけ。移行完了後も既存labテストが通ること。native PCMはSSEに巨大JSONを流さず認証済みbinary WebSocketで送る（capture session ID、sequence、sample rate/channel/frame countをheaderで指定）。Origin/session検査とbounded queueを適用し、drop時は欠落を記録してASR区切りへ反映し、無制限bufferにしない。

更新commandのrequestId重複は同じ結果/operationを返し、再送で二重生成・二重仕事を作らない。TauriとHTTPの配送形式が違っても業務のrunId/台帳/取消を共用する。event backlogは上限付きとし、追いつけない場合はgapを通知してsnapshot再取得へ移る。再接続を業務再実行に置換しない。

loopbackだけにbindし、Host/Origin/session認証を必須とする。launcherは短寿命・一回限りのbootstrap codeをURL fragmentでブラウザへ渡す。画面はPOSTでcodeを交換し、直ちにhistoryからfragmentを消す。再利用を拒否し、交換は一致するHost/Originからだけ許す。長寿命session tokenはURLやログへ出さずHttpOnly cookieに保持する。書込みではCSRF対策を維持。成果物HTMLは別originまたはsandboxで隔離し、アプリ認証cookie/親frame権限を渡さない。

### API移管台帳

H00で`docs/plans/http-runtime-inventory/api-migration.tsv`を作る。列は`old_source,command_or_event,direction,request_type,response_type,stream_type,usecase,tauri_adapter,http_adapter,profile,contract_test,status`。添付commands全行に対応する。with_handler登録、event literal/定数/動的名、Channel、frontend呼出しを突き合わせる。重複commandは一つの操作へ対応付け、元行は両方残す。

statusはpending/implemented/verified/desktop-shell。desktop-shellにできるのはwindow geometry/closeなど外殻だけ。会話・音声・source閲覧・Providerをそこへ逃がさない。native helperの別window表示はsurfaceKindを明示する。登録されない宣言はunregistered理由を残し、単なる消去で棚卸しを合わせない。新規API/eventもCIで台帳未登録なら失敗するようにする。

## 5. 接続ON/OFFの決定仕様

共通runtimeのservice registryが正本。保存済み設定にsession overlayを重ねる。テスト目的のOFFで保存設定を変えない。UIは要求状態、実効状態、依存先、active run数、停止理由を表示。stateは`disabled/starting/running/stopping/failed/blocked`。remote停止未確認のrunは別の`unknown`で保持する。

全変更をrevision付きで直列化する。OFFのlinearization point以後は新規dispatch、retry、queue claimを拒否し、公開ツール/文脈も外す。既に作成された会話から来る古いtool callもdispatch直前に再検査。許可したrunはgeneration付きleaseを持ち、OFF後の古い結果を通常回答/記憶へ採用しない。ただし監査、取消、台帳終端は書ける。失効leaseと新しいONのleaseを混同しない。

| service ID（画面表示） | 対象コード/止める経路 | 依存とOFF中の進行処理 |
| --- | --- | --- |
| `agent` | conversation queue、turn/tool loop、入力受付 | 必須providers/設定。入力受付停止、active turn取消、未実行queueはhold。ONでholdを勝手に再送せず再開操作を表示 |
| `contextstill.recall` / `.search` | memory/context_still_*、agent_dispatch、context組立て | 任意接続。両方独立、まとめてOFFも可能。呼出し取消、古い結果は文脈に不採用 |
| `memory.read` | personal state/recall/snapshotの会話への注入・検索tool | 保存記録閲覧は維持。OFFは会話からの読出しを止め、削除しない |
| `memory.write` | extract/consolidation、結果のmemory採用、更新worker | 永続キューをholdしgeneration失効。forget等のユーザー管理操作と監査は維持 |
| `memory.export` | episode_exportとretry | scopeやconsolidation設定は保存状態のまま。新規送信停止、進行中取消と確認。外部消去を取消完了と混同しない |
| `tools.discovery` | tool_selection selection/catalog、MCP公開口 | OFFで選択系toolを外す。legacy直接toolへ自動fallbackしない。各実行gateも閉じる |
| `mcp:<source-id>` | MCP managerのsourceごとのpoll/session/dispatch | sourceごと停止、他sourceは維持。既存session閉鎖、取消未確認は記録 |
| `web.search` / `web.content` | runtime/web_fetch公開・dispatch・worker | 任意。実行中取得取消、WebView破棄。別backendへ再送しない |
| `coding` / `workers` | coding terminal/runtime、worker_agents、worker_lane | 未着手をhold、実行中取消。ファイル変更を巻戻さず、確認済み/不明を台帳に残す |
| `generated.tools` / `generated.ui` | generated_capabilities publication/実行、generative_ui | 公開と実行を両方停止。既存成果物閲覧は維持。ONで設定を再読込し起動時固定制約を除去 |
| `media.image` / `media.music` | saaa-mediaの新規run・retry | 種類別gate。同じservice/台帳。取消後成果物がremoteで生成された可能性を維持 |
| `situation` | monitor、会話への最新観測の注入 | monitor停止。最後の値はstale表示、稼働中扱いしない |
| `schedule` / `steward` / `adaptive` | timer/observe、pump、adaptive worker | 次回claim停止、既存処理は下流leaseに従う。schedule OFFで手動会話まで停止しない |
| `voice.asr` / `voice.tts` | capture/ASR delivery、TTS生成/playback | ASR OFFは明示的録音停止。TTS OFFは再生取消だけでASRを止めない。同一VPIO所有者を維持 |
| `provider:<registry-id>` | providers/session/probe、用途route | 設定済みrouteを保つ。停止で利用側をblockedにし、勝手に別Providerへ変更しない |

上表の粒度を固定する。HTTP対応不足による「OFFしかできない」を実装済みとしない。依存は必須/任意/診断時のみをservice descriptorへ記録する。必須依存のOFFは既定409で影響する利用側を返す。UIで一括停止を選ぶと`cascade=true`の同一operationで利用側から逆順停止。ONは必須依存の明示的起動を先に行い、任意接続まで強制ONにしない。

停止期限は共通最大30秒を初期値とし、既存domainがより厳しい期限を持つ場合は維持する。期限超過時はfailed/stopping理由と未確認runを残し、OFF成功にしない。強制process終了でも外部変更をロールバックしたとは扱わない。ON/OFFを3回繰り返し、worker数、tool一覧、呼出しcounter、採用結果が期待どおりか検証する。

## 6. 診断単独・native同等性

### 診断

`DiagnosisContext`は設定/記録reader、clock、catalog、必要probe port、runtime effective states、event sinkだけを持つ。既存CheckSpecの入力をこれへ変更。aggregate/storeと証拠のtier/TTL/reasonを維持する。[useDiagnosisReport](../../src/features/diagnosis/useDiagnosisReport.ts)の注入口へHTTP backendを渡す。

quickはProvider推論・process spawnなし。現行のLARM catalog読出しだけ許す。full/capabilityは対象probeの依存を一時leaseで開始し、成功/失敗/timeout/dropいずれも解放する。通常の会話/定期workerをそのために起動しない。診断profileのサービス未起動と、fullで意図したOFFを区別する。過去の成功証拠は時刻付きで表示できるが、今のruntimeがrunningである証拠と混同しない。

### nativeを除外せずTauriだけを外す

macOSを最初のfull受入環境とする。HTTPは「native codeなし」を意味しない。VPIO、WebView、OS権限はTauri非依存で実装する。他OSは現在の対応範囲を維持し、未検証platformへ同等性を宣言しない。

| 機能 | HTTP hostの実装方針 | 受入 |
| --- | --- | --- |
| マイク/TTS/AEC | 既存macos_vpio.c/hとFFIをnativeへ移し、同じinstanceでcapture/render。helperが必要なOS所有はnative-hostへ集約 | TTSだけでASR発話なし、人の重畳発話は届く。途中TTS OFFでもcapture継続。純粋PCMテストはvoiceで実行 |
| マイク権限 | Tauri以外のhelper app bundleに既存の用途宣言/必要entitlementを用意。専用bundle IDと権限状態を診断表示 | 許可/拒否/再起動を実機確認。Tauriの権限取得済みをHTTPの証拠にしない |
| 動的Web取得 | 既存ContentFetcherを維持し、HTTPはmacOS WKWebView helperを実装。静的HTML fast pathも共有 | JSで描画されるfixture、redirect/unsafe URL、期限、取消、background動作、guard結果を同じ契約で比較 |
| 生成HTML preview | 管理下成果物は別originのsandbox iframe。token/許可資源/期限を共通化 | 親権限・cookieへの到達不可、再表示/解放/スクロール/失効を確認 |
| 外部source閲覧 | X-Frame-Options/CSPでiframe不可のページはnative helperの別windowへ表示。`surfaceKind=native-window`を明示し、既存UIからopen/scroll/closeを操作 | 表示位置だけTauri埋込と異なる。閲覧・操作・cancel・履歴を維持し、単なる外部ブラウザ丸投げで代替しない |
| Codex/role-routing等sidecar | RuntimeResourcesから現バージョンを解決。必要な資源のprepareをverify管理に分離 | 設定した実agent起動/停止、資源欠損時の具体的エラー。mock差替えなし |
| Window/外部URL/file | Tauri window管理はdesktop-shell。HTTP側のアプリ停止はruntime stopと明示UI、ブラウザtab closeは同一視しない。外部URL/fileはhost portで検証 | 悪意あるpath/URL拒否、キャンセル・permission拒否、終了漏れなし |

WKWebView helperは既存Apple SDKとC/Objective-Cのbuild経路を使い、Tauri/tauri-pluginをリンクしない。現在のplugin型に依存するprojectionをローカルの型付きdocumentへ変換し、guard結果とURL/redirect検査を維持する。新しいWeb取得アルゴリズムへ変えない。ライセンスを確認せず外部pluginコードをコピーしない。既存Bun sidecarだけで動的Web取得の同等性が証明できなければ、static-onlyへ縮退して完成にしない。

helper通信は親子pipeの固定request enum/requestIdと上限付き出力を使い、主threadのrun loopとasync処理を分ける。一般network listenerを持たせない。EOF/SIGTERMでVPIO・WebView・子processを回収する。HTTP/desktopの同時DB所有禁止に加え、デバイス所有の競合も診断する。

## 7. 全単体テスト・影響選択・検証UI

### 単体テストの移管契約

`verify unit`は既定で全frontend unitと全Rust業務unitを発見・実行する。外部通信はfake、DBはin-memoryまたは隔離fixture、時間はfake clock。ネットワーク/実機/Provider課金が必要な試験をunitに混ぜない。既存`verify test`とadvance/fullは従来範囲を維持し、unit成功を全体成功としない。

`docs/plans/http-runtime-inventory/test-migration.tsv`をH00で作る。列は`baseline_path,case_or_macro,old_feature,new_package,new_case,suite,reason,evidence,status`。suiteはunit/transport/native/live。既存添付tests全行を処理し、macro-generatedとparameterizedは展開後の名前・ケース数で追跡。名前変更は対応表を残す。`native`へ移せるのはOS/Tauriの実結合を確認する部分だけで、同じtest内の業務assertはunitへ分離する。fixture/補助moduleも対応付ける。

unitのdependency closureに`tauri`, `tauri-build`, `tauri-plugin-*`, `wry`, `webkit*`、`src-tauri`, `saaa-native`/native-hostが入れば失敗。HTTP buildはnativeを許すがTauri系は禁止。dev-dependency/build-dependency/feature/target条件も対象とし、Cargo metadataをverify内部で取得して判定する。HTTPやunitからIPC binding生成のためにdesktop exampleをbuildしない。export例をcontractsへ移し、既存`ipc:generate`もverify/serial管理の共通exportへ接続する。

unit case discoveryと実行結果を照合し、対象ありで実行0件、予期せぬignore、未移管ケース、未分類ファイル、parse失敗は失敗にする。正規表現だけで「全件」としない。テストmacro/cfgの候補は添付一覧を入口に、Rust libtestのlistとTS runnerの報告を取得して照合する。実行環境が揃わないsuiteはblocked、テスト起動済みだけではpassedにしない。

### 自動選択

既存`verify-affected-*`, `verify-input-*`, `verify-fingerprint`, `verify-report`を拡張する。入力はHEAD差分だけでなくstaged/unstaged/untracked/削除/renameと変更前後manifestの和。domain変更は依存するpackageと公開契約利用側を含める。frontendはimportの利用側と対応テスト、共有transportは全frontend契約を含める。共通config/Cargo.lock/package lock/build脚本/generated binding/所有不明は広い検証へ倒す。

unit modeのfallbackは**全unit**でありdesktopを混ぜない。影響したnative/IPC統合は`additionalRequiredSuites`として必ず表示し、unitだけでreadyにしない。通常affected advance/fullは必要なdesktop/nativeを含む。変更なしでも自動全件unitか明示no-changeを返し、0件実行を全件合格にしない。新しいdomainは初期から対応表に登録し、avatar allowlistだけのselectedを完成扱いしない。

共有verification lock取得後に対象を決め、開始/終了のhashとwatcherで並行編集を検出する。1回再計画しても変化が続けばinconclusiveで終了。未追跡ファイルと削除も対象。生成物の意図した変化と外部編集を区別できなければ安全側に不確定とする。開始時結果を現在ツリーの合格として再利用しない。

### 起動中に検証する

HTTP launcherはprepare/buildだけverify lockを保持し、稼働プロセスは解放する。DB/機器所有ロックとは別物。稼働中のnative/.so/sidecarをbuildで上書きしないよう、起動時に実行資源だけの小さいversioned bundleを作る（target全体のコピー禁止）。revision付きmanifestを保持し、停止後に不要bundleを削除。単体テストは隔離DB/機器fakeを使う。既存Tauri devのlock契約は変更しない。

ブラウザ検証画面は全件/領域/変更影響から選ぶ。suite→verify引数の固定表で起動し、任意command文字列を受け付けない。verifierがstdoutを診断用、machine reportを結果用に返す。reportの`executedTestCount`はprocess/step数でなく実ケース数。0件/取消/dirty入力は成功にしない。jobのqueued/running/passed/failed/cancelled/inconclusive、対象、所要時間、完全な失敗ログ、入力revisionをbackend側で保持する。

## 8. 実装作業票

状態はH00以外もすべて未着手として開始し、既存実装は差分確認後に再利用する。各票は変更対象、必要作業、合格条件を満たしてからverifiedにする。モジュールサイズ基準に合わせファイルを分割し、baseline更新で収めない。

| ID / 前提 | 変更対象と作業 | 合格条件・次工程 |
| --- | --- | --- |
| H00 / なし | 添付inventoryと現在ツリーを照合。API/テスト移管TSV、`scripts/runtime-contract-check.ts`（追加）、全module→第2節の所有者対応を作成。中央とwrapper handler、event、Channel、動的名を登録 | 未分類候補を機械検出。登録漏れfixtureで失敗する試験。台帳は未実装状態を正直に保持し、以降の移管で追記 |
| H01 / H00 | contracts/storage抽出。AppStateの取消/ID、SQL writer/readers、credential instance、資源解決、DB排他を共通化。Tauri側も同じDB ownerを使用 | 隔離DB migration、旧データ読取、二重所有拒否が復旧より先、transaction原子性、credential非漏洩のunit。既存Tauri API wire不変 |
| H02 / H01 | applicationのServiceRegistry/TaskSupervisor/bootstrap/shutdownを追加。`lib.rs` setupの起動一覧をdescriptor化。まずfake serviceで依存順/逆順停止/rollbackを検証 | 起動失敗途中のtask/DB漏れなし。revision競合、cascade、OFF/ON3回、epoch不採用がunitで通る |
| H03 / H02 | diagnosis crateとDiagnosisContext、独立profile、既存画面backendを接続。storage/catalog/probe portsを注入。providersのcatalog/probe最小部分はここで先行抽出し、PCM診断の純粋関数もvoiceへ先行移管 | Tauriなしで診断unit実行。quickでworker/process/inference 0、catalogのみ。full probe timeoutでlease解放。これを最初の動作実証にする |
| H04 / H01-H03 | HTTP host、launcher、共通TS transport、認証、events/operations/blobs、profile画面を実装。mediaの配送を統合 | 実診断画面がHTTPで動く。未知command/auth/Origin/gap/duplicate request/cancel/stream終端の契約試験。Tauri adapterの既存試験も維持 |
| H05 / H02,H04 | providers/conversation抽出。queue/turn/tool loop/role-routing/context/履歴を移す。共通EventSink/ProviderPortへ変更しTauri commandも委譲 | HTTPの既存会話画面で設定した実agentへ送信・stream・取消・履歴再表示。fakeのcontractと実接続の証拠を別記録 |
| H06 / H05 | memory/situation抽出と各接続gate。agent_dispatchの公開/実行とcontext注入、記憶更新/exportを第5節へ接続 | OFF後に新規呼出し/文脈付与/採用なし、管理閲覧・forgetは維持。ON復帰、memory tests移管、並行変更の回帰なし |
| H07 / H05,H06 | tools/work抽出。MCPごとのgate、Web ports、coding/worker、生成能力/UI、schedule/steward/adaptiveをTaskSupervisorへ移行 | 各接続のOFF/ON、hold/取消/未確認、副作用保持。schedule OFFでも手動会話可能。直接tool fallbackの迂回なし |
| H08 / H01,H04 | voice/native/helper抽出、VPIO・音声権限、WebView/preview host、sidecar resourcesのprepareを実装。純粋PCMはvoiceへ | 第6節のnative契約。HTTP graphにTauriなし。既存OS挙動/guardを維持。unitからnative C buildが不要 |
| H09 / H05-H08 | 既存mediaとProvider試用画面を統合。ASR/TTS/LLM/Laya両種/Embedding/image/music/avatarのfeature profileを接続 | 1機能ずつ試用、無関係worker 0。fixtureと実Providerを明示。取得/取消/進捗/成果物保存/復旧の同じserviceを通る |
| H10 / H03-H09 | frontend40ファイルのTauri参照と全API台帳を解消。10画面full起動、profile切替、接続操作画面。Tauri setupを共通bootstrapだけへ | HTTP bundleのTauri直接参照0（adapter除く）、台帳pending 0。全画面の正常/失敗/権限拒否。full→diagnosisで旧worker 0 |
| H11 / H00から逐次、H10で完結 | 全Rust/TS unitを各所有先へ移す。contracts exportをdesktopから分離。`verify unit`とdependency監査を追加 | 未移管業務test 0、missing/double-count検出、graphにTauri/nativeなし。削除/ignoreで件数を合わせない |
| H12 / H11 | verify affectedの全domain対応、revision付きreport、検証UI、起動中bundle/lock管理、http-smoke/runtime-live/native-runtime入口を追加 | unknown/deletion/rename/untracked/並行編集で安全側。package指定不要、0件passなし、稼働中unit実行可能 |
| H13 / H10-H12 | 下の受入一覧を実施し、Tauri回帰・live・native・時間/容量を記録。旧lab入口は互換aliasとして整理 | A1-A7すべて証拠付き。未実装/未移管0、未検証を隠さない。実装記録と起動手順を本書末尾に更新 |

H08は音声/ブラウザnativeの作業量が大きいが省略不可。H03の診断単独、H05の実会話が通った時点で利用可能な中間成果として示してよい。ただし本タスクをそこで終了しない。

H02では登録・lifecycleの基盤を作り、具体serviceはH03以降に順次接続する。未移管serviceはpendingとして表示し、ダミー成功を返さない。H05の中間実会話は明示的に限定profileで検証できるが、保存済み設定の書換えで依存を隠さない。H10でfullのpendingをすべて解消する。

## 9. 受入試験と実行入口

既存の確実に利用できる入口:

```sh
bun run --silent verify test --scope typescript -- tests/verify.test.ts
bun run --silent verify advance --package crates/saaa-media
bun run --silent verify advance --package services/feature-lab
bun run --silent verify --scope typescript
bun run --silent verify:advance
bun run --silent verify:full
```

部分工程では新しいcrateごとに同じ`verify advance --package <実在ディレクトリ>`を使い、影響する利用側も検証する。全体gateは共通bootstrapの切替と最終受入で必要。調査目的や各ファイル移動のたびにfullを回さない。既存失敗は原因と未実行範囲を記録し、影響範囲の失敗を残してreadyとしない。

以下はH11/H12で**verifyの正式subcommandとして実装する**。単なる文書内の架空コマンドで残さない。CLI parser/planner/lock/report/異常終了まで既存verifyへ組み込む。`runtime-contract`は検証時の棚卸しも担当する。

```sh
bun run --silent verify runtime-contract
bun run --silent verify unit
bun run --silent verify unit --domain diagnosis
bun run --silent verify unit --affected --explain
bun run --silent verify http-smoke --profile diagnosis --fixture
bun run --silent verify http-smoke --profile feature --fixture
bun run --silent verify http-smoke --profile full --fixture
bun run --silent verify runtime-live --data-profile isolated
bun run --silent verify native-runtime --data-profile isolated
```

`--explain`はplanのみ（試験未実行と表示）、それ以外の成功は既存契約どおり`OK`のみ。詳細はJSON reportへ。fixture smokeは追加依存を入れず既存browser smoke基盤を利用。`runtime-live`は設定済み接続の選択を必要とし、不足をfailedまたはblockedで返してfixtureへ落とさない。native/liveはオプトインで、通常unitから自動実行しない。

| 試験ID | 手順 | 必須の判定 |
| --- | --- | --- |
| R01 | runtime-contractで全manifest/登録command/wrapper/event/frontend使用/台帳照合 | 未分類・未移管・対応漏れなし。HTTP/単体test依存閉包が禁止packageに到達しない |
| R02 | diagnosis HTTPを隔離DBで起動→quick→probe timeout→停止 | quickの許容catalog以外network/process 0、会話/記憶/schedule worker 0、probe後資源0、TTL/既存失敗保持 |
| R03 | full fixtureで10画面を順に開き更新操作・再読込 | stub全成功でなく実usecase/SQLiteを使う。保存値・一覧・audit・エラーが正しい |
| R04 | 会話開始→stream中取消→event切断→再接続 | 二重turnなし、sequence/gap/終端一回、履歴とoperation整合。古いterminalで新runを閉じない |
| R05 | 各service ON/OFF3回、active中OFF、古いtool call/結果返却、cascadeとrevision競合 | OFF後dispatch/retry/採用なし、ONで復帰、worker重複0、保存設定不変、remote不明を成功にしない |
| R06 | 各featureを単独起動し、成功/失敗/期限/取消/結果不明/履歴を試す | 9対象すべて確認。実Providerとfixtureの結果を別記録。image/musicの成果物内容を実際に表示/再生 |
| R07 | 実agent＋記憶/外部MCP等をON/OFFして会話 | 実呼出し/auditと文脈・tool一覧で接続差を確認。文章の見た目だけで判定しない |
| R08 | full→feature→diagnosis→full、SIGINT/TERM/EOF、起動途中失敗 | 不要task/子process/device所有が残らない。復旧を二重実行しない。旧sessionイベントを拒否 |
| R09 | 同じDBでTauri/HTTPを逆順に起動、diagnosis readonlyも試す | writer二重所有拒否。拒否側が台帳を変更しない。readonlyでmigration/probe writeを拒否 |
| R10 | Origin/Host/auth異常、未知command、任意path、巨大音声、成果物HTML攻撃 | 拒否して機密漏洩なし、処理を実行しない。最大値/型は既存契約を維持 |
| R11 | JS描画ページ、redirect、安全でないURL、guard拒否、source閲覧、preview失効 | Web取得・表示の機能同等性。static-only化や全ページiframe化で誤魔化さない |
| R12 | 実機でTTSのみ→人の重畳発話→TTS停止、権限拒否/再起動 | TTSのみASR発話なし、重畳発話は届く。capture継続、AEC/duckingの実効値を記録 |
| R13 | unit全件、影響選択、変更なし、未知config、rename/delete/untracked、実行中編集 | ケース対応が保たれ、0件/編集後をpassにしない。unit fallbackでもTauri buildなし |
| R14 | HTTP稼働中にunit、sidecar関連変更build、verifier取消 | lock適用、稼働bundle不変、revision差表示、検証process回収、target巨大複製なし |
| R15 | 全体advance/full＋Tauriで会話/音声/診断/Provider/終了 | 共通化の回帰なし。native/liveの不足をfull成功だけで補完しない |

### 計測

cold/warmを混ぜず、同じprofile・変更・cache状態でHTTPとTauriの使用可能時刻、増分build/link/unit時間、lock待ち、RSS、target容量差を記録する。最低3回のwarm観測の中央値と範囲、環境・失敗を残す。cold計測のためにclean/別target/巨大コピーを行わない。cold未計測はそのまま記録する。高速化を事前に約束しないが、Tauri依存除去はgraphとbuild commandの証拠で必ず確認する。

## 10. 進捗・再開・終了報告

実装中は本節の下へ次の形で追記する。作業IDの完了、全体完了、環境待ちを混同しない。

```text
Hxx: pending / implementing / implemented / verified / blocked
変更: ファイル・移管前後・共有API
契約: API台帳行 / test台帳行 / Rxx
検証: 実行コマンド、実ケース数、結果、入力revision、log/reportの場所
未実行: 理由（既存障害、実Provider未設定、権限、など）
次: 次に実施するHxxと残っている具体的作業
```

再開時はstatusだけでなく差分/入力revision/台帳と照合する。実装済みでも未検証ならverifiedへ進めない。最終報告には使える起動コマンド、A1-A7判定、単体ケース移管数/残数、R01-R15結果、native配置差、性能実測、既知の制限を書く。

### この引渡書の作成記録

製品コードは変更せず、調査用build/test/live呼出しも実行していない。静的棚卸しとリンク・既存コマンド・対応表を確認した。文書を1回レビューし、storage→domainの逆依存、application→native依存、診断工程が後続Provider抽出を待つ順序矛盾、session bootstrapの未定義部分を修正した。実装の受入結果はまだない。Context Compile/Evalは今回各1回、会話累計各4回。
