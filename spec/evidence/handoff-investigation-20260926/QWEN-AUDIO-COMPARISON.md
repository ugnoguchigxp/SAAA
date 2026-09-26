# qwen-audio-agentを参考にした音声・引継ぎ設計の比較

2026-09-26。比較対象は `/Users/y.noguchi/Code/qwen-audio-agent`、HEAD `2b70ebc4fffde4a7ce384bc04660346aab30e862`。調査前後とも同リポジトリの作業ツリーはclean。SAAAは本調査のREPORT.mdと同じ作業ツリーを参照。製品実装、設定、本番DBは変更していない。

## 判断

**参考にできる。特に「会話を受け持つFrontend」と「時間のかかる仕事」と「結果の通知」を別の寿命で管理する考え方が有用。ASR/LLM/TTS全体を移植する提案ではない。**

SAAAのQwen即応→必要時ornithという分担、独立した接続Owner、正式なclaimを待ってから生成する順序は合理的。今回弱いのは、接続・実行枠待ちを含む長時間処理を、利用者から見て一つの応答待ちにしている点である。比較先の非同期受付と通知設計は、この待機体験の改善に使える。

ただし、元runの直接原因であるLARMのsemantic probeの実行枠競合は別問題。比較先の設計を採用しても枠は増えない。性能限界との断定や、移植すれば最終回答が速くなるとの保証はできない。

## ソースで確認した経路

1. Frontendが仕事を依頼すると、`AgentTaskRuntime` がsession・turn・目的のfingerprintからsubmissionKeyを作り、`TaskOperations.submit` に渡す。TaskManagerは同じowner/scope/keyの仕事を再利用する。受付結果は `accepted` または `duplicate` とtask IDで返り、backend完了まで音声会話を拘束しない。
2. `TaskOperations` がbackendを実行し、イベントと最終結果をTaskManagerへ渡す。ownerごとにbackend laneを1本に制限する。これは投入の整理であり、別プロセスやLARM内の全要求を含む容量制御ではない。
3. 接続ごとの `SessionTaskCoordinator` がowner/sessionに対応するイベントを購読し、完了・失敗通知のclaimを取得する。音声窓口のcloseは通知claimを解放し、受理済みbackend仕事は取り消さない。明示的な取消は `TaskOperations.cancel` を通る。
4. `AnnouncementManager` が結果を音声Frontendに渡す。発話中や別音声待ちなら保留し、通知claimのlease更新、世代確認、再送を管理する。
5. 生成完了と音声再生を区別する。ただし**通知のdeliveredは全文再生完了を保証しない**。`realtime-presentation-runtime` は対象の再生開始でconfirmし、`dismissActive` もconfirmする。中断後に同じ通知を何度も読まないという方針であり、SAAAの「最後まで発声した」証拠と同一視してはいけない。

根拠:

- [受付・submissionKey](/Users/y.noguchi/Code/qwen-audio-agent/server/src/frontend/tools/agent-task-runtime.mjs:375)
- [backend実行・取消の入口](/Users/y.noguchi/Code/qwen-audio-agent/server/src/orchestration/task-operations.mjs:57)
- [重複受付の再利用](/Users/y.noguchi/Code/qwen-audio-agent/server/src/task/task-manager.mjs:464)
- [接続単位の通知所有権](/Users/y.noguchi/Code/qwen-audio-agent/server/src/orchestration/session-task-coordinator.mjs:1)
- [通知のconfirm・再送・lease](/Users/y.noguchi/Code/qwen-audio-agent/server/src/voice/announcement/announcement-manager.mjs:167)
- [再生開始時のconfirm](/Users/y.noguchi/Code/qwen-audio-agent/server/src/voice/realtime-presentation-runtime.mjs:566)

ここでいうnotification claimは通知権の確保であり、LARMのAgent Connection claimとは別物。両方の所有権と期限を混同しない。

## 領域別の比較と採用判断

| 領域 | 比較先で確認したこと | SAAAへの判断 |
|---|---|---|
| ロールの受渡し | Frontendからbackendへ仕事を受け渡してもFrontendは会話を続ける。受付・処理・通知が分離している | 最も参考になる。短い返答は今の経路を維持し、長時間処理だけ継続可能な仕事として扱う案を検討。新規TaskManagerを丸ごと足す前に既存run/root/stepへ対応づける |
| ASR入力相関 | provider item IDをturnへ対応づけ、古い・重複・invalidなtranscriptを除外。確定入力をjournalと表示に送る | SAAAにもsession/utterance/revision、重複排除付きVoiceFinalDeliveryQueueがある。全面置換より、遅着・割込み・再接続の試験条件を借りる |
| ASR精度 | realtime providerが認識結果を返す。s2s adapterではSTT本体を外部サービスが所有する | 「寿限無」が「発生してください」になった認識誤りを、このリポジトリだけで直せる証拠はない。元音声、区間切り、partial/final、抑制記録を別途調べる |
| LLM待ち | 受付の即時応答、backend進捗、完了通知を独立管理 | 接続確保前の「受け付けました」と生成中を区別する。ただしacceptedは実行資源確保済みではない |
| TTS/再生 | 発話・未完応答・queued audioをAnnouncementWindowで調停し、結果通知を再送管理 | SAAAのspeech_deliveriesと既存再生処理を活かす。回答保存、音声生成、再生開始、再生完了、取消を区別し、通知再送がLLM再実行にならないようにする |
| 割込み | speech開始で再生clearとFrontend cancel。受理済みbackend仕事は継続 | 「読み上げを止める」「新しい話を始める」「仕事そのものを取り消す」を区別する参考になる。ただし現行の明示停止を黙ってバックグラウンド継続へ変更しない |
| 再接続・所有権 | 一つの接続試行の共有、古い接続のイベント無効化、backoff、通知claimの解放 | SAAAにもOwner/OnceCellと世代管理がある。二重Ownerを導入せず、同じOwnerに絶対期限・監査を集約する |
| 状態通知 | backend MESSAGE由来の進捗をまとめ、通常60秒間隔、静かな境界を選んで通知 | 「考えています」の反復より実際の状態を示す考え方を採用。ただし比較先の進捗機構はLARM接続を監視しないので、そのままでは今回のprobe待ちに何も出ない |

ASRの実コードは [realtime-input-runtime](/Users/y.noguchi/Code/qwen-audio-agent/server/src/voice/realtime-input-runtime.mjs:107)、[turn-correlation](/Users/y.noguchi/Code/qwen-audio-agent/server/src/voice/turn-correlation.mjs)。音声調停は [AnnouncementWindow](/Users/y.noguchi/Code/qwen-audio-agent/server/src/voice/announcement/announcement-window.mjs)、進捗通知は [ProgressAnnouncementManager](/Users/y.noguchi/Code/qwen-audio-agent/server/src/voice/announcement/progress-announcement-manager.mjs:6)。

SAAAの既存対応物は [voiceAsrProjection](/Users/y.noguchi/Code/SAAA/src/features/voice/voiceAsrProjection.ts)、[VoiceFinalDeliveryQueue](/Users/y.noguchi/Code/SAAA/src/features/voice/voiceFinalDeliveryQueue.ts)、[音声delivery記録](/Users/y.noguchi/Code/SAAA/src-tauri/src/voice/streaming_tts/runtime/streaming_speech_runtime.rs:330)、[再生処理後の終端更新](/Users/y.noguchi/Code/SAAA/src-tauri/src/voice/streaming_tts/runtime/streaming_speech_runtime.rs:480)。VoiceFinalDeliveryQueueは**ユーザーのASR確定入力**の配送であり、最終回答TTSのキューではない。

## そのまま取り入れない点

- **音声モデル構成が違う。** [s2s adapter](/Users/y.noguchi/Code/qwen-audio-agent/server/src/voice/providers/s2s.mjs:25) は外部speech-to-speechのRealtime契約を記述する薄い層。VAD/STT/LLM/TTSモデルは外部プロセスが所有する。SAAAの個別ASR、Qwen、ornith、system TTSの実装代替ではない。既定のQwen Audio方式とSAAAのQwen判定器も同一構成ではない。
- **全待機に短い期限があるわけではない。** s2sにはresponse開始60秒設定があるが、TaskManagerのhard timeout watchdogは確認した箇所ではscheduled_task向け。通常仕事の受付から最終通知までの絶対期限を保証する手本とは判断できない。[該当条件](/Users/y.noguchi/Code/qwen-audio-agent/server/src/task/task-manager.mjs:755)
- **容量不足を静かに再試行する方針もある。** provider sessionはcapacity_busyの一般エラー通知を抑制する。SAAAがこれを無条件に移植すると無応答問題を残す。状態表示とユーザー向け期限は別に必要。[該当処理](/Users/y.noguchi/Code/qwen-audio-agent/server/src/voice/realtime-provider-session.mjs:280)
- **通知済みと聞き終えたことは異なる。** 前述のconfirm条件をそのままspeech_deliveries.completedへ対応づけない。再生開始後の中断は別状態として扱う設計が必要。
- 接続先のhealthやBackendPort.startがあることを、LARMで実行環境をclaim済みという証拠にはしない。SAAAのcreate→poll→claim→provider request→releaseを保持する。

## SAAAの改善案・優先順位（未実装）

**P0: 長い無応答を確実に終える。** 現行Qwen即応を保ち、接続作成・readiness・claim・生成queue・出力を別状態として監査/表示する。root絶対期限を全awaitと再試行へ渡す。ユーザー向け待機上限で短い理由説明を保存・表示・発声し、取消、期限、capacity失敗を区別する。説明文の生成自体を待機中のornithに依存させない。閾値の具体値は実測とUXで決め、比較先の60秒をそのまま採らない。

**P1: 音声窓口と長時間仕事の寿命を分ける。** 比較先の受付・仕事・通知の分離を既存run/root/stepへ適用する。会話を継続している間の状態質問、結果通知、明示取消を同じ仕事IDで処理する。短い通常回答まで全て永続非同期仕事に変える必要はない。後から回答する場合は、元の失効runへ書き戻すのではなく、元仕事との関連を保持した配送として定義する。

**P1: 事前確保と実行枠対策を組み合わせる。** 音声セッションの開始後に独立OwnerでLARM確保を先行させ、Qwenの処理はawaitさせない。不要時のrelease、取消中に到着したcreate/claim応答、世代交代、cleanup失敗を同じOwnerで処理する。probe公平性とclaim後queueの上限はLARM側で別途解決する。先行確保がQwenを圧迫しないか実測が必要。

**P2: 配送の契約を統一する。** 既存ASR入力キュー、run結果、speech_deliveriesのID対応と終端を揃える。音声通知の再試行は回答の再生成から切り離す。再接続後の再送、古いイベント、二重受付の抑止を比較先の試験例から補強する。

## 検証したこと・未確認事項

Node v24.11.1で以下7ファイルを実行し、**69件成功、0件失敗**。これは状態遷移・競合の単体試験であり、実音声やLARMの正常E2Eの証明ではない。

```text
node --test server/test/session-task-coordinator.test.mjs \
  server/test/announcement-window.test.mjs \
  server/test/realtime-input-runtime.test.mjs \
  server/test/input-arbitration.test.mjs \
  server/test/turn-correlation.test.mjs \
  server/test/progress-announcement-manager.test.mjs \
  server/test/announcement-manager.test.mjs
```

最初に試したrealtime-provider-sessionとrealtime-session-runtimeの2ファイルは、依存ws/zodが未導入のためロード時に失敗した。機能不良とは判定しない。比較先への依存インストールやサービス起動は行っていない。Coordinatorの15件はこの最初の実行でも成功し、69件の集計に含まれるため二重加算しない。

ログは本ディレクトリの `qwen-audio-core-tests.log` と `qwen-audio-initial-tests.log` に保存。実機の認識精度、初回音声遅延、最終回答時間、再生割込み品質は未測定。比較先の正常音声E2E、LARM連携、再起動を跨ぐ仕事継続も今回検証していない。元調査の実LARM試験で最終回答成功を確認できなかったという結論は変わらない。

実装する場合の優先回帰項目:

1. ornith未確保/枠busyでもQwen初回応答が遅れず、ユーザー期限内に状況説明が届く。
2. 同じturnの重複handoff、再接続、遅着claimで仕事/接続/生成が二重にならない。
3. 発話割込みと明示取消の意味が混ざらず、取消後の進捗・結果が再び発声されない。
4. 回答保存済み→再生失敗→再配送でLLMを再実行しない。再生開始後中断を全文再生成功と数えない。
5. 古いASR final、重複final、再生中の自己音声、ASR失敗、空の認識結果を別々に扱う。
6. 実LARMと隔離DBでcold/ready/busyを通し、実要求→最終回答保存→UI→TTS完了を検証する。Qwenのp50/p95も比較する。

凍結対象を変更する場合はAGENTS.md指定のASR/初回応答回帰試験と理由付きfreeze更新が必要。今回は調査文書のみの変更。比較先LICENSEはApache-2.0表記を確認したが、コード転載は今回行っていない。

運用ツールの会話累計: initial_instructions 1回、context_compile 3回、compile_eval 3回（今回の評価保存済み）。
