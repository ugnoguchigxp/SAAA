# SAAAの用途別クラウドAPI切り替え実装計画

作成日: 2026年10月4日 JST

状態: 方針合意済みの実装計画。製品コード、保存設定、実サービスへの接続は本書の作成では変更していない。現状欄は当日の作業ツリーを読んだ結果であり、実機の動作認定ではない。

SAAAがLARMと外部クラウドを用途ごとに選べるようにする。サービスの登録、利用可能な機能とモデル、用途への割り当てを分離し、設定で選んだ接続先と実際の処理を一致させる。会話は既存の単一キューとツールループを維持し、まず会話LLMの直接依存を解消してから音声、生成、背景処理へ広げる。

## 正本と適用範囲

製品コンセプトの正本は[SAAAの全体コンセプト](https://chatgpt.com/space/page_9fc5877949748191b556705128f6a2f5)である。本書は、そのModel ProviderとAgent Providerと音声の区別、保存設定と実効Routeの表示、単一会話Runtimeを具体化する実装計画であり、別のコンセプト文書ではない。

正本のProvider設定、資源管理、Memoryの方針と、10月1日の単一会話Providerへの決定を確認した。用途の追加を受付モデルと回答モデルへの分割、旧五つのProvider固定構成、旧会話executorの復元理由にしない。新しい製品方針が必要になった場合は、正本Pageへ反映し、本書には差分と整合結果を記録する。

### 実装する範囲

- 接続サービスと複数機能の登録、資格情報への参照、設定の保存と移行。
- 用途ごとの能力確認、接続先解決、実行開始時の設定固定、取消、タイムアウト、明示的な代替先。
- LARMと直接クラウドの共通実行境界。最初は既存のChat Completions形式とHTTP音声形式を扱う。
- サービス登録と用途選択を分けたUI、プリセット、設定と実際の利用先の表示。
- 画像と音楽の生成、背景の記憶整理、埋め込み、Agentと検索サービスへの段階的拡張。
- 設定移行、送信境界、会話と音声の回帰、実サービスの受入確認。

### この計画に含めない変更

全クラウドサービスの同時対応、URLだけから未知のAPIを実行する汎用エンジン、品質や料金による自動モデル選択、Memoryの正本や保存方式の全面変更、新しいVector DB、外部Memory製品への置換は含めない。WebSocket等の通信方式を全Providerへ一律に要求したり撤去したりしない。

コード実行の権限、sandbox、承認、ネットワーク許可は既存契約を維持する。認証保管方式の変更やOAuth全般の実装、新規の大型SDK導入は初期範囲に含めない。Rust、SQLite、reqwest、Tokio、既存React UIを基点にする。

### 守る契約

保存設定を初期化しない。起動失敗を理由に接続先を置き換えない。DB移行試験はコピーまたは隔離DBで行う。既存の未コミット変更を取り込む、戻す、上書きする作業は本計画の実装と分ける。

ASRはTTS再生中もマイクを受け取り、独立した人の声と重なった声を配信する。macOSでは同じVoiceProcessingIOを通す入出力とAECを維持し、他音声のduckingを明示する。残留エコー処理は実際に再生したPCMを参照する。再生フラグや時間窓によるマイク停止、フレーム破棄、ASR配信停止を導入しない。

## 現状と変更の根拠

| 領域 | 当日のコードで確認した内容 | 変更方針 |
| --- | --- | --- |
| 個別登録 | [IndividualProvidersSection](../../src/features/settings/IndividualProvidersSection.tsx)はLLM、Agent Session、ASR、TTSを追加できる。設定は用途別Provider単位 | 接続情報と資格情報を共有できるサービス単位の登録へ整理する |
| 接続先選択 | [ServiceConnectionsSection](../../src/features/settings/ServiceConnectionsSection.tsx)はLLM、ASR、TTSのsourceとProviderを保存する | 「用途ごとの選択」へ集約し、LARMとクラウドを同じ候補選択で扱う |
| 通常会話 | [queue_runtime/conversation_answer.rs](../../src-tauri/src/runtime/conversation_check/queue_runtime/conversation_answer.rs)はcached_larm_asrからSessionを取り、complete_larm_role_with_eventsを使う | 現行会話キューの下で用途Bindingを解決し、選んだLLMを呼ぶ |
| LARM接続 | [conversation_check.rs](../../src-tauri/src/runtime/conversation_check.rs)のconnect_larmはtts、asr、llm、embeddingをまとめて要求する | 利用する機能集合を接続計画へ渡す。クラウド会話にはLARM接続を要求しない |
| ASRとTTS | 同conversation_check.rsと[streaming_speech.rs](../../src-tauri/src/runtime/conversation_check/streaming_speech.rs)にHarnessと直接Providerの分岐がある | 既存音声処理を保持し、選択と認証と監査を共通化する |
| 既存LLM transport | [chat_completions/mod.rs](../../src-tauri/src/providers/chat_completions/mod.rs)は旧executorのContextやTool設定を受けるとUnavailableを返す | ガードを単に外さない。現行キューから使えるtransport境界を作り、低水準HTTPとSSE処理だけを必要に応じて共有する |
| LLMの型 | [provider_settings.rs](../../src-tauri/src/models/provider_settings.rs)のrequest_options等はlarm-sessionのLlmOptionsに依存する | SAAAの意味上の要求型とProvider固有のwire optionsを分離する |
| 画像と音楽 | [media_generation/mod.rs](../../src-tauri/src/media_generation/mod.rs)はLARMのMediaClientに直接依存する | LARMのサービス発見と外部生成APIをそれぞれMedia adapterへ置く |
| 記憶整理 | [product_binding.rs](../../src-tauri/src/memory/personal_state/product_binding.rs)はLARMのSession、claim、context subject、専用capabilityを使う | 記憶の正本、source/View管理と推論実行を切り分ける。一般LLMの差し替えだけで対応済みにしない |
| 設定保存 | [settings/documents.rs](../../src-tauri/src/persistence/settings/documents.rs)は複数documentを一つのtransactionで保存する | 新しい登録と用途Bindingも既存Writerで原子的に検証・保存する |
| 設定版 | [settings_migration/stored_document.rs](../../src-tauri/src/persistence/settings_migration/stored_document.rs)にsettings_revisionと更新triggerがある | 既存revisionと、関連設定から作るfingerprintを使う |
| 実効Route | [effective_route.rs](../../src-tauri/src/persistence/effective_route.rs)は旧conversation.respondとprovider_sessionsを参照する | 現行キューのjobと用途別attemptの実績も参照する。旧表の存在を現行会話の実行証拠にしない |
| 資格情報 | [credentials.rs](../../src-tauri/src/credentials.rs)のnamed secretは現在credential_secretsへ保存される | 実装をOS Keychainと仮定しない。既存backendを抽象境界越しに維持し、参照を移行する |

既存のRuntimeとProviderのREADMEには削除済み入口の参照が残る。着手時は実在する現行キューから呼出しを追い、旧文書の入口を復元しない。

## 用途と対応段階

用途は「どの仕事に使うか」、能力は「サービスが何を実行できるか」である。同じモデルを複数用途へ割り当てても、用途別の予算、送信許可、優先度は別に持つ。

| 用途ID案 | UIの用途 | 必要能力と契約 | 導入段階 |
| --- | --- | --- | --- |
| conversation.respond | 会話と回答 | テキスト生成、現行action契約、Context予算、取消。早期表示と発話には対応するstream契約が必要 | P2 |
| voice.transcribe | 声を聞く | ASR、音声形式、言語、途中版と最終版の扱い | P3 |
| voice.speak | 声で返す | TTS、voice、音声形式、共通player、再生PCMの参照 | P3 |
| media.image.generate | 画像を作る | 画像生成、jobまたは同期結果、成果物取得 | P5 |
| media.music.generate | 音楽を作る | 音楽生成、job、取消、成果物取得 | P5 |
| memory.extract | 記憶を整理 | 許可されたsourceからの構造化抽出、検証、既存Writerによる採用 | P6 |
| world.maintain | Worldを更新 | 既存の主張・根拠・失効・採用契約と推論処理 | P6 |
| search.embed.memory | 記憶検索用の埋め込み | モデルと次元と前処理が固定された索引 | P6 |
| search.embed.tools | Tool検索用の埋め込み | Tool索引固有のモデル契約。記憶索引と自動で共用しない | P6 |
| coding.assist | コードを書く | Agentのthread、stream、cancel、sandboxと既存権限 | P7 |
| retrieval.web.search | Webを検索 | 検索APIまたは既存Tool backendの契約 | P7 |

画像入力の理解はLLM resourceの入力能力として扱い、画像生成と区別する。rerank等は必要な消費先と検証ケースが決まった時点で追加する。未接続用途には「未対応」または「移行が必要」を表示し、実行しない設定を保存できる画面で完了を装わない。

## 設定とデータの構造

以下の型、namespace、module名は新設案であり、現在実装済みという意味ではない。

### 接続サービス

ServiceConnectionは安定したconnectionId、表示名、adapterKind、接続先と必要なregion等、authentication、credentialRef、有効状態、接続設定版を持つ。接続先が複数あるサービスはadapterが機能別endpointを解決する。URLからProviderの種類を推測しない。

credentialRefは既存named secretのserviceとaccount等へのopaqueな参照とし、secret本文を設定JSON、UIの読出し、監査、エラーへ含めない。利用者のキー入力はbackendへ書込むための経路にだけ渡す。接続先変更時は以前の動作確認を失効させる。リダイレクトや予期しないoriginへ資格情報を引き継がない。

### 機能とモデル

ServiceResourceはresourceId、connectionId、モデルまたはサービスのselector、能力集合、入力と出力形式、token容量とbyte上限、streamとcancelの契約、voice等の用途固有設定を持つ。

能力情報の出所を設定テンプレート、利用者の明示、サービス広告、動作確認に分ける。確認状態、確認時刻、設定fingerprintを別に保持し、広告能力と実動作を混同しない。Unknownを成功に変えない。モデル一覧APIがないサービスでは手入力を許し、利用可能なモデルを固定リストで断定しない。

同じ接続先に会話モデル、ASR、TTS、埋め込みを複数登録できる。移行時にendpointが同じという理由だけで接続やsecretを統合しない。

### 用途への割り当て

PurposeBindingはpurposeId、primaryResourceId、順序付きfallbackResourceIds、用途の有効状態、全体期限とattempt期限、送信先とデータ区分の許可、再試行条件を持つ。LLMのreasoning等の意味上の要求は用途側へ置き、Provider固有の詳細値はadapter optionsへ置く。

登録、用途への割り当て、動作確認済みは独立した状態である。必須用途の未選択は実行時に構成エラーとし、任意用途の無効化は通常会話全体の起動を止めない。別用途の選択を暗黙に継承する既定値は設けない。プリセットは具体的な割り当てを一度生成する操作とする。

### 保存先と一貫性

既存settings_documentsへproviders.connections/default、providers.resources/default、routing.purposes/defaultを追加する案とする。namespaceの許可、cross-document validation、RustとTypeScriptの型、設定load/save IPCを同時に更新する。schema versionの次番号は着手時の値から決め、現在の15を将来の固定値にしない。

三document、互換表示への投影、移行状態を同じWriter transactionで保存する。既存settings_revisionはdocument更新ごとに増えるため、保存一回がrevision一増加であるとは仮定しない。読出しは整合したsnapshotで行い、保存にはexpectedRevisionを追加して競合を検出する。関連設定のみのfingerprintも作り、無関係なUI設定変更で実行資源を作り直さない。

新設定の有効化後は新documentを正本とする。旧providers.modelとrouting.tasksの編集は、対応範囲を新Bindingへ変換する一つの入口へ統合する。新設定と旧設定を別々の正本として双方向同期する構造を作らない。Agent等のまだ移行していない設定documentは当該契約の正本として維持する。

## 実行経路とProviderの境界

### 共通の選択と実行

1. 現行キューがjobを開始し、用途、Scope、必要なデータ区分を提示する。
2. 整合した設定snapshotからPurposeBindingを解決し、能力、送信許可、有効状態、Context予算を確認する。
3. connection、resource、設定版、fingerprint、選択理由、代替候補、期限をResolvedRouteとして固定し、jobまたは用途操作の台帳へ記録する。
4. adapterが接続と必要資源を準備する。LARMだけがprofile、claim、lease、capacity、releaseを扱う。
5. SAAAの共通Contextと要求をadapterへ渡す。Provider固有の役割、履歴、相関ID、署名等は当該adapterが正しく保持する。
6. eventと結果を現行Runtimeへ返す。Runtimeがツールを実行し、根拠と取消を再確認して回答または成果物を採用する。
7. 用途別attemptの実績を保存し、資源を解放する。UIはこの記録を読んで実際の利用先を表示する。

用途Routerは接続先だけを決める。Actor、recipe、仕事の採用、Tool許可を再決定しない。通常会話では既存queueが唯一の実行所有者となり、Role Routingの画面や保存状態を並行した会話選択器として使わない。別のAgent実行経路との接続はP7で境界を定義する。

### 会話の共通要求と出力

ModelRequestはSystemContext、履歴と証拠、現在の入力、出力契約、Context容量、出力予約、期限、取消を含む。Provider固有のrequest JSONを共通型として強制しない。token上限、byte上限、モデルの入力容量と出力予約は分ける。

初期は現行のanswerまたはTool actionのJSON契約を維持する。既存[action parser](../../src-tauri/src/runtime/conversation_check/queue_runtime/action.rs)の重複キー拒否、有限Tool loop、根拠の再検証、回答の一回保存を保つ。短い挨拶も同じ会話Providerを使う。

stream eventは回答本文の増分、Tool要求、内部推論、使用量、正常終了、失敗を区別する。現行のanswer.contentとして取り出せる公開本文だけを表示・TTSへ渡す。Tool引数や内部推論を読み上げない。EOFだけで正常終了にしない。必要な出力契約を満たせないモデルは会話候補から除外するか、検証可能な制限付きモードとして理由を示す。

Providerのnative tool calling対応は後段で追加できるが、adapterの変換後も同じRuntimeによる許可と実行と採用を通す。外部SDKにSAAAのTool loopや無断の自動実行を委譲しない。

### adapterの分割

能力別にModel、ASR、TTS、Embedding、Media、Agent、外部Toolの境界を持つ。共通化するのは選択、認証参照、期限、取消、監査であり、全能力を一つの汎用JSONメソッドへ押し込まない。

初期adapterはLARM、直接Chat Completions、直接HTTP ASR、直接HTTP TTS、System TTSとする。AnthropicやGemini等のnative APIは、必要な機能と公式契約を確認し、各adapterとfixtureを追加する。対応済みadapterのサービスは登録操作で切り替えられ、新しいAPI形式への対応はadapter追加で行える構造にする。

LARMのSessionは制御接続、profile、必要能力集合、設定版、認証世代に結び付ける。用途ごとに常に別Sessionを作る必要はないが、別profileや別credentialを誤共有しない。LLMだけの利用に四機能を要求しない。leaseは実際の要求に結び付け、失敗と取消とshutdownでもreleaseを照合する。

HTTP/SSEを既存基点とし、サービス固有の通信方式はadapterへ閉じ込める。transport変更のためにcaptureやplayerの所有者を増やさない。

### 設定変更と中止

通常の選択変更は新しい会話job、ASR発話、TTS発話、media job、背景jobから適用する。同じ会話jobのTool loopは同じResolvedRouteを保持する。履歴に拘束されるProviderは検証された引継ぎや新しいsessionを使い、別モデルへ履歴や内部推論をそのまま移植しない。

ASRは同じutteranceの途中版と最終版で接続先を固定する。次の発話への切り替え準備中もcaptureを継続し、boundedなbufferと既存revision契約で受け渡す。buffer不足は検出可能な入力エラーとし、黙って捨てない。TTSは一つの発話内でモデル、voice、decoderを固定する。

有効状態の取消、送信許可の撤回、Scope失効、secret削除は通常の選択変更と区別する。各新規送信とTool実行の前に最新の拒否条件を再確認し、拒否なら停止する。固定したRouteの代わりに別サービスを選ばない。取消はローカルの採用停止と遠隔計算停止を分けて記録する。

### 障害と代替先

接続障害、capacity、rate limit等で代替先を使う場合も、保存済みの候補順、用途の送信許可、残りの全体期限に従う。認証、権限、契約違反、入力過大、取消では自動で接続先を替えない。

P2の初期会話fallbackは、Tool実行、本文の公開、音声再生がまだない最初の推論attemptに限定する。後続Tool step、部分回答公開後、遠隔受付済みか不明な生成要求は自動で再送しない。ASRは採用済みの版を二重配信しない。TTSは再生済みの文を繰り返さない。Mediaは生成POSTの受付が不明なら同じjobの照合へ進み、重複生成しない。

失敗をProviderFailureとして分類し、retryable、outputStarted、effectStarted、受付の確実性、部分成果、遠隔job IDを保持する。費用と使用量の未取得値はnullとし、0や成功に置き換えない。実費制限を広告価格や推定tokenだけで保証したことにしない。

### 実際の利用先の記録

現行job IDからpurpose、connection、resource、model、設定fingerprint、attempt ID、Provider request ID、開始と終了、選択理由、代替理由を追えるようにする。既存provider_sessionsとtransport eventsを優先して拡張し、jobとの関連が不足する場合だけ最小のcolumnまたは関連表を追加する。

設定選択、接続確認、送信開始、受付確認、完了、成果採用を別々の証拠として扱う。UIの「直近の利用先」はprobe成功から作らない。過去設定の実績には設定版と時刻を付け、新設定では「未実行」と表示する。全文プロンプト、secret、音声PCMを通常のRoute監査へ保存しない。

## UIの実装

### 登録したサービス

登録導線はサービスの種類を選択、表示名と接続情報を入力、認証情報を保存、機能とモデルを登録、用途別に動作確認の順とする。既知のサービスはadapter別テンプレートを使い、独自URL等は詳細設定へ置く。登録フォームには対応形式と未対応の能力を表示する。

初期は既存資格情報backendの制約に合わせ、接続を無効draftとして保存してからsecretを登録し、確認後に有効化する。途中失敗や再起動から続けられる。複数操作を一つの成功と誤表示しない。再利用したsecretを失敗時の後始末で削除しない。新しい画面に旧保管方式の説明が残る場合は実装に合わせて直す。

モデル一覧取得や接続確認は利用者の操作で行う。無条件の全サービスprobeや生成テストを起動時に行わない。生成を伴う確認は送信する固定入力と対象用途を示し、記憶本文や会話履歴を試験入力に流用しない。MediaのColdサービスを定期起動しない。

### 用途ごとの選択

行には用途、選択したresource、確認状態、現在処理中または直近に使ったresourceを表示する。会話、聞く、話す、画像、音楽を基本表示とし、記憶とWorld、埋め込み、Agent、外部Toolを詳細表示へまとめる。

能力だけでなく言語、入力形式、Context容量、出力契約、送信許可が適合する候補を表示する。不適合や未対応の候補は理由を示す。ユーザーが無効化したresourceを自動有効化しない。候補がない任意用途は無効を選べる。

用途の詳細では全体期限、attempt期限、代替先の順序、許可されたクラウド送信を設定する。localOnlyはLAN URLやlocationという自己申告だけで証明しない。既存のlocal binding等の実行契約に基づき、外部転送を含むサービスは許可から除外する。

### プリセットと適用

LARM中心、登録クラウド中心、会話だけクラウドを用意する。初期プリセットは基本用途のBindingだけを生成し、索引再構築や記憶送信許可を含めない。変更一覧と未対応用途を表示してから適用する。「会話だけクラウド」は会話をクラウド、ASRとTTSをLARMへ明示的に割り当てる。

実装する画面は、前の提案の操作例をそのまま保存状態の正本にせず、backendの設定snapshotと用途別attemptへ接続する。draft、保存、資源準備、実行済みを分ける。設定競合時は再読込と差分確認を案内する。適用時に実行中の依頼を黙って取消しない。

### 特別な移行を伴う操作

埋め込み変更は、対象索引、件数、モデル契約、新索引の構築状態を表示する移行画面へ進む。構築完了と検索検証後に切り替える。記憶とWorldには移行済み範囲と維持している専用契約を示す。Agentは既存の権限とworkspace選択を併記し、会話モデル登録だけでコード実行を許可しない。

サービス削除では利用中用途を提示し、Bindingの変更または任意用途の無効化を同じ保存で行う。既存jobが参照する設定snapshotと監査識別子は削除しない。secretの削除は接続削除と分け、共有利用と実行中参照を確認する。

## 設定の移行と互換性

1. 登録Provider、Harness、用途Route、資格情報参照、無効状態、timeout、voice、詳細optionsをlosslessに棚卸しする。利用者の実DBを検証用に書き換えない。
2. 旧Provider IDから安定したconnection IDとresource IDを作る対応表を保存する。複数用途の同一credentialRefは共有できるが、別キーを推測で統合しない。LARM control credentialとclaimで得た短期credentialを区別する。
3. 保存設定と実行経路が一致している用途は新Bindingへ変換する。設定と実際の経路が食い違う会話設定はneeds-reviewとして、旧保存値と従来の実行方式を両方保持する。
4. needs-reviewでは利用者が用途画面で適用するまで新Bindingを有効化しない。互換実行モードは従前の経路と送信境界を維持する。現行会話で未適用だったクラウド設定を更新だけで実行しない。従前の境界を確定できない場合は構成確認エラーを返す。
5. 旧設定の正常な値、secretの参照、無効状態、音声の利用同意を保持したまま新documentを原子的に追加する。未知のkind等は保全し、診断対象にする。既定値で上書きして読めたことにしない。
6. 移行再実行でID、設定、資格情報参照が変わらないことを確認する。新設定の有効化後、旧入口は共通保存へ接続するか読取専用にする。

資格情報保管を移す必要がある場合は別migrationとして定義する。本計画は既存credential_secretsを初期化せず、接続側から参照できるようにする。DBコピーや証拠にはsecretを含めず、テストではfixture secretを使う。

## 実装段階と完了条件

| 段階 | 作業 | 主な変更対象 | 次段階へ進む条件 |
| --- | --- | --- | --- |
| P0 現状確認 | 使用中経路と保存設定の対応、Contextとstream契約、音声、既存テストの基準値を採取 | 現行queue、設定migration、診断、fixture | 動作と設定の食い違い、既存失敗、未確認事項を証拠として残す |
| P1 登録とBinding | 三層の型、lossless migration、revision検証、credentialRef、ResolvedRoute、用途別attemptを追加 | models、settings、credentials、generated IPC、Provider registry案 | 既存設定保持、移行の冪等性、参照と能力と送信許可の拒否を隔離DBで確認 |
| P2 会話LLM | 現行queueの下にModel adapterを接続し、LARMと直接Chat Completionsで同じaction loopを実行 | queue_runtime/conversation_answer.rs、conversation_check.rs、context_compiler、Provider adapter案 | LARM未接続のテキスト会話、Tool、取消、部分失敗、回答一回保存、設定と実送信先一致がfixtureで通る |
| P3 音声 | ASRとTTSとSystem TTSを用途Resolverへ接続し、発話単位の固定と共通playerを維持 | conversation_check.rs、streaming_speech、cloud_asr、cloud_tts、audio backend、frontend capture | 混在構成、途中版と最終版、音声割込み、接続切替、AECの両条件が通る |
| P4 設定UI | サービス登録と用途選択、詳細設定、基本プリセット、設定と実行の表示を接続 | SettingsPage、ServiceConnectionsSection、IndividualProvidersSection、ProviderCard、RoleRoutingSection、i18n | 登録から適用と再起動、競合、無効化、削除、needs-reviewを一連のUI操作で確認 |
| P5 Media | 同期とjob型の共通結果を定義し、LARMをMedia adapterへ接続。最初の外部画像adapterを追加 | media_generation、LARM MediaClient、成果物IPC | 二重生成防止、受付不明、再起動からjob照合、取消、成果物保存を確認。外部音楽は対応adapterの受入後に有効化 |
| P6 記憶と埋め込み | source/View所有と推論を切り分け、許可された背景推論を共通Model adapterへ接続。索引版と移行を実装 | personal_state product_bindingとmanaged、World worker、埋め込み消費先、索引保存 | Memoryの正本と失効と送信許可を維持。新索引構築と比較と原子的切替、取消と再開が通る |
| P7 Agentと外部Tool | Agentと検索の登録情報を統一画面で扱い、用途Bindingから既存の実行契約へ委譲 | coding、agent_session、role_routingの境界、tool_selection backend、検索接続 | threadとsandbox等を保ち、Model登録から権限が増えず、取消と成果採用が既存台帳へ戻る |
| P8 実機受入 | 全体の設定、実際の送信先、再起動、音声、実サービス、性能を確認 | 受入fixture、実機、監査、設定コピー | 下記受入行列の対象構成が合格し、未対応サービスを明示した状態で提供できる |

P1からP2、P3、P4を初期提供単位とする。既存の必要用途と資源集合が揃った後で切替UIを有効化する。P5からP7は共通基盤の拡張として個別に提供し、未完了の用途をプリセットで自動有効化しない。

### native Model APIの追加

P2の拡張作業として、初期提供の受入後にChat Completionsと異なるnative Model APIを一つ接続する。最初のサービスはP0で必要能力と利用可能な契約から選定し、P5のMedia対応とは独立に進める。Anthropic Messages、Gemini native等が候補となる。

要求と履歴の変換、独自の終了event、usage、取消、Provider固有の署名や履歴拘束、現行actionまたはnative tool callingからの共通actionへの変換をfixtureで確認する。Contextを無視した単発テキスト成功だけでadapterを認定しない。同じ会話、Tool往復、取消、部分失敗のシナリオを通し、P4のテンプレートと候補選択へ追加して実サービス受入を行う。

### 新設moduleの候補

providers/service_registry、providers/purpose_router、providers/resolved_route、providers/operation_store、providers/adaptersを責務の単位として検討する。model_requestとcapability_contractをLARM型から独立させる。実際の配置は既存module境界に合わせ、小さなmoduleに分ける。

Runtimeはjob、Context、Tool実行、採用を所有する。Provider層は解決済み資源の準備とtransportを所有する。Persistenceはsnapshot、移行、attempt記録を所有する。UIはbackend状態の投影と編集draftを所有する。既存の単一Writer以外の書込接続を作らない。

### 記憶と索引の追加条件

P6では既存のlocalOnlyとlocal bindingの要求を緩めない。クラウド送信できるデータ区分と用途を明示したBindingだけを候補にし、制約のある既存記憶処理はローカル経路を維持する。LARM context subjectやViewのtokenを別クラウドへ流用しない。ローカルのsourceから許可されたbounded Contextを組み立て、既存の採用検証へ戻す。

索引はpurpose、resource、model revision、次元、前処理版、source版を識別する。既存consumerごとに保存先と呼出し契約をP0で棚卸しする。新旧ベクトルを混在させず、新索引を別generationへ作り、書込中のsource変更とforgetを追従してからactive参照を切り替える。取消・中断・再起動でも旧索引は利用でき、削除済みsourceを新索引やrollbackで復活させない。

## 検証計画

### 決定的な検証

全adapterのfixtureは実secretと外部ネットワークなしで実行する。比較は同じ入力、初期DB、設定、Tool条件で行う。モデルの回答品質差と接続管理の回帰を分ける。

| ケース | 期待する結果 |
| --- | --- |
| 旧設定の移行と再移行 | Provider ID対応、選択、無効状態、voice、options、secret参照、同意を保持。二回目に意味状態が変わらない |
| 保存クラウド設定と旧実経路の食い違い | needs-reviewを表示し、移行や再起動だけでクラウド送信しない |
| 設定保存の途中失敗と競合 | 部分的なBindingだけを有効化せず、旧snapshotを維持。expectedRevision違いは競合として返す |
| クラウド会話とLARM停止 | LARMの接続・claim要求が0件で、テキスト回答と許可されたTool loopが完了 |
| LARM会話とクラウド音声、その逆 | 各用途の設定と実際のendpoint、model、資格情報参照が一致 |
| 会話中の設定変更 | 開始済みjobは旧snapshot、新規jobは新snapshot。Tool loop内で接続先を再選択しない |
| 権限とScopeの撤回 | 次の送信・Tool実行・成果採用を停止し、代替先へ逃げない |
| 部分回答後、Tool実行後の失敗 | 同じ仕事を別Providerへ自動で再実行せず、部分状態と失敗を残す |
| stream終端不足とaction不正 | EOF、重複キー、複数action、schema不正を完了にしない。Tool JSONを読み上げない |
| ASR途中版と最終版 | 同じutteranceは同じresourceで処理し、revisionを二重配信しない |
| TTS中の発話と接続準備 | captureとASR配信が継続。切替により音声を黙って破棄しない |
| 実効Routeの表示 | 保存やprobeだけで実行済みにしない。現行jobからattemptを追跡し、旧設定の実績を区別 |
| Mediaの受付不明と再起動 | POSTを重複送信せず、既知jobを照合。cancel未確認をcancel完了にしない |
| 新旧索引の切替と中断 | 新旧を混ぜず、完了前は旧索引を使う。source訂正・forgetが両世代で効く |
| 共有credentialとサービス削除 | 別用途のcredentialを削除せず、参照中用途を保護。監査にsecretを出さない |

新規テスト群の候補はpurpose_route_contract、service_registry_migration、conversation_provider_switch、voice_provider_switch、service_connections_ui、purpose_routes_uiとする。名称は実装時に既存構造へ合わせる。初期は既存queueのfixtureに直接クラウドtransportを追加し、Router単体の成功だけで会話完了を判定しない。

### 実装時に実行する確認

以下は現在存在する入口である。新設テストのfilterとファイル名を段階ごとに追加し、0件実行を合格扱いしない。計画書だけの作成ではこれらの製品テストを実行済みとはしない。

```sh
# 設定と既存UIの対象確認
bun test tests/settings-review.test.ts tests/settings-regressions.test.ts tests/settings-ui-contracts.test.ts tests/http-audio-provider-settings.test.ts
cargo test --locked --manifest-path src-tauri/Cargo.toml persistence::settings

# 会話キューと音声の決定的な回帰確認
bun run e2e:conversation-queue
bun run test:asr
bun run test:provider-unit

# IPCと全体の品質ゲート
bun run ipc:check
bun run check:local

# LARMライブラリの契約を変更した段階
cargo test --locked --manifest-path crates/larm-session/Cargo.toml
```

既存testとcheck:localに変更前の失敗がある場合は基準値へ記録し、新規回帰と分けて原因を追う。未解決の必要チェックを無視して段階完了にしない。基準値を変更して失敗を消す場合は別の変更として説明する。

### 実サービスと実機の受入

実接続試験はfixtureと分けた明示的なliveモードで行う。使用するサービス、対象モデル、固定入力、送信するデータ区分、実行回数の上限を示す。料金表、具体モデルの能力、endpoint、認証契約はadapter着手時に公式資料と実応答で確認する。

最低限の構成はLARM中心、直接クラウド会話とSystem TTS、直接クラウド会話とLARM音声、LARM会話と直接クラウド音声である。各構成で新規入力、Tool往復、取消、接続失敗、再起動、設定保存後の新規依頼を確認する。LARM停止時のクラウドテキスト会話は、ASRとTTSを使わない条件で試す。

macOS実機でTTS-onlyがASR発話を作らないことと、TTS再生中の人の発話がASRへ届くことを両方確認する。収録・再生・ASR受信時刻、PCM参照、採用されたutteranceの証拠を残す。HTTPの合成成功やfixtureのASR結果だけでAEC成立を認定しない。

遅延は接続準備、最初の回答本文、最初のmixer sample、全体時間、取消反映を分ける。LARMの確保待ちとクラウドのrate limitも別に測る。使用量、cached tokens、実費等はProviderが取得可能な範囲で記録し、測定不能を0にしない。合格閾値はP0の同条件基準値と各機能の契約から確定する。

## 提供と回復

新Routingの有効化状態を明示的に保存し、段階ごとに有効化する。利用者が適用済みの接続先は、再起動や起動失敗で旧経路へ自動で戻さない。migration、startup、resource準備、実行、採用の失敗を分けて表示する。

実装のrollbackは設定と資格情報を保持することを前提にする。新namespaceのまま古いbinaryを起動できるとは仮定しない。旧形式へlosslessに表現できる範囲だけ互換投影を行い、native adapterや索引版等の表現できない設定は保持して利用を停止する。別サービスへの置換でrollbackを成立させない。

rollbackの検証には設定のコピーとfixture secretを使い、実データ全体の古いbackup復元でforgetや後続更新を戻さない。資源と索引の回復は現在のsource版と失効状態に照合する。全体失敗時は、最後に成立した設定と有用な部分成果を残し、次に必要な修復操作を表示する。

## 完了の判定

初期提供の完了はP1からP4の達成と、対応adapterについてP8の会話・音声受入が成立した時点とする。少なくともLARMと直接クラウドLLMを登録し、会話・ASR・TTSを独立に割り当て、再起動しても設定と実際の利用先が一致する必要がある。

全計画の完了はnative Model API一種類の受入とP5からP7の接続と移行を含み、各用途が実行できるか、非対応理由と必要な移行を正しく示す状態で判定する。既知の非対応サービスを明示することは全サービス対応を意味しない。提供した各用途については、選択UIだけでなく実行と成果採用と回復まで受入済みであることを求める。

実装前に確定する事項は、最初のnative LLM adapterと外部Media adapterのサービス選定、各embedding consumerの現在の索引契約、LARMの用途別profileと必要能力集合、既存Agent境界、記憶系の許可されたデータ区分である。P0で実契約を確認して各段階へ反映する。秘密情報の不足や実サービスの非対応があっても、fixtureによる共通基盤の検証は独立して進められる。

## 実装状況と計画との差分（2026年10月5日時点）

レビューで確認した六件の不具合を修正し、初期提供の会話・音声・設定UIへ実行境界と回復操作を追加した。native会話APIとしてAnthropic Messages、外部MediaとしてReplicate Predictionsを追加した。実サービス・macOS音声の受入は、決定的なfixtureの合格と区別する。

### 実装と検証の範囲

- 登録、用途選択、secret保存の途中失敗からの再開、無効化、参照保護付き削除、同一接続内の複数モデル、revision競合を扱う。APIキーは表示・監査へ出さず、既存設定と資格情報を初期化しない。
- 会話は開始時の経路を固定し、次の送信・Tool・回答採用の前に接続の無効化と送信許可の撤回を再検証する。通常の選択変更は次の依頼へ反映する。最初の推論だけ、出力・Tool実行前の接続不可・混雑・一時不提供に限り直接API間の代替先を使う。
- 全体期限は準備・Tool往復・音声待ち・採用を含む。個別の通信期限を全体の残り時間へ切り詰める。HTTPのJSON応答を受けた場合も同じ応答を処理し、別の生成POSTを追加しない。
- 既存の会話互換設定を保持し、出力上限の指定方法・推論・thinking・生成中表示をResource単位で編集できる。Context容量は既存の保守的な予算を使い、モデル別容量の編集は未対応。
- HTTP ASRは同じ発話の途中版と最終版を固定した設定で処理する。TTSも発話内のチャンクを固定する。ライブASRの方式変更はマイク再開時へ反映し、画面にその制限を示す。capture、PCMのAEC処理、音声重なりの契約は変更していない。
- 明示操作のモデル一覧・固定文の確認と、実際の用途別attempt・結果採用の履歴を別に保存する。設定変更前の利用実績を現在の設定の成功として表示しない。
- Anthropic Messagesはsystem・履歴を変換し、完成したpublic textだけを共通action loopへ戻す。native tool delegationは行わない。完了後に表示する形式で提供する。
- 画像・音楽のBinding、Replicateのモデル固有JSON入力、remote IDと設定snapshotの永続化、再起動後の照会、確認済み／未確認を区別した中止、採用前の成果物保存を実装した。生成要求は自動再送せず、同じrun IDをDBで拒否する。成果物の取得でAPIキーを外部配信URLへ転送しない。

### 維持した互換性と提供範囲

Registryは一つのdocumentを原子的に保存する。従来のProvider設定が所有する接続は読出し時に投影し、新規接続はregistryに保存する。従来形式で表せない音声接続を既存Provider設定へ上書きしない。会話のneeds-reviewは明示適用まで従来経路を維持する。

記憶・Worldの正本はローカルのRust/SQLiteにある。既存の背景推論にはlocal binding、source/View、配送・削除・tokenizer等の専用契約がある。任意のクラウドAPIへ切り替えるP6、埋め込み索引のgeneration移行は未実装であり、用途画面に非対応理由を表示する。既存索引やforget状態は変更しない。

P7の実装エージェントは既存の実装設定へ案内し、既存のworkspace・実行権限・成果採用の入口を維持する。Agentと外部検索を新しいPurpose Bindingで管理する拡張は未実装。会話モデルの登録によってコード実行権限を増やさない。

P8の実API料金・応答品質・実機AEC受入は未実施。TTS-onlyが発話を作らないことと、再生中の人の発話がASRへ届くことの両方を実機で確認するまで、音声受入完了とは扱わない。全計画の完了を示すものではない。

検証結果と残る品質ゲートは [修正検証記録](../../spec/evidence/purpose-cloud-api-fixes/20261005/verification.md) に記録する。
