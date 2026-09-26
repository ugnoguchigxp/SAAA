# Role Routing 音声会話統合計画 — VC改訂

更新日: 2026-09-21。状態: **実装予定・受入未完了**。この文書の保存は実装完了を意味しない。

[全体計画](saaa-role-routing-plan.md) / [実行契約](saaa-role-routing-execution-contract.md) / [完了ロードマップ](saaa-role-routing-completion-roadmap.md) / [受入仕様](saaa-role-routing-acceptance.md)

## 1. 正本と今回の完了範囲

音声会話については本VC改訂が旧C3.1/C8.1/C9、RR-08/13/14/15、E23〜E27の競合記述に優先する。認可、scope、入力変更barrier、toolの副作用確定、revision、再起動時の自動再実行禁止は緩和しない。Sol/Astra・学習・review全体の完成を、この音声経路を直す前提にはしない。それらの既存ゲートは別途維持する。

ユーザー承認により端末設定のRole Routingはtrueに変更済みとの前回確認がある。reasonerには既存LAN Provider、frontendは未設定だった。実装開始時に実DB・policy version・起動バイナリを再確認する。enabledは利用意思、readyは接続・設定・実行可能性であり、同一ではない。欠けたfrontendをQwenやhost定型文で代用してreadyにしない。

完了は「稼働中のアプリでASR→LFM会話→必要時Qwen思考→回答の優先再生」が成立した状態。Provider単体応答、単体テスト、設定trueだけでは完了にしない。

## 2. 会話と推論の責務

- LFMは会話担当を続ける。Qwenに**思考を依頼**しても会話担当を委譲・終了・置換しない。「常にアクティブ」は入力受付可能・接続維持の意味で、無発言時に生成を繰り返す意味ではない。
- ASRは1,500msの無音で区切った確定文を会話受付へ送る。これは依頼完了の判定ではない。発話再開で無音時間をリセットする。既存の会話pace設定がこの境界を上書きしないよう、設定表示と実際の値を揃える。
- LFMは短い相槌、簡単な応答、確認を行う。まとまった推論・調査・実行の依頼だけreasonerへ送る。初回起動の挨拶は登録済みの呼称で一度だけ許容し、再接続ごとに再生しない。未登録の名前を推測しない。
- Qwenの処理中もASR受付とLFM応対を継続する。同意・相槌・進捗確認でQwenを再起動しない。追加条件だけ既存rootのrevision更新、別依頼はqueue、明示中止はcancelへ送る。対象が曖昧なら確認する。
- 最終回答の発話をQwenに優先させるが、LFMの会話sessionは終了しない。Qwen回答後は新しい発言にLFMが応対する。停止した古い相槌を再生し直さない。

## 3. LFMの最小出力とフォールバック

### 3.1 最初に実装する契約

```json
{"say":"少々お待ちください、考えます。","think":true}
```

必須キーはsayとthinkの2個だけ。sayは空でない短文・最大80文字、thinkはboolean。未知キー、型違い、欠落、途中切れ、tool callを拒否する。LFMが生成する本文にはschemaVersion、ID、action列挙、confidence、対象、依頼の要約を要求しない。IPCのversion/ID/epochはhostが外側のenvelopeに付ける。対応Providerではこの2項目だけのJSON Schema制約を使い、非対応を無限retryしない。

think=trueは思考依頼の候補であり、ツール実行・承諾・完了の権限ではない。coordinatorは受理済みinputの範囲、現在のroot、重複、scope、残予算を検証する。sayが推論結果や作業完了を先取りしている場合は採用せず、実際に思考依頼を受理できた場合だけ短いhost待機文を出せる。queue中や失敗なのに「考えています」と言わない。

「旅行を考えているんだけど」のような未完成入力をthink=trueにする誤判定も不合格とする。JSON構文の成功率だけで判定能力を評価しない。ホストのキーワード一致やconfidence閾値を下げる修正で試験を通さない。

### 3.2 不安定ならLFMを平文へ、Qwenを並走へ

設定に `conversationFrontend.outputMode = minimal_json | plain_text_parallel` を追加する。既存設定のversionを加算しmigration・型生成・UIを同時更新する。切替判定と使用モードはpolicy/session snapshotに記録する。

- 起動時に、固定の合成入力でProviderの構造化出力対応をprobeする。不対応ならplain_text_parallelへ切り替える。実会話を起動probeへ流用しない。
- 実行中に不正JSON、途中切れ、timeoutが1回発生した時点で、そのsessionはplain_text_parallelへ降格する。同じ入力でJSON生成を繰り返さない。再接続や次の発言で自動的に昇格し直さない。
- 壊れたJSONは発話しない。同じ保存済みinput IDを使い、LFMの平文応答は最大1回再要求できる。Qwenの補助判断も同じ入力へ紐付け、二重task/toolを作らない。LFM自体が利用不可なら平文化では直らないため、音声会話をdegradedとして具体的に表示する。
- plain_text_parallelではLFMが普通の短い日本語を生成し、Qwenが同じ確定入力を並走して判断する。Qwenの分類・draftは非公開で、必要な依頼と判定される前のtool実行は禁止。reasonerの回答担当stepを二重に起動しない。
- Qwenが既に思考中なら、補助判断用の容量がある場合だけ別のread-only分類呼出を行う。追加呼出不可なら入力を保存して採用barrierを立て、実行中Qwenの結果を保留した上で、slot解放後に分類する。LFM受付は止めない。分類不能・期限超過は確認待ちとし、旧回答を勝手に採用しない。
- 相槌/同意と判断された入力ではbarrierを解除し、既存Qwenの結果を採用できる。条件追加は既存のdrain・revision処理へ進める。副作用unknownの作業は再実行しない。

モデル/Provider fingerprint単位のlive試験で意味的な誤判定が出た場合も、minimal_jsonを合格にせずplain_text_parallelを検証する。どちらも不合格なら完成と報告せず、原因と未達ケースを残す。

## 4. 既存Role Routingへの接続

### 4.1 入口・台帳・実行

`useAmbientVoiceSession`の確定文を専用受付IPC `receiveConversationUtterance` へ接続する。保存後にreceiptを返し、ASR queueを解放する。LFM/Qwenの完了までIPCをawaitさせない。`useConversationTurn.submitPrompt`へ戻してQwenを起動する経路は使わない。

`rr_inputs`のroot_id nullableを利用し、依頼になる前の発言も既存conversation_messagesの参照とdigestで受理する。inputId/sourceIdで冪等化し、別payloadでの再送はconflict。coordinatorの入力処理は外部推論をawaitせず、別futureの結果をイベントとして受け取る。

依頼成立時は `requestReasoning` が受理した複数のinput参照と条件をrootへ束縛し、既存executorのpermitを通じてreasoner stepを開始する。LFMの生成した要約を新たなユーザー指示にしない。本文の別台帳への複製は避け、既存context組立で原文と来歴を保持する。受付sessionの世代とroot revisionを分離する。

frontendを毎回のreasoner recipeの前段に置く直列実行をやめる。会話frontendはsessionに属する独立slot、reasoner recipeは思考taskに属する。root成立前のfrontend判断はrr_inputsを参照する受付記録として既存writerに保存し、偽の推論rootを作らない。root必須のrr_steps/rr_decisionsへ無理に格納せず、必要な関連列・受付eventの加算migrationをVC01で確定する。

### 4.2 接続と容量

`roles.frontend`と`roles.reasoner`からactorを解決する。Harnessのprofile/provider capabilityを解決するadapterを既存Role Routingに追加し、frontend=backchannel、reasoner=llmの対応と実model fingerprintを検証する。動的な期限付きURL/tokenを固定Provider設定へコピーしない。credentialはRustの既存session ownerだけで保持・更新・解放する。

既存provider/codex_sdk transportの意味を変更せず、Harness役割bindingの明示的な設定型、validation、migration、再接続を追加する。default profileのLLMをLFMと推定しない。未登録frontend、モデル不一致、claim失敗、容量不足を別コードにする。

同一GPUでも、広告された実容量とlive計測が並走を保証する場合だけ並行する。resourceGroup名を変えて排他制約を回避しない。並走できない配置を「LFM常時応対可」と表示しない。Qwenがslotを占有してもLFM応対が止まらないProvider配置を製品gateとする。常駐準備時間・LFM待機時間・Qwen待機時間を別々に測る。

### 4.3 発話の単一owner

`role_routing/speech_queue.rs`と`speech_repository.rs`に実際のTTS driverを接続する。`preemptAcknowledgementForFinalAnswer` は最終回答の採用transaction後、次の順に処理する。

1. conversation speechEpochを進め、未再生のLFM発話を破棄する。
2. 再生中のLFM音声へ停止要求を出し、実playerの停止を確認する。確認期限は500msを初期設計値とし、達成可否をliveで測定する。
3. 停止確認後にQwenの最終回答を原文で再生する。LFMに再要約させない。
4. 合成完了時と再生直前の両方でepochを検査し、遅着LFM応答・音声を棄却する。停止未確認なら二重再生せず、確定した画面回答を保持して音声障害を表示する。

Qwen回答中も入力受付は可能にするが、LFM音声は重ねない。AEC・自声の再認識防止を検証し、ユーザーの明示割り込みは既存barge-in方針に従う。遅いSpeechEndedは別speechIdの所有権を解放しない。timerによるack/progress生成は廃止する。

## 5. 実装カードと順序

全カードは**未完了**。下表は主要変更先であり、一度に全ファイルを編集する指示ではない。5ファイルを超える単位はa/bに分割する。`rr/`は`src-tauri/src/role_routing/`。新規名は責務名であり、同責務の既存コードがあれば再利用する。

| 順/ID | 変更先・作業 | 依存と完了条件 |
| --- | --- | --- |
| VC00 配線監査 | `useAmbientVoiceSession.ts`、`useConversationTurn.ts`、`providers/larm_voice/frontdesk*.rs`と`runtime/`の入口を追跡 | 現在のdirty差分・呼出図・設定・実バイナリを保存。二重入口と独立台帳を特定。無関係な変更を戻さない |
| VC01 受付契約 | `rr/contracts.rs`、`schema.rs`、`ipc.rs`、生成IPC型 | root未成立の受付、input範囲、session generation、冪等receipt、設定mode、migrationの往復試験。既存データ保持 |
| VC02 接続設定 | `rr/adapters/`、設定validation/UI、既存`providers/larm_voice/mod.rs` | LFM/Qwenの実binding・lease・capacity確認。enabledとreadyを分離。未設定/期限切れを再現可能 |
| VC03 LFM応対 | `rr/`のfrontend adapter、`contexts/`、parser tests | say/thinkのみ、無発言時呼出0、平文降格・同入力重複0。構文と判断の双方を検証 |
| VC04 非同期統合 | `rr/coordinator.rs`、`driver.rs`、`executor.rs`、`recipe.rs`と必要なrepository helper | VC01〜03。LFM常駐slotとreasoner task分離、requestReasoning、追加条件/同意/別依頼、Qwen補助分類、barrier・予算・副作用保護。再帰的execute_turn禁止 |
| VC05 発話統合 | `rr/speech_queue.rs`、`speech_repository.rs`、TTS driverと`runtime/event_hub.rs` | VC04。停止確認・epoch・遅着破棄・最終優先。今回追加の別speech_priority ownerを統合し二重所有0 |
| VC06 UI入口/監視 | `useAmbientVoiceSession.ts`、`useRoleRouting.ts`、`roleRoutingStore.ts`、監査モニター | VC04〜05。receipt時解放、DB snapshot復元、LFM/Qwen別表示、障害理由。旧LFM専用IPC/台帳への新規書込0 |
| VC07 offline縦通し | `tests/`、Rust実writer/coordinatorのfixture | VC01〜06。下記VT全件を通常の製品入口から実行。pure reducer/HTTP単独で代用しない |
| VC08 実機受入 | `spec/evidence/role-routing/`、通常ビルド・起動手順 | VC07。実ASR/LFM/Qwen/TTS、反復・再起動・連続会話。未達は部分、完成申告不可 |

VC00で今回追加した独立`lfm_voice_utterances`等の使用状況を確認する。データがあれば既存rr_inputsへの移行・参照対応を先に検証し、無条件DROPしない。使えるparser/診断は低層部品として再利用し、Role Routingを迂回するdispatchやspeech ownerは残さない。名称の`delegate`はこの音声経路では`request_reasoning`へ改め、上位モデル間の委任と区別する。

ロードマップE00〜E11の必要な採用・permit安全条件とE14等の必要なtool安全条件はVC04までに満たす。依存が未達ならその実装を先に行う。音声基本受入をSol/Astra・review/specialist・夜間学習の完成待ちにはしない。

## 6. 受入試験（VT）

| ID | 入力/故障/順序 | 必須assert |
| --- | --- | --- |
| VT01 | 発話後1,499ms/1,500ms無音、途中で再発話 | 1,499で送信0、1,500でLFM受付1。無音だけでQwen起動0 |
| VT02 | 挨拶→「旅行を考えているんだけど」→具体的条件と依頼 | 前2件はLFM応対、Qwen task0。完成時task1、原文条件欠落0、LFMsession維持 |
| VT03 | Qwen思考をbarrierで保留して追加の相槌/進捗確認 | LFM応答がQwen完了より先、Qwen cancel/restart0、根拠のない進捗表現0 |
| VT04 | 不正JSON/文字列boolean/余分キー/途中切れ/timeout/Schema非対応 | 壊れたJSON発話0、平文降格1、同入力受付1、tool/task二重実行0、reasonerを巻き添え停止しない |
| VT05 | 平文並走でQwen分類と既存回答が競合、容量1も注入 | 受付継続、barrier前後の採否が一意、未解決条件を含む旧回答発話0、同時推論数<=広告容量 |
| VT06 | LFM合成中/再生中/queue中にQwen最終採用 | 古いLFM発話破棄、停止確認後Qwen開始、同時音声<=1、最終回答1。遅いSpeechEndedで新owner解除0 |
| VT07 | 停止未確認、TTS失敗、回答採用DB失敗 | 前2件は画面回答保持し具体的音声障害。DB失敗なら回答採用/TTS開始0 |
| VT08 | 条件追加、取消、同input再送、tool成功後通信切断 | revision/target/operationKey照合。副作用最大1、unknownなら再実行0、同意だけでcancel0 |
| VT09 | 長い無言、Qwen回答後、UI再接続、アプリ再起動 | 無言中自発発話0、破棄相槌の再生0、再接続挨拶0、旧task自動再実行0、新発言にはLFM応対 |
| VT10 | frontend未登録、実model不一致、期限切れ、lease/capacity失敗 | enabledのままready=false/degradedを表示。失敗stage/actor/codeを一意に表示、黙った旧経路fallback0 |
| VT11 | 通常ASR入口→LFM→Qwen→許可tool→DB→実TTS→UI再読込 | 全input/root/step/speechがRole Routing記録に辿れる。旧start_turnへの迂回0、最終本文と保存・発話一致 |

### 実機の合格証跡

- 通常ビルド（cfg(test)ではない）と起動中アプリのbuild ID、policy version、実actor/model fingerprint、使用outputMode、Provider容量を記録する。tokenや実会話本文を診断ログへ出さない。
- 固定の日本語fixtureを最低20ケース、各3回実行する。挨拶、話の途中、具体的依頼、同意、条件追加、否定・引用、曖昧な依頼を含める。minimal_jsonの誤依頼/取りこぼし/不正形式は1件でも未達とし、plain_text_parallelで再評価する。これは初期受入標本であり一般的な完全性の保証ではない。
- ASR確定からLFM応答準備までp50/p95、最初の可聴音までp50/p95、Qwen推論時間、回答採用からLFM停止確認まで、Qwen可聴開始までを分離する。初期目標は温間LFM応答準備p95<=2秒、可聴開始p95<=3秒、停止確認<=500ms。数値は設計目標であって実測値ではない。未達を設定deadline延長だけで合格にしない。
- 実マイクでVT02/03/06/09/11を反復し、最低30発言・10往復、Qwen思考中の追加発言、再起動後の再準備、自声の誤送信防止を確認する。音声を送れない環境では未検証とする。
- 証跡は`spec/evidence/role-routing/voice-integration-results.md`へ、test名・実行command・件数・失敗・build/policy・計測値・未検証を記録する。今回の計画更新で空の成功証跡を作らない。

## 7. 監視と復旧

表示は「受付保存」「LFM準備/応対」「Qwenへの思考依頼なし/待機/思考/失敗/回答済み」「発話担当/停止確認」「minimal_json/plain_text_parallel」。各状態にinputId、rootId（なければ未成立）、actor/provider fingerprint、最終更新時刻を紐付ける。古い失敗を現在の入力として表示しない。

失敗コードの例: `frontend-not-configured`、`frontend-output-invalid`、`frontend-response-timeout`、`reasoning-capacity-unavailable`、`speech-stop-unconfirmed`。設定不備・推論失敗・音声障害を一つのconfiguration-errorへ潰さない。復旧操作は接続再準備、出力mode変更、対象入力の明示再試行に限定し、未知副作用の自動再実行をしない。
