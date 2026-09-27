# SAAA：Jarvis 5 Provider 初回実装計画

作成日: 2026-09-26  
状態: J1のdispatcherとRust ASR監査チャネルからの観測接続を実装・回帰確認済み。J0はLARM APIで一時接続を確保しQwenとornithの並行生成を1回、複数行JSON制御レコードから本文へ続く10件の形式試験を確認した。`replyTo`等の項目充足、分類精度、実音声の遅延は未確認。通常入口の切替は未完了。

## 1. 到達点と最初の着手範囲

[コンセプト](saaa-jarvis-five-provider-concept.md)を、[Runtime契約](saaa-jarvis-five-provider-runtime-contract.md)に従って既存Runtimeへ段階的に実装する。[実装状況・検証手順](saaa-jarvis-five-provider-implementation-status.md)は現状と証跡の参照先とする。本書は作業順序と完了判定を定め、元の契約を緩めない。

最初の音声統合の到達点は、**ornithが仕事Aを処理している間もASRとQwenが依頼Bを受け付け、Bを保存し、両モデルの音声を重ねずに返せること**とする。質問への回答返送、条件変更、停止、再起動時の重複防止まで含める。embeddingの検索接続はその次の独立した実装単位とし、接続診断だけで5 Providerの完成とはしない。

最初に着手するのは **J0「ベースラインと経路確認」→ J1「入力更新dispatcher」**。J1は決定的な模擬入力と通常入口の観測で検証し、新しいLLM要求、仕事登録、発声はまだ有効にしない。周期・訂正・区切りを先に固めることで、後続のモデル・DB・音声の不具合と切り分ける。これは最初の変更単位の完了であり、Jarvis音声応対の完成ではない。

## 2. 計画作成時に確認した再利用箇所と不足


| 領域 | 確認したコード | 計画への反映 |
| --- | --- | --- |
| ASRの版付き更新 | [contracts.rs](../../src-tauri/src/voice/streaming_asr/contracts.rs) に `Partial/Final`、発話ID、revision、時間範囲、stable/unstable本文がある | 契約を新設し直さず、dispatcherの入力へ正規化する |
| 通常の音声受付 | [ambientVoiceCaptureActions.ts](../../src/features/voice/ambientVoiceCaptureActions.ts) はpartialを表示し、finalを `submitPrompt` へ送る。[useConversationTurn.ts](../../src/features/chat/useConversationTurn.ts) は実行中の音声入力を最大4件待機させる | 途中入力のQwen受付を通常turnの完了待ちから分離する。既存final入口との二重配送を切替時に防ぐ |
| 本人登録と照合 | [enrollment.rs](../../src-tauri/src/voice/profile/enrollment.rs)、[streaming_verifier.rs](../../src-tauri/src/voice/profile/streaming_verifier.rs)、[speaker_gate_runtime.rs](../../src-tauri/src/voice/streaming_asr/speaker_gate_runtime.rs) がある | 未実装と決めつけず再利用する。profileなし・filter無効では `all-speakers` になるため、その状態を本人限定モードの成功にしない |
| 話者ゲートの待ち | 16 kHzの1,600 samples単位、15ブロックの窓、5ブロックhop、複数票を待つ処理がある | コード上の窓は1.5秒・hopは0.5秒。これはQwen送信周期とは別の待ち。実遅延を測り、精度を保たず閾値だけを緩めない |
| TTS中のcapture | [conversationTurnControls.ts](../../src/features/chat/conversationTurnControls.ts) はspeechStartedでsuspendを呼ぶ。ただしnative captureかつbarge-in有効なら、上記capture actionsはdetachを省く | WebView/nativeを分けて確認する。「常にマイク停止」「常に継続」のどちらにも一般化しない |
| 自声除去の候補経路 | [audio_backend/macos.rs](../../src-tauri/src/voice/audio_backend/macos.rs) にVoiceProcessingとAEC状態、[playback_vpio.rs](../../src-tauri/src/voice/http_audio/playback_vpio.rs) に同backendへの出力がある | 実際の入力・TTSがこの経路を共有するか確認する。AEC有効フラグだけで自声除去合格にしない。system-tts等の別出力も個別に検証する |
| Qwen受付 | [conversation_provider_route.d/01.rs](../../src-tauri/src/runtime/conversation_provider_route.d/01.rs) はHTTP streamを全受信後に解析する | SSE受信とヘッダー／本文解析を分離し、1要求で早期本文を取り出す |
| 仕事・会話の正本 | [butler_loop/ledger.rs](../../src-tauri/src/runtime/butler_loop/ledger.rs) に `conversation_work_state`、`conversation_run_inputs`、`conversation_events` がある | 既存workを思考依頼の正本として拡張する。Qwen専用の仕事DBを作らない |
| Role Routing台帳 | [schema.rs](../../src-tauri/src/role_routing/schema.rs) にroots、steps、inputs、events、speech、Toolリンクがある。会話ごとのactive root制約がある | 受付を新しいactive rootにせず、仕事の実行時に既存root/runへ結び付ける。制約を外して並列化しない |
| 入力の重複排除 | [receipts.rs](../../src-tauri/src/role_routing/repository_turns/receipts.rs) は同じsource IDで本文が違う再送を競合とする | ASRの正常な訂正を旧入力の再送と混同しない。発話revisionと仕事revisionを区別する |
| 音声queueとplayer | [speech_queue.rs](../../src-tauri/src/role_routing/speech_queue.rs) は主に確定本文単位、[playback.rs](../../src-tauri/src/voice/http_audio/playback.rs) はappend・stop・drainを扱う | 句、公開枠、pause/resume、消費frame、訂正を追加する。現行のstopをpauseとして流用しない |

既存の[Butler実装計画](butler-conversation-runtime-implementation-plan.md)からは仕事台帳・通常会話の継続を再利用する。第一声の確定・再生開始を全Toolの必須待ちにする規則は、今回の受付音声とornith準備の並行方針へ一律には持ち込まない。既存の操作権限・必要な承認は維持する。[macOS音量低下の検証計画](verification/macos-asr-playback-ducking-plan.md)は音声経路の回帰観点として参照し、無関係な音量修正へ範囲を広げない。

## 3. 実装の単位と依存関係

| 単位 | 成果 | 依存・完了判定 |
| --- | --- | --- |
| J0 | 実効経路・遅延・構想の成立判定 | 最初に実施。話者ゲート、Qwenの1要求形式、GPU同時実行を小さく実測し、各分岐を決める |
| J1 | 1.5秒更新・区切り即時送信の入力dispatcher | 純粋ロジックはJ0と並行可。通常入口への観測接続と切替判断はJ0後。模擬時計で競合・訂正・backpressureを検証し副作用0 |
| J2a | 本人限定の常時入力と自声除去 | J0の遅延判定後。WebView/nativeと実出力経路の能力表を完成 |
| J2b | 1要求Qwen adapter | J0の形式・同時実行判定後。SSE契約試験と状態を要しない簡単な分類評価を完了。J2aと独立 |
| JE | 日本語の固定評価集合 | J2bと並行して作成。群別・重大操作別の独立ケース、ラベル、分割、採点器を揃え、J3受入前に固定 |
| J3 | 永続思考キュー・質問返送・取消 | J1のID契約とJ2bの制御形式に依存。短い状態投影を使った宛先判定をJEで評価 |
| J4 | 両モデル共通の早期TTSとpause/resume | 公開枠はJ2b、仕事の版・停止はJ3に依存。キューの模擬試験は先行可能。実機割り込みはJ2aの自声除去合格後 |
| J5 | 最初の音声統合を受け入れる | J2a・J2b・JE・J3・J4の全条件を満たす。長考中の受付・音声順序・復旧とコンセプト段階2〜4の受入を実機で確認 |
| J6 | embeddingの検索接続と5 Provider統合 | J5後の独立変更。Tool検索で既存方式と比較し、コンセプト段階5〜6まで検証する |

J0の成立判定前にJ1の純粋ロジックと模擬試験は進められるが、通常入口の切替はしない。J2a/J2bとJ3のmigration案を確定してから実動作を切り替える。各単位は独立してレビュー可能な差分にし、先行の未コミット変更を混ぜてcommitしない。

### J0：実効構成とベースライン

1. 起動build、HEAD・差分、実model/量子化、5役割の接続先識別子、入力・出力デバイス、WebView/native、話者scope、AEC経路、Context関連flagを記録する。認証情報・生の私的会話は診断へ出さない。
2. 通常入口で「挨拶」「簡単な質問」「Toolが必要な依頼」「長考中の追加入力」「TTS単独」「TTSと本人発話の重なり」を測る。温間・冷間を分離する。測れない経路は理由付き未測定とする。
3. `音声終了→話者ゲート解放→ASR更新→Qwen送信→ヘッダー→最初の句→TTS要求→実再生開始` の時刻を同じ相関IDで結ぶ。既存auditに不足する時刻だけ追加する案を確定する。
4. Qwenとornithの同時要求が実際に進むか、共有GPU・lease・prefillの待ちを測る。5サービスprobe成功を並行実行成功の代わりにしない。
5. 固定した短い日本語入力10件で、実Qwen 2Bが1要求で制御レコードから本文へ進めるか、LARMが必要なSSE・出力制約を提供するかを小さく試す。形式違反の件数と最初の本文までの遅延を記録する。これはJ2b/JEの分類評価の代わりにはしない。

J0の終了時に次の判断を記録する。話者ゲート後の残り時間でp95 2.5秒の可聴開始が成立しなければ、窓・推論・入力経路の変更案を比較し、目標を勝手に緩めずJ5受入を保留する。10件の試行で制御レコードと本文の1要求形式に違反が1件でもある、または必要なSSEがない場合はJ2bのadapter設計を変更し、2回生成を自動追加せず計画を再審査する。ornithの長考が続く間にQwenの最初の本文が届かない場合は、GPU/lease/配置を変更して再測定するまで、長考中受付を約束するJ3〜J5の切替を止める。測定不能は成立と扱わない。

成果はベースライン表、経路別能力表、匿名化したイベントfixture。デバイスやProviderが使えなければ模擬のJ1へ進めるが、実機受入は未完了のまま残す。ユーザー設定のリセットやProvider差替えで通さない。

2026-09-26の暫定判定では、claimed Provider上の10件で「複数行JSONレコード→本文」は成立したため、J2bの増分解析器はJSONの完結位置で分離する方式へ変更した。明示指示した`replyTo`は9件で省略され、分類の正しさもこの試行の合格対象ではないため、制御項目を無条件で採用しない。ornithとの並行生成は1試行で成立したが、反復測定は未完了。話者ゲートから可聴開始までの実機p95は未測定であり、J5の切替条件を満たしていない。接続準備も64〜267秒とばらつき、失敗例があるため、冷間起動と温間応答を分けて再測定する。

### J1：最初の変更単位 — 入力dispatcher


- 入力キーはsession・conversation・utterance・ASR revision・確定状態。ASR revisionとwork revisionは別の型にする。タイマーは注入可能な単調時計を使う。
- 発話中は1.5秒周期、区切り時は最新の未送信更新を即時flushする。後着最終認識・訂正も即時投入する。周期と区切りが同時でも同じ版は一度だけ送る。
- Qwen実行枠は1つ。処理中は同じ発話の古い途中版だけをまとめ、別発話・確定入力を黙って消さない。停止候補は優先し、実行中要求へのプロンプト挿入はしない。
- 空更新、重複、session再接続、会話切替、旧sessionの遅着を明示的に処理する。上限到達を状態として返し、ASRのcaptureを入力キュー満杯だけで停止しない。
- 初回は観測モードで同じイベントを模擬Qwen sinkへ渡す。実Provider・TTS・仕事保存へは接続しない。既存final配送が唯一の実動作入口のまま、重複数・滞留・送信予定時刻だけを比較する。

必須試験は、1499/1500ms前後の更新、周期と区切りの同時到着、本文不変のfinal化、遅着訂正、Qwenが3秒かかる場合、別発話混在、session失効、満杯。合格は送信順・版・件数が期待どおりで、副作用0、既存のASR回帰が通ること。タイマーの実sleepに依存する試験にしない。

### J2a：本人限定の常時入力

既存の話者登録とverifierを使う。本人限定モードでは `all-speakers` を受理可能にせず、登録不足・デバイス不一致・モデル不一致を表示する。WebView/native、TTS出力経路ごとに、TTSだけの誤入力と本人が重なったときの受付を実機で測る。話者ゲートの窓・推論時間を別に測り、p95で2.5秒を超える原因ならJ0の基準と比較して改善する。AECフラグやマイクを止めた状態の試験で合格にしない。

自声除去未成立なら、契約どおり自動割り込みを無効にし、再生中の本人未確認入力を業務へ流さず制限を表示する。これは暫定動作でありJ5の成功条件を満たさない。咳判別やpause/resumeが未対応のadapterも能力表へ明記する。

### J2b：Qwenの1回生成

`frontend_completion` から通信・構造解析を小さなadapterへ分離する。1要求で、完結した制御レコードの後に本文deltaを受ける。実Qwenの1件は複数行JSONの後に本文を出したため、行末ではなくJSONオブジェクトの完結位置で制御と本文を分ける。UTF-8/SSE/JSONの境界分割、重複ヘッダー、本文先着、形式エラー、取消、timeout、出力上限を模擬HTTPで検証する。保存・状態確認待ちの本文は上限付きで保持する。2回目の本文生成は自動追加しない。純粋な増分解析器 `runtime/qwen_control_stream.rs` は実装済みだが、実Providerへのadapter接続と10件のレコード妥当性評価は未完了である。

ヘッダーは無効な対象・版・曖昧性をRuntimeが拒否できるようにし、モデルのconfidenceだけを許可条件にしない。J3完成前は模擬repositoryで制御要求を検証し、実業務を起動しない。簡単な応答の実機確認は隔離した試験入口で行い、通常入口との二重生成を避ける。

J2bの分類評価は、状態投影を要しない挨拶・簡単な回答・引継ぎに限定する。`replyTo`、仕事の条件変更・取消の宛先判定はJ3で実台帳から短い状態投影を作ってから評価する。J2bの成功を宛先判定の成功として扱わない。

### JE：固定評価集合と採点器

Runtime契約の群別100件以上と重大な誤操作の種類ごとに独立1,000件以上を、J3の受入前に用意する。日本語原文、ASR途中版・訂正、進行中の仕事・質問の状態、正解宛先、許される確認、期待する台帳変更をラベルに持たせる。作成・レビュー・重複除去・開発用／固定評価用の分割を作業として計上し、モデル調整後に固定評価集合を使い回さない。件数と誤操作の機会数を別に報告する。合格線の正本はRuntime契約第1章に置き、本書では繰り返して定義しない。

### J3：既存台帳へ思考キューを接続する

初期の正本対応は次の案で固定し、既存DDL・起動復元処理とのmigration試験で確認する。別のqueueテーブルを作るだけで仕事状態を二重管理しない。

| 概念 | 対応・拡張案 |
| --- | --- |
| `taskId` と仕事revision | `conversation_work_state.work_id/revision`。待機順・状態・継続先を必要最小限追加する |
| 実行ごとのrun/root | `runtime_runs` と `rr_roots/rr_steps`。Qwen受付のたびにactive rootを作らない |
| 発話と訂正 | utterance/revisionの受付記録を既存台帳に関連付ける。確定会話本文は `conversation_messages`。途中版ごとに確定発言を増やさない |
| `questionId/replyTo` | work・revision・提示speechへの参照を持つ質問記録。既存 `open_questions_json` との二重正本を避け、同じtransactionで消込する |
| 再送と配信 | message ID・受付receipt・未配信状態を台帳に保持する。保存とoutgoing eventを同一transactionにする |

Qwen受信箱と仕事queueはRustが所有する。Qwen用の短い状態投影は台帳から作り、全文Memory・Tool catalogを渡さない。ornith実行枠は初期はアプリ内1つとし、実行可能な仕事をFIFOでclaimする。確認待ちは枠を解放する。回答を受けた継続は契約の優先規則で再開し、実行中の別件を自動中断しない。会話をまたぐ状態や音声を混ぜず、別会話の結果は当該会話へ配送する。

質問返送、条件変更、取消は保存と対象版の再検証を通す。停止Alertは本人ASRの到着時にRuntimeが検出し、Qwenを優先する。Alertだけで全Toolを止めず、確定した対象に `RunCancellation` と新規Tool起動禁止を適用する。再起動時の実行中・Tool送信済みは結果照合へ進み、自動再実行しない。

実装時に最も注意する競合は、①ASR訂正をsource ID競合として拒否する既存receipt、②Qwen受付とornithが会話のactive rootを奪い合うこと、③既存Butler入力消費と新queueの二重消費である。入力所有者を会話session単位で一意に選び、切替中の旧runは終了・取消確認まで旧経路が所有する。

完了条件は、A実行中のB/C保存、FIFO、Q1への回答先着・再送・失効、DB commit直後の配信失敗、二重claim、再起動、取消不可Toolを模擬試験で通すこと。加えて、旧Butler消費者と新queue消費者を同じ隔離DB・会話sessionで同時起動し、ASR訂正とQwen受付を競合させる。受付receipt、`active root`、仕事claim、会話本文の各IDについて、二重消費0・紛失0・意図しない競合0を確認する。切替中の旧run終了・取消確認が遅い場合も再現する。

J3の宛先判定はJEの固定集合と台帳由来の短い状態投影を使い、合格線はRuntime契約の正本に従う。未達なら常用切替を止め、規則との併用・モデル変更・入力形式の改善を比較する。重大操作を毎回ユーザー確認へ縮退する選択は可能だが、不要確認率や正当な操作の受付率を含む契約合格としては扱わず、機能制限と再評価を明記する。

### J4：公開本文から一つの声へ

既存 `speech_queue/speech_repository` を発話・句の契約へ拡張する。現在の `rr_speech` のkind/status制約、同一root/revision/kindのunique制約が複数質問・訂正・pauseを表せるかをmigrationで検証する。公開発話と最終採用の記録を分け、単純に `BufferedRoleStepSink` を外さない。

- Qwenとornithの公開枠だけを共通dispatcherへ接続する。初期は合成worker1つ・player1つ。本文の句読点で即合成し、全文の完了を待たない。
- speech単位の順序を維持し、正常終了の末尾だけflushする。古い相槌は同じ仕事の回答到着で失効する。内部出力、失効版、再送を読まない。
- playerにpause/resume/cancel、開始・drain・消費frame通知を用意する。pauseはPCMを保持し、cancelは世代を失効させて破棄する。rodioとnativeのadapterごとに検証する。
- 本人発話・停止Alert・確認待ち等の保留理由を別に持つ。相槌判定で一つ解除しても、他の保留があれば再開しない。確認が必要なら旧音声を退避して確認speechを再生する。
- 採用却下後の停止・訂正イベントを台帳へ一度だけ保存し、聞こえた可能性のある範囲を次のContextへ渡す。再起動後の音声自動再読はしない。

完了条件は、句の交互混在・同時再生・重複・内部出力の発声0、pause後の句の再読0、遅着終了による新owner解除0。無限生成・合成失敗・上限超過・停止未確認の各経路で無限占有を防ぐ。能力未対応のplayerを対応済みとして選択しない。

### J5：通常入口の切替と最初の音声統合

J1の観測経路を通常入口へ切り替える条件はJ2a・J2b・JE・J3・J4の合格とする。試験時の有効化は隔離DB・sessionの明示的な実効モードで管理し、保存済みProvider設定を置換しない。実行中にモードを切り替えず、旧runと音声をdrainまたは取消確認してから新sessionを開始する。

固定の日本語30発言・10往復以上で、長考中の別件、質問への「明日」、聞きながらの「うん」、条件変更、停止、接続断、再起動を繰り返す。TTS単独と本人重なりを実スピーカーで確認する。指定された機器と実model/buildを記録し、温間・冷間を分ける。p95で2.5秒の実回答開始、句読点からp95で1秒、Runtime停止要求から500ms、相槌後p95で1秒の目標を測り、未達の項目は理由と改善対象を残す。値を緩めたり相槌だけで合格にしたりしない。

回帰があれば新規sessionの有効化を止める。処理中の仕事は台帳を保持して整合させ、旧経路へ入力を再送して二重実行しない。追加schemaの破壊的な巻戻しや設定初期化は行わない。J5合格で最初の音声統合を完了とし、5 Provider全体の完成はJ6後に判定する。

### J6：embeddingの利用と継続運用

[tool_selection/mod.rs](../../src-tauri/src/tool_selection/mod.rs) のローカルembedding/rerankerを比較基準に、Harness `embed_query` をTool候補検索の消費先へ接続する。同じquery集合で取得品質・遅延・同時負荷を比較し、rerankerは根拠なく削除しない。model・次元・前処理・source版で索引世代を分離し、Scope・削除・忘却を検証する。

合格しなければ既存検索を維持して未達と報告する。Memoryへの全移行や再索引は初回の前提にしない。最終的にembedding負荷中のASR/Qwen/TTSと、コンセプト段階6の復旧・継続利用を確認する。

### 規模・リスクと停止条件


J0の3つの成立条件が未達ならJ2以降の通常入口切替を止める。JEの集合と採点器が未完成、J3の重大誤適用が残る、J2aの自声除去が未成立、またはJ4の単一音声所有が破れる場合もJ5へ進めない。各停止時は未達の条件、代案、再測定条件を記録する。


各単位で変更箇所に必要な試験を行い、J5/J6で統合する。以下は実装時の予定コマンドであり、今回実行済みではない。

| 対象 | コマンド | 成功条件 |
| --- | --- | --- |
| J1の新dispatcher | `cargo test --manifest-path src-tauri/Cargo.toml runtime::voice_frontend`（新規module案） | 模擬時計で境界・順序・重複・取消が一致。実装後にフィルターが実際に試験を実行したことも確認 |
| ASR保護域を変更する全単位 | `bun test tests/ambient-voice-session.test.tsx tests/voice-capture-races.test.ts tests/voice-asr-packet-sender.test.ts` と `cargo test --manifest-path src-tauri/Cargo.toml voice::streaming_asr` | 両方成功。既存final配送・capture終了の回帰なし |
| native入力・本人照合 | `bun test tests/ambient-native-voice-capture.test.ts tests/voice-profile.test.ts` と `cargo test --manifest-path src-tauri/Cargo.toml voice::profile` | profile未準備・機器変更・native経路を含め成功 |
| J2b/Qwen・既存役割経路 | `cargo test --manifest-path src-tauri/Cargo.toml butler_route_tests` | 既存挨拶・引継ぎ・失敗処理と新しい1要求契約の期待値が一致 |
| JE/J3の分類 | 固定集合の採点器（JEでコマンドを確定） | Runtime契約の群別・重大操作別の件数と誤操作・不要確認・受付率を報告 |
| J3/台帳と復旧 | `cargo test --manifest-path src-tauri/Cargo.toml role_routing` と `cargo test --manifest-path src-tauri/Cargo.toml runtime::butler_loop` | transaction、migration、重複消費、再起動復元が成功。旧DBコピーでも確認 |
| J4/音声 | `bun test tests/voice-final-delivery.test.ts tests/voice-pipeline-response.test.ts tests/larm-voice-owner.test.ts` と `cargo test --manifest-path src-tauri/Cargo.toml voice::streaming_tts` | early本文と旧finalイベントで二重再生しない。新しいplayer adapter試験も追加 |
| J6/embedding | `cargo test --manifest-path crates/larm-session/Cargo.toml` と `cargo test --manifest-path src-tauri/Cargo.toml tool_selection` | 契約・検索・索引世代・縮退が成功。実検索の比較証跡を併記 |
| 初回応答保護域の変更 | `bun run quality:check` と `bun run desktop:smoke` | macOSでsnapshotをloadしprimary conversationを用意して両方成功 |
| IPC変更・統合 | `bun run ipc:generate`、`bun run ipc:check`、最終的に `bun run check` | 生成契約・型・既存回帰が一致。未実行や環境失敗を合格に含めない |



試験が失敗したら、既存ベースラインとの差・対象・原因を記録し、対象修正後に同じ条件で再実行する。環境障害は前提を復旧して再試行し、原因不明の反復やtimeout延長だけで合格にしない。模擬試験はProviderなしで実行可能にし、実モデル分類評価・マイク受入・GPU同時負荷試験は明確に別レーンにする。

## 5. 完了報告と変更範囲

各単位の報告には、変更した契約とファイル、実行した試験と件数、未実行の条件、変更前後の遅延・精度・失敗率、凍結更新理由を残す。実機証跡は実model・設定fingerprint・デバイス・buildと相関IDを付け、文書だけの更新と実装完了を区別する。

今回の実装対象から外すものは、会議録音・呼びかけ先分類、Tool権限の緩和、自律的な仕事の追加、全Memory再設計、複数ornith worker、複数同時TTS、無関係な診断UI改修である。既存変更を捨てず、設定・DBはコピーか隔離環境で試験する。
