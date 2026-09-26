# SAAA：独立した回答Runtimeの実装計画 v2

2026-09-26／計画のみ。今回、製品実装・設定変更・Provider要求は行わない。

体験の正本は[5 Providerコンセプト](saaa-jarvis-five-provider-concept.md)。構造は[再構築提案](saaa-jarvis-response-runtime-rebuild-proposal.md)、旧経路の撤去は[削除計画](saaa-jarvis-response-runtime-deletion-plan.md)に従う。本書は**削除後に必要な実装と、削除を可能にする最小の後継実装**を定める。既存Runtimeへ機能を足す旧implementation-plan-v1は、新しい回答経路の実装手順として使わない。

追加指示どおり、Memory・World Modelとの会話接続は外す。embeddingは利用可能な資源として残し、最初の回答処理では呼ばない。Tool実行、長期記憶、状況推定の再接続は本計画の最初の完了条件に含めない。

最初の実装I0・I1は[詳細設計書](saaa-jarvis-response-runtime-i0-i1-detail-design.md)を実装単位の正本とする。同時入力1件・一句ずつ合成/再生・自動再試行0で始め、ASR/jobとそのキュー・再配送はI2/I3で追加する。本書の全体契約を、初回から空の実行器や未使用テーブルとして作らない。

## 1. 読取り時点と削除作業との接続

読取り時点のHEADは `a809d94981bcf729f4c985e584ee4ad0011edeb1`、ブランチは `codex/response-runtime-rebuild`。[削除実装の基準記録](../evidence/response-runtime-deletion-20260926/implementation-baseline.md)が追加されていた。これは進行中の作業のスナップショットであり、削除作業全体の最新状態を保証しない。本書作成ではそのファイルや削除側の製品コードを変更しない。

作成終了時の再確認では、旧conversation_turn、LFM受付、ASR session管理、TTS runtime等の削除差分が進行していた。本書で削除済みの旧経路の復元を要求しない。削除後の通常会話が一時的に未提供なら、その状態を明示したままI0〜I3で新経路を構築する。削除完了・build成功はこの観測だけでは判定しない。

基準記録には、freeze成功、既存sizeチェック失敗、新core未作成、D1〜D3が必要とある。本書でテストを再実行した結果ではない。したがって「削除が全て終わるまで実装しない」「新実装があるまで削除計画から進めない」という待ち合いにせず、次の受渡しにする。

| 削除側の成果 | 実装側で使うもの | 本計画との対応 |
|---|---|---|
| D1：資源・音声I/Oの切出し | 設定読取り、正式なLARM接続、役割別handle、単一Audio I/O所有者 | I0で受取り。既に作られた同責務の実装を二重に新設しない |
| D2：旧経路・Memory/Worldからの隔離 | 新Sessionの排他的な試験入口、隔離DB | I0〜I1で成立させる |
| D3：最小の実音声経路 | Qwen単独とornith引継ぎの実機完走 | I2〜I4で証拠を揃える |
| D4〜D5：切替・撤去 | 通常会話の入口一本化と共有回帰 | I5と同じ変更単位で実施 |

作業開始時に削除側のHEAD/差分と境界名を再照合する。旧ファイルの存在を前提に再導入せず、残った機能の契約を利用する。別のCodexタスクへのメッセージ送信はこの計画に含めない。

上表は削除計画との対応であり、削除が先行した場合に旧実装を戻すための順序制約ではない。I0では削除後の残存参照・初期化・IPCを確認し、新hostを接続できるbuild境界を受け取る。旧入口が残っていなければI5は切替ではなく、新しい通常入口の有効化と残存物の確認になる。

## 2. 最初に完成させるもの

次の二経路を実マイク・実Provider・実音声出力で繰り返し動かす。

```text
マイク → Listening/ASR → Frontdesk/Qwen ── 短い回答 ──→ Speaking/TTS → スピーカー
                                   └─ 仕事を保存 → Reasoning/ornith ──┘
```

Qwenは仕事を保存した後、ornith完了を待たず次の入力へ戻る。ornithの回答をQwenで言い換えない。TTSは両方の公開本文を共通の合成worker・playerで扱う。

初回の範囲:

- 会話は一つ。ASRは確定発話を入力にし、partialは表示・観測する。
- Qwenは短い直接回答・確認質問・思考依頼を扱う。音声による取消や既存仕事の条件変更は未実装と明示する。
- ornithは実行1件、待機1件の小さいFIFO。新たな受付枠がなければ受付不能を返す。
- Qwenとornithはそれぞれ一要求で回答する。Toolループ、採用審査、追加の言い換え推論を挟まない。
- 最初から句読点でTTSへ送り、全文完成を待たない。明示的なUI停止、期限、重複・遅着排除は実装する。
- 本人確認・自声除去は保護を外さない。未成立ならコンセプトの制限付きモードを表示し、音声割込みの完成とは扱わない。

初回の実装完了と、コンセプト全体の完成を分ける。1.5秒更新、途中訂正、複雑な停止意図、pause/resume、再起動後の仕事継続は後段で追加する。

### 2.1 qwen-audio-agentを制御構造の基準にする

ユーザーが安定稼働を確認している `/Users/y.noguchi/Code/qwen-audio-agent` を、受付・仕事・結果配送・接続の寿命を分ける設計の基準にする。参照HEADは `2b70ebc4fffde4a7ce384bc04660346aab30e862`。以前の[比較調査](../evidence/handoff-investigation-20260926/QWEN-AUDIO-COMPARISON.md)は実施済みだが、本計画の初稿では採用する仕組みと検証の対応が不足していた。以下を新実装の必須条件にする。比較調査の「既存run/rootへ足す」という当時の提案は、現在の削除・再構築方針には適用しない。

**四つのドメインを独自に設計し直す範囲を絞り、参考先の状態所有者・イベント順序・競合時の振る舞いを先に移す。** ASR/LLM/TTSの実Provider呼出しはadapterに置き換える。JavaScriptの全機能やRealtime通信形式をそのまま持ち込む必要はない。

| 参考先の実装 | SAAAでの所有者・採用する動作 | 対応する参考先テスト／導入段階 |
|---|---|---|
| [TaskOperations](/Users/y.noguchi/Code/qwen-audio-agent/server/src/orchestration/task-operations.mjs)、[TaskManager](/Users/y.noguchi/Code/qwen-audio-agent/server/src/task/task-manager.mjs) | Reasoningが仕事の状態を所有。Frontdeskは重複検証付きの受付receiptを得て応対へ戻る。仕事完了をawaitしない | [task-operations.test](/Users/y.noguchi/Code/qwen-audio-agent/server/test/task-operations.test.mjs:73)：同じownerのlane・重複排除・task identity。I0で契約、I3で実行 |
| [SessionTaskCoordinator](/Users/y.noguchi/Code/qwen-audio-agent/server/src/orchestration/session-task-coordinator.mjs) | hostの接続ごとの購読者が、仕事を観測してSpeakingへ配送する。購読者のcloseで仕事を取り消さない。別セッションへ誤配信しない | [session-task-coordinator.test](/Users/y.noguchi/Code/qwen-audio-agent/server/test/session-task-coordinator.test.mjs:235)：切断で通知claim解放、再接続で仕事再実行0。I0・I3 |
| [RealtimeInputRuntime](/Users/y.noguchi/Code/qwen-audio-agent/server/src/voice/realtime-input-runtime.mjs)、[TurnCorrelation](/Users/y.noguchi/Code/qwen-audio-agent/server/src/voice/turn-correlation.mjs) | Listeningが発話ID・版を所有。遅着finalを元発話へ対応づけ、重複・失効入力を排除。音声停止と仕事取消を分ける | [turn-correlation.test](/Users/y.noguchi/Code/qwen-audio-agent/server/test/turn-correlation.test.mjs)：遅着・invalidの維持・手動入力優先。I2 |
| [AnnouncementWindow](/Users/y.noguchi/Code/qwen-audio-agent/server/src/voice/announcement/announcement-window.mjs) | Speakingが発声許可を所有。ユーザー発話中・応答処理中・音声待ちを区別。LLM生成完了だけで音声枠を解放しない | [announcement-window.test](/Users/y.noguchi/Code/qwen-audio-agent/server/test/announcement-window.test.mjs)、[realtime-presentation-runtime.test](/Users/y.noguchi/Code/qwen-audio-agent/server/test/realtime-presentation-runtime.test.mjs)：応答・本文・再生終了の対応。I1 |
| [AnnouncementManager](/Users/y.noguchi/Code/qwen-audio-agent/server/src/voice/announcement/announcement-manager.mjs) | 結果保存と配送を分離。配送所有者・期限・世代を管理し、接続交代で未配送を戻す。再配送は保存済み本文を使う | [announcement-manager.test](/Users/y.noguchi/Code/qwen-audio-agent/server/test/announcement-manager.test.mjs:274)：生成済み≠通知済み、再生中の重複禁止、遅着・所有者交代・有限再試行。I1・I3 |
| [RealtimeProviderSession](/Users/y.noguchi/Code/qwen-audio-agent/server/src/voice/realtime-provider-session.mjs) | Resourcesが接続試行を一つに集約。複数利用者が同じ準備結果を待ち、旧接続のcallbackで新接続を壊さない | [realtime-provider-session.test](/Users/y.noguchi/Code/qwen-audio-agent/server/test/realtime-provider-session.test.mjs:128)：旧イベント無効、同一接続試行共有、有界buffer。I0、実adapter検証I4 |
| [RealtimeResponseSlot](/Users/y.noguchi/Code/qwen-audio-agent/server/src/voice/realtime-response-slot.mjs) | adapterが遠隔実行中・取消中・停止確認を区別。ローカルtimeoutだけで遠隔枠が空いたと判断しない | [realtime-response-collision.test](/Users/y.noguchi/Code/qwen-audio-agent/server/test/realtime-response-collision.test.mjs:155)：取消確認前の次要求0、未確認接続の隔離。I1・I4 |
| [RealtimeSessionRuntime](/Users/y.noguchi/Code/qwen-audio-agent/server/src/voice/realtime-session-runtime.mjs) | hostは上記を結線する。ミュート・接続断・読み上げ停止は仕事取消とは別。Qwen応対とornith仕事を同じ待機処理にしない | [realtime-session-runtime.test](/Users/y.noguchi/Code/qwen-audio-agent/server/test/realtime-session-runtime.test.mjs:130)：非同期受付後の会話・割込み・close、遅着からの仕事追加0。I3 |

この表の役割を一つのSession reducerへ再集約しない。Sessionは配送と接続状態の調整に留め、仕事の正本はReasoning、発話の正本はSpeaking、資源所有権はResourcesに置く。DBは各状態の保存先であり、UIや別workerに第二の正本を作らない。

### 2.2 参考先との差分を限定する

- **音声モデル境界**：参考先のRealtime音声経路に対し、SAAAはASR・Qwen・ornith・TTSを独立した資源として使う。入力相関と発声調停の契約を保ち、モデル呼出しだけをadapterへ分ける。
- **結果の発声**：参考先は結果を音声Frontendへ注入する。SAAAはコンセプトどおりornithの公開本文を共通Speakingへ直接渡す。配送のためにQwenで再生成しない。
- **停止・配信済みの意味**：参考先の通知confirmは再生開始やdismissでも成立し、全文再生済みを意味しない。SAAAでは配送の消込、再生開始、全文再生終了、中断を別記録にする。中断した結果を自動で冒頭から読み直さない。保持位置からのpause/resumeはB2で加える。
- **LARM**：接続・再接続の単一所有者を採用し、その内部を正式なcreate→poll→claim→provider request→releaseへ接続する。通知を届ける権利のclaimと、実行資源を確保するLARM claimは別ID・別寿命。遠隔停止が不明でも共有Agent Connection全体を破棄せず、該当役割を利用不可として隔離し、他役割の所有権を保つ。
- **待機時間**：利用者への状況通知期限と、仕事を終了する絶対期限を分ける。受付から30秒で全仕事を一律失効させる初稿案は撤回する。準備待ちの間にもQwenが応対できることを実機で確認する。

差分が必要になった場合は、元テストの条件、変更理由、SAAA側の期待値を先に記録する。実装担当が独自の再試行・状態所有者を追加して「参考にした」扱いにしない。直接コードを移植する場合はApache-2.0のライセンス・著作権表示とNOTICEの有無を確認し保持する。

## 3. 実装配置と依存規則

以下は配置案。削除作業で同責務の境界が作られた場合は、それを正本にして名前を合わせる。

```text
crates/saaa-conversation-core/
  src/contracts.rs       ID・入力・制御・公開出力・失敗理由
  src/session.rs         ドメイン間の配送・購読接続の調整
  src/listening.rs       発話受付と版・確定の整理
  src/frontdesk.rs       Qwen受付と制御レコードの扱い
  src/reasoning.rs       小さい仕事FIFO、期限、結果
  src/speaking.rs        句分割、発話順、音声の世代と終了

src-tauri/src/conversation_host/
  mod.rs / commands.rs   Sessionの生成・停止、IPC
  workers.rs             有限の実行枠で非同期I/Oを起動
  repository.rs          新状態と会話本文の保存
  adapters/              ASR、Qwen、ornith、TTS、player

src-tauri/src/provider_resources/  D1から受け取る資源境界
src/features/conversation/        薄い操作・表示
```

最初から細かいframeworkや多数のcrateには分けない。coreはHTTP、LARM token、Tauri、SQL、旧AppStateを知らない。各ドメインは参考先の状態遷移を担当し、Sessionはネットワークをawaitせず配送する。hostがI/Oを実行し、担当ドメインへ結果イベントを戻す。停止・期限イベントは通常入力のFIFOを待たせない。汎用effect frameworkの新設を目的にしない。

ドメインの状態遷移の正本はcore、永続化の手段はrepository、外部I/Oはadapter。UIはsnapshotとイベントを表示し、独自の仕事キューや音声再開判断を持たない。監査subscriberから生成・再生を開始しない。

禁止する依存:

- 新core/host → 旧 `execute_conversation_turn`、旧Role Routing/Butlerの実行器、旧UI submitPrompt。
- 新会話 → Memory/Worldの取得・生成許可・scope解決・抽出worker・embedding検索。
- Qwen/ornith adapter → TTS/playerの直接呼出し。
- 一件のjob取消 → 他のjobやASRが利用中の共有Agent Connectionのclose。

旧HTTP client、SSE/JSON parser、音声decode等の再利用は、その依存が旧会話制御へ戻らないものだけにする。コード移動だけで境界が成立したとは扱わない。

### 3.1 ドメインの責任と状態の所有者

**Listening・Frontdesk・Reasoning・Speakingを四つの会話ドメインとし、Provider Resourcesを独立した資源管理層にする。** ドメイン名は機能を表し、Qwen・ornith等の製品名はadapterと設定に閉じ込める。各ドメインは公開契約から独立に実装・検証できるようにする。初期は同一process内のRust moduleで構成し、複雑化したドメインの内部だけを分割できる構造にする。

| 境界 | 所有する判断・状態 | 受け取るもの／返すもの | 境界外の責任 |
|---|---|---|---|
| Listening：聞き取り | 発話の開始・区切り、話者/自声判定の受理規則、発話ID・版、partial/finalと重複・遅着の整理 | 音声I/O・ASRの観測 → 受理したInput、音声活動・障害イベント | 返答内容、思考依頼の分類、job取消、音声再生の開始判断 |
| Frontdesk：一次対応 | 短い回答・確認・委譲の判断、現在の応対、受付receiptを待つ状態、公開本文の許可 | Inputと必要最小限の状態要約 → 直接回答、SubmitJob要求。JobAccepted/Rejectedを受けて受付発話を確定 | jobの実行順・期限・再実行、LARM確保、TTS呼出し |
| Reasoning：仕事 | job受付、重複排除、FIFO、実行・期限・取消、結果の確定 | SubmitJob/CancelJobと実行結果 → 受付receipt、job状態、公開回答delta・終端 | Frontdeskの応対状態、マイク制御、読み上げの順番・再生操作 |
| Speaking：発声・結果配送 | 発声許可、句分割、発話順、同じjobの古い相槌の失効、配送所有者、再生位置・世代・停止 | 許可されたPublicDelta、入力の音声活動、停止要求、TTS/player結果 → 合成・再生の指示とSpeechEvent | 回答本文の再推論、依頼分類、jobの取消・再生成 |
| Provider Resources：資源管理 | 5 Providerの設定解決、LARM接続の準備・claim・利用権・release、接続世代、容量/障害状態 | 役割を指定した利用要求 → 型付きhandle、準備状態、失敗理由 | 会話内容、handoff判断、job成功判定、発声許可 |
| Session / host：結線 | ドメインの生成・購読、宛先配送、I/O実行、保存commitの結果返送、明示的な全体終了 | 公開command/eventを正しい所有者へ渡す | 上記ドメインの業務判断と状態を再実装すること |

Listeningは音声活動を通知し、Speakingが再生への影響を判断する。Frontdeskは仕事を要求し、Reasoningが受付を確定する。Reasoningは公開本文を渡し、Speakingが発声順を決める。この判断の所在をhostへ移さない。

Speaking内部では、結果の配送、句の合成、実再生を別の状態機械として扱う。結果配送の失敗で再推論せず、TTSの合成完了で再生成功にせず、再生停止でjobを取り消さない。物理マイク・スピーカーはD1のAudio I/Oが一つだけ所有し、ドメインはその入出力契約を使う。

### 3.2 ロジックが混ざらない依存規則

1. **各ドメインの状態は非公開**にし、公開command/eventと読み取り専用snapshotだけを渡す。兄弟ドメインの実装module・可変state・DBテーブルを直接参照しない。共通契約に巨大なAppStateや各ドメインの全状態を含めない。
2. **ドメインが要求するI/Oをportとして定義し、hostのadapterが実装する。** ASR認識、LLM生成、TTS合成、再生の通信形式とProvider固有エラーはadapterで変換する。adapterは依頼分類・仕事採否・発声順を判断しない。Resourcesの具体的なLARM型やtokenをcoreへ渡さない。
3. **Sessionは決まった宛先へ配送する。** `Input → Frontdesk`、`SubmitJob → Reasoning`、`JobAccepted → Frontdesk`、`PublicDelta → Speaking`等の結線に留める。本文、モデル名、例外文字列を読んで経路を変えない。永続化成功前の外部処理は、各ドメインが保存結果を受けて許可する。
4. **保存実装を共有しても判断は共有しない。** repositoryに入力・job・speech用の狭いAPIを設け、ドメインが決めた変更だけをcommitする。repositoryや監査subscriberに再試行・受付・生成・発声の判断を置かない。必要な原子性は§4.2の単位で確保する。
5. **待機と取消の範囲を分ける。** 各ドメインの有界キューと処理枠を持ち、LARM準備・ornith・TTS待ちの間もListening/Frontdeskのイベントを処理できるようにする。全会話共通のlockを外部I/Oのawait中に保持しない。期限・取消tokenは対象の仕事/要求/配送に属し、親の明示終了だけが全体へ伝播する。
6. **交換と拡張は境界内で行う。** ASRのモデル変更はListeningのadapter、Qwenの交代はFrontdeskのadapter、発声方式変更はSpeakingのadapterへ閉じる。Memory・World・Toolの後日接続はReasoningのportから始め、他ドメインの通常応対条件に追加しない。

通信経路は、hostが契約に従って配線する少数の明示的なcommand/eventとする。全イベントを全ドメインが監視する汎用busを導入しない。これにより、qwen-audio-agentの状態所有権を保ちながら、SAAAの独立したASR/LLM/TTSへ対応できる。

### 3.3 分離できたことの受入条件

I0で公開API・依存チェックを置き、I1〜I3で各ドメインの試験を追加する。ディレクトリ分割だけでは完了にしない。

- 各ドメインの状態遷移を、他ドメインの実装・実Provider・Tauri・本番DBなしで検証できる。
- 兄弟ドメイン内部へのimport、coreから具体的なProvider/SQL/UIへの依存、adapterから他ドメインへの直接呼出しを依存チェックで検出する。
- 実hostでornithの資源待ち・生成を保留しても、ASR入力受理とQwen応対が進む。TTSを故障させても入力・回答保存が進み、表示だけの回答として残る。これは単体試験だけでなくI3/I4の実機試験でも確認する。
- 同じInput契約を使うテキスト入力でListeningを外してもFrontdesk以降が動き、Speakingを外した試験でも受付・仕事・最終本文保存が成立する。
- 読み上げ停止、job取消、接続交代を別々に試し、それぞれの状態所有者だけが対象状態を変える。必要な波及は明示したイベントで追える。

既存のqwen-audio-agent対応試験（§2.1・§6.1）にこの境界試験を加える。ドメイン分離と参考先の安定した振る舞いの両方を、最初の実装の完了条件にする。

## 4. 最小契約を先に固定する

### 4.1 IDとイベント

| 契約 | 最低限の内容 | 規則 |
|---|---|---|
| `Input` | session ID、utterance ID、revision、partial/final、原文、音声時刻、本人確認結果 | 認識原文をQwenの要約で上書きしない。同じ発話版の再送は一度だけ処理 |
| `FrontdeskControl` | action、入力参照、発話種別 | 初回はreply/clarify/delegate。将来のpartial用waitは、受付済み仕事の代わりに使わない |
| `Job` | job ID、入力参照、原文/制約、状態、受付時の絶対期限 | 状態を移るたびに期限を延長しない。新規依頼だけFIFOへ追加 |
| `PublicDelta` | output/speech ID、入力/job参照、世代、連番、本文 | hostが発行した公開枠だけ通す。本文のタグで内部出力を公開へ変えない |
| `SpeechEvent` | speech ID、世代、句番号、開始/終了/停止/失敗 | 合成完了と実再生完了を区別。古い終了通知で新しい発話を解放しない |
| `ResourceEvent` | connection/role、準備状態、関連request ID、理由 | 資源状態とjob状態を混ぜない。秘密値は含めない |

Qwenの一要求を「完結した制御JSONレコード → 本文ストリーム」として扱う。複数行JSONと分割されたUTF-8/SSEを受けられるparserをadapterに置き、重複キー・後続制御の差替えを拒否する。モデル出力の意味的な正しさは別の固定入力試験で評価する。

delegateはjobの保存成功後に成立する。その間の本文は上限内で保留する。job保存失敗・満杯・古い版なら受付を称する本文を捨て、確認済みの短い定型説明へ進む。保存後にQwenの本文が途切れても、成立したjobを勝手に取り消さない。

ornithは初回から公開回答用・Toolなしの要求にする。Providerが返す内部reasoningは別フィールドとして除外する。経路が分離を保証できなければ契約不適合として止める。正しい公開回答を得るためだけにQwenへ戻したり二回生成したりしない。

### 4.2 状態・保存

Jobの基本遷移は `queued → waiting_resource → generating → succeeded`。任意の稼働状態から `failed / expired / cancel_requested` へ遷移する。取消要求と遠隔停止確認を区別し、確認不能なら診断に残す。`succeeded` は生成の正常終了と最終本文保存が両方成立した時だけ。

Speechは別に `queued → synthesizing/playing → played` を持ち、失敗・停止・失効を区別する。音声障害でjobの本文を消さず、job成功だけで音声を再生済みにしない。先行して読んだ後に生成が失敗した場合は残りを止め、途中までの本文/再生記録を残す。

接続・仕事・配送の寿命を次のように固定する。これはI0〜I3の要件であり、再起動復旧のB5まで延期しない。

| 出来事 | 仕事 | 配送・接続 |
|---|---|---|
| マイクmute、画面の再購読、音声接続断 | 受理済みjobを継続。新job/再生成を起こさない | 入力または配送を保留。接続交代で旧配送所有権を解放 |
| 読み上げ停止 | jobの取消は行わない | 対象speechを停止・消込。遅着音声は破棄。自動で全文再送しない |
| 対象jobの明示取消・実行期限 | 終端への遷移と遠隔取消を要求 | 対象の未再生音声を失効。他jobの接続をcloseしない |
| 同じ会話への再接続 | 既存jobの状態を購読 | 未再生・未失効の配送だけ取得。旧所有者からの完了通知を拒否 |
| アプリprocess終了・明示的な全会話終了 | 初回実装では継続保証せず中断を保存しcleanup | UIの一時切断と区別。次回起動で自動実行・自動発声しない |

配送は最小の `pending → claimed → settled` とowner/generation/leaseを持ち、settled理由をplayed/interrupted/failed/expired等で記録する。再生開始後の接続断など再生位置が不明なら、全文を自動再送せず本文と再生操作を示す。完全に未再生と確認できた配送だけを、有限回の再試行で同じ本文から回復する。再試行上限に達した配送が後続を塞がないよう、失敗として残して所有権を解放する。上限はI0で明示し、初期案は追加1回。user stop・期限後の再試行は0。

ストリーミング中と最終結果通知は同じoutput/speech IDを参照する。既に句を再生した後のjob完了で、全文を新しい通知として再投入しない。参考先の通知機構へSAAAの早期TTSを組み合わせる際の必須差分として検証する。

保存は一つのrepositoryに集約する。新規の入力receipt、job、speech、重要イベントの最小テーブルを追加し、確定したuser/assistant本文は既存会話履歴へ一度だけ保存する。テーブル名とDDLはI0で確定する。旧rr/Butler状態への二重書込みや、旧 `runtime_runs` の成功を偽装してUIを動かす方式は採らない。

原子的に行う単位:

1. 確定入力の重複検証・原文保存・受付記録。
2. delegate成立とjob作成、外部workerへ渡す受付イベントの保存。
3. 最終本文保存とjobの終端、UIへ配送すべき完了記録。

全tokenをDBへ同期保存してから合成する方式にはしない。本文ストリームはメモリ内で蓄積し、公開範囲・句の配送・重要な状態変化を記録する。再接続UIにはsnapshotを返し、失ったイベントの再要求でモデルを再実行しない。再起動後は未完了jobを中断として示し、自動再生成・自動再生しない。

履歴テーブルへの保存を、既存のMemory抽出workerが自動的に消費しないことも確認する。新会話には由来を付け、抽出予約やWorld更新の対象から外す。隔離hostはこれらのworkerを起動せず、通常入口へ切り替えるI5では他用途の処理を保ちながら新会話の対象外条件を検証する。直接のMemory呼出しが0でも、保存後の背景処理が動けば未接続条件の未達とする。

### 4.3 初回の期限と上限

次は**試作hostの初期値案**。保存済みユーザー設定へ書き込まない。I0で実効値と設定値の対応を記録し、変更した場合は試験結果にも残す。既存の安全上限は超えない。

| 項目 | 初期値案 | 到達時 |
|---|---|---|
| Qwen入力受付から制御・本文終了 | 8秒 | 要求取消、短い失敗表示。自動再生成しない |
| 本回答未到着の状態説明 | 受付10秒後に1回、同じ待機状態のままなら30秒後に追加1回 | 実際の待機理由と取消操作を表示。発声可能なら確認済み定型文を共通Speakingへ。jobは継続し、Qwen応対を塞がない |
| ornith job受付から最終本文保存 | 試験hostで明示した有限のjob budget。冷間試験の初期案は最大480秒、既存安全上限内で設定 | queue/資源/生成のどこで期限に達したか記録し、明示的な期限応答。無通知でこの時間を待たせない。受付時からの絶対期限を自動延長しない |
| 資源の先行準備 | 最大300秒（現在の接続上限を超えない） | 接続をfailedにしcleanup。jobとは別の所有者・期限。終了jobを準備完了で復活させない |
| 個別TTS要求 | 15秒と保存済み設定上限の小さい方 | その発話を音声失敗へ。本文は保持 |
| 発声可能になった発話の最初の可聴音待ち | 15秒 | 待機理由を表示し、その音声配送を終端。利用者が話していて発声保留の場合は別状態として計測 |
| 再生所有中、後続本文/音声が来ない時間 | 10秒 | 停止確認後に所有権を解放し、他の発話を永久に塞がない |

受付後の経過時間は発声保留中も表示する。発声保留・音声キューの総寿命にも有限の上限（初回120秒案）を持たせ、失効後は本文だけを残す。これらは品質目標ではなく無限待ち防止の上限。温間Qwenの可聴開始p95 2.5秒というコンセプトの目標は別に測り、8秒以内なら低遅延達成とはしない。

10/30秒の通知は無応答を放置しないための試作値で、定期相槌ループにはしない。準備段階の変化と利用者からの問い合わせには最新状態で答える。本回答が届いたら同じjobの古い状態説明音声を失効させる。480秒案は既知の64〜267秒のclaim待ちを試験するためのjob上限であり、性能改善の主張でも通常利用の待ち時間目標でもない。各Provider要求自体の上限を延長せず、製品値は冷間・温間の実測後に確定する。

「発声可能」は、その発話の順番が回り、本人の音声活動がなく、区切り条件を満たす状態を指す。音声順番待ちを計測から除外せず、可聴開始までの全体時間と上記の工程別時間の両方を記録する。

Qwenの未処理入力は実行中を含め4件、ornithは実行1件＋待機1件を初期上限にする。別発話のfinalを黙って上書きしない。PCM・未公開本文・未合成句・音声bufferは件数/byte/音声長の上限をI0で型付き設定として定義し、上限超過を明示する。既存の低水準I/O上限を流用する場合も値を証拠へ記録する。

## 5. 実装単位

### I0：資源境界・Session・隔離host

作業:

- D1の成果を受け取り、Provider資源層に「準備開始、状態購読、役割handle取得、利用終了、session終了」の境界を置く。
- `saaa-conversation-core` と薄いhost、repositoryの最小schema、相関IDとイベントを作る。旧AppState丸ごとの注入をしない。
- 保存済み設定を読取り、実効model/role/routeを表示する。Memory/World/embeddingの利用は配線しない。
- 同一セッションの所有者を一つにし、GUIの再描画で再接続・再生成しない。
- §2.1の参考先テストをSAAAの契約試験へ対応づける。仕事・接続・配送のOwnerとclose対象を先に固定し、その試験を通す最小実装から作る。
- テキストを投入できる小さい試験画面と停止操作を置く。過大な新UIを作らない。

決定的検証：単一Owner、commit失敗時の外部effect 0、同じ入力の重複、期限、取消中の遅着、UI購読再接続。参考先どおり接続試行を共有し、交代済み接続のcallbackは新状態を変更しない。既存DBの隔離コピーと空DBのmigration。

実接続検証：正式なcreate→poll→claimで返った役割とmodelを記録し、release/cleanupを確認する。カタログや直接モデルURLでは代用しない。5役割一括claimしかできない場合はその制約を確定し、資源未準備中を「Qwen応対可能」と表示しない。役割別確保APIを想像して実装しない。

完了物：起動できる隔離hostと資源状態表示、純粋core、最小保存。**会話完成とはまだ呼ばない。** 資源が利用不能なら以降の純粋実装は進められるが、live合格を付けない。

### I1：テキスト → 実Qwen → 実TTS

作業:

- 実Qwen adapterを1要求の制御レコード→本文へ接続する。
- Speakingに公開本文の `、。！？` 分割、正常終了時だけの末尾flush、合成1・player1、句番号/世代を実装する。
- UIには受付、応対中、発声中、終了/失敗を表示する。
- reply/clarifyを通す。delegateは「思考処理は未接続」と明示し、受付済みjobを装わない。

決定的検証：SSE/UTF-8境界、複数行/不正/差替えJSON、保存前本文の非公開、複数句と末尾、失敗時flush 0、停止後の音声0、同時合成/再生各1以下。参考先の「生成終了と再生終了の分離」「再生中の再配送0」「旧世代の遅着0」「取消未確認の枠で次要求0」を移す。

実機検証：固定テキストの挨拶・簡単な質問でQwen1要求、実TTSの可聴開始/終了、DBの本文を確認。長めの公開本文で、生成完了前に最初の句が合成されることを確認する。合成wav生成だけで再生合格にしない。

完了物：実際に声が返る一本目の経路。この段階で音声出力の問題を解消してからASRを接続する。

### I2：実ASR待受けを接続

作業:

- D1のAudio I/Oと選択済みASR adapter、既存話者登録を新Listeningへ接続する。
- partialは表示、本人として受理したfinalはI1と同じInput入口へ送る。ASR final専用の別会話Runtimeを作らない。
- マイクの取得、ASR処理、会話入力の受理を区別する。ユーザーの発話中に通常回答を再生しない。
- Qwen生成中もListeningを継続。TTS中の自声除去が未確認なら、明示した制限付きモードで未確認音声を受理しない。

決定的検証：session世代、PCM順序、二重final、遅着final、空認識、話者不明、停止、ASR障害。入力滞留/上限で原文が消えない。

実機検証：実マイクから挨拶・簡単な質問を各経路へ通し、原文→Qwen入力→本文→実再生を関連付ける。TTSだけで新規入力/仕事が発生しないことを確認する。自声除去未成立は未達として残す。

完了物：マイクからQwen単独で声が返る。ASR精度とQwen分類を別に採点する。

### I3：思考キューと実ornithを接続

作業:

- delegateのjob保存・受付応答・Reasoning workerをつなぐ。原文を保持し、Qwen本文を仕事の指示として再利用しない。
- ornithの資源待ちと生成は独立workerで進め、Frontdeskを占有しない。
- 公開回答を同じSpeakingへ直接送る。
- jobの継続と接続ごとの配送購読を分離する。ミュート・接続断の間も結果を保存し、同じ会話へ再接続してもornithを再実行しない。
- 同じjobのornith回答が最初の句へ達したら、そのjobの未再生受付音声を失効させる。再生中なら停止確認後に回答へ移す。別件のQwen回答は残す。
- 発話の所有権はspeech単位で維持し、Qwenの句とornithの句を交互に混ぜない。

決定的検証：受付保存前の「受け付けました」0、二重job/二重要求0、小さいFIFO、queue満杯、同じjobの受付音声だけの失効、期限・取消後の回答採用0。参考先の非同期receipt、切断時の配送claim解放・job維持、再接続時の再実行0、停止後の自動読み直し0を必須にする。

実機検証：自然な固定依頼で実Qwenがdelegateし、実ornithの最終回答保存と実TTS完了を確認する。固定依頼の一つに「寿限無の名前を省略せず発声して」を含める。期待本文/許容差は試験前に固定する。

さらに実ornith要求が進行中の時間帯に、別の挨拶を入力する。Qwenの要求と本文がornith完了前に進むことを確認する。音声の順番待ちとLLMの停止を混同しない。同時生成が資源側で成立しない場合はLARM/配置の課題として記録し、アプリ側のテストで覆い隠さない。

完了物：二つ目の実音声経路。Qwenの受付だけでjob成功としない。

### I4：冷間・失敗・停止の終端を固める

作業:

- 接続準備、semantic probe/capacity、claim、生成queue、生成、合成、再生の待機理由を区別する。
- 最初からある期限・取消を実adapterへ通し、すべての段階で終端記録と説明を残す。
- UI停止はモデル判断を待たず、音声停止とjob取消を別のcommandにする。
- 自動のモデル再試行は初回では0。手動再試行は新request/明示的な新実行として記録し、元jobの期限を延長しない。
- 接続再試行、保存済み本文の配送再試行、モデル再生成を別操作にする。接続だけの再試行には参考先の共有試行・backoff・世代確認を使い、資源準備期限内に制限する。jobや本文を再作成しない。

検証行列：create応答前、poll中、claim中、生成queue中、本文の途中、TTS要求中、再生中で停止/期限を入れる。cleanup責任・idempotency keyを失わず、共有接続を利用中の他役割を止めない。TTSが使えないときは画面に説明と本文を残す。

冷間準備が30秒を超える場合も、状況説明を届けたうえでjobの実行期限内なら継続する。利用者は別の会話や明示取消を行える。有限の実行期限・確定した接続失敗・利用者取消で終端させる。準備が後から成功しても失効jobの音声を勝手に出さない。温間の本回答成功、冷間からの本回答成功、明示的に終了した失敗を分けて集計する。説明を返せただけでは本回答成功にしない。

### I5：通常入口へ切替・旧本体撤去

I2〜I4の実機証拠が揃ったら、削除計画D4〜D5と合わせて通常の音声/テキスト入口を新Sessionへ切り替える。旧入力をdrainし、新旧同時所有と自動fallbackをなくす。旧実装の呼出しが必要になった場合は小さい不足契約を実装し、旧Runtimeを再導入しない。

検証：起動snapshotとprimary conversation、履歴・設定・話者登録、通常会話、coding等の残す機能、アプリ終了、再起動時の中断表示。削除作業で残した共有通知を新Speakingへ接続し、旧playerを起動しない。

完了物：通常入口で使える最小の新Runtime。以降は各ドメイン内部の機能拡張へ進む。

## 6. 段階ごとの検証と証拠

| 段階 | 必須の確認 | 証拠 |
|---|---|---|
| I0 | coreテスト、資源/保存/境界テスト、空DB・隔離コピー | 実効設定fingerprint、役割・接続phase、release、依存チェック |
| I1 | Qwen契約、Speaking回帰、実Qwen/TTS | request ID、最初の本文/句、TTS要求、実再生開始/終了、本文 |
| I2 | AGENTS.mdのASR回帰＋実マイク | 音声区切り、本人判定、partial/final、認識原文、誤入力数 |
| I3 | FIFO/受付/重複/並行応対＋実両LLM | job、実ornith request、Qwenの重なり、最終message、speech |
| I4 | 仮想時計による全段階失敗＋実冷間/停止 | stage別期限・取消、失効結果、cleanup結果、利用者に届いた説明 |
| I5 | 全体回帰、通常入口smoke、反復E2E | build/差分/設定、全試行、失敗内訳、旧経路呼出0 |

新coreのコマンドは作成後 `cargo test --manifest-path crates/saaa-conversation-core/Cargo.toml`。hostには独立したtest moduleを設け、例えば `cargo test --manifest-path src-tauri/Cargo.toml conversation_host` で実行できるようにする。これらは計画上の新コマンドで、現時点で存在/成功すると主張しない。

ASR変更時は `bun test tests/ambient-voice-session.test.tsx tests/voice-capture-races.test.ts tests/voice-asr-packet-sender.test.ts` と `cargo test --manifest-path src-tauri/Cargo.toml voice::streaming_asr`。初回応答変更時は `bun run quality:check` とmacOSの `bun run desktop:smoke`。IPC変更時は生成・binding整合を確認する。最後に `bun run check:local` を実行する。

削除でテストfilterが0件になったら未検証。実行対象を新入口へ移し、件数とケース対応を残す。既存size失敗等は開始時の基準と分け、基準値を一括緩和して通さない。

初回完了の反復条件案は、Qwen単独とornith引継ぎを各10回以上、全試行を記録すること。全件で期待した経路・本文保存・実再生終了が成立することを最低条件とする。期待と異なるQwen分類、欠落したASR、TTS失敗を無視して成功例だけ選ばない。修正後の再実行も元の失敗履歴を残す。

各10回は経路の反復確認であり、安定したp95の保証ではない。遅延は発話終了→最初の音、受付→本回答、Provider待ち、音声順番待ちを分け、失敗数を併記する。初期目標未達は未達のまま示す。コンセプト全受入の30発言・10往復、1.5秒更新、割込み・復旧の試験は後段で実施する。

live試験の証拠は隔離DB・診断ログへ保存し、tokenや私的な会話を通常の公開添付に含めない。テストのために保存済みProviderや話者設定をリセット・置換しない。

### 6.1 参考先を採用できたかの判定

§2.1の各行に対し、実装時に「参考元のテスト名 → SAAA側テスト名 → 結果 → 意図した差分」を残す。名称を似せただけでは採用完了としない。特に次の連続シナリオをI3の必須試験にする。

1. 依頼Aを受理し、ornithが資源待ちの間に挨拶BへQwenが応対する。同じAを再送してもjobは1件。
2. Aの途中でマイクmute・UI再接続・音声接続交代を行う。Aは継続し、Aのornith要求を再作成しない。
3. Aの結果が配送不能中に到着する。結果を保持し、復帰後に有効な所有者が一度配送する。古い所有者からの完了は新しい配送を消さない。
4. 再生開始後に「読み上げ停止」を操作する。job結果は残り、停止した本文を自動で冒頭から読み直さない。
5. 別の依頼Cを明示取消する。取消前の接続・生成・音声callbackが遅れても、Cを復活・発声させず、次の会話は進む。

まず実hostと模擬adapterでイベント順序を決定的に検証し、次に同じシナリオを正式なLARM接続・実マイク・実TTSで通す。実資源待ちとornith生成中の二条件でQwen応対を計測する。参考先のテスト成功はSAAAでの成功に代用しない。

**今回確認済みの範囲**：参考先の7ファイル・69件の単体試験は前回成功済み（[ログ](../evidence/handoff-investigation-20260926/qwen-audio-core-tests.log)）。今回、結果と再生の寿命を確認する `realtime-presentation-runtime.test.mjs` を追加実行し15件成功した。同時に実行した `task-operations.test.mjs` は依存 `zod` 未導入でロード失敗し、試験本体は未実行（[今回のログ](../evidence/handoff-investigation-20260926/qwen-audio-adoption-tests.txt)）。以前のprovider/session runtime試験も依存ws/zodでロード未完了。これらのソースと試験条件は確認したが、実行合格とは扱わない。参考先の設定・依存・ソースは変更していない。

ユーザーの安定稼働報告を設計選択の根拠とし、上記ソースと実行済み試験でその構造を確認した。今回こちらで参考先の実音声E2Eを再測定したものではない。SAAA側への移植・実機成功も未実施であり、上記はこれから実装する条件である。

## 7. 最小経路が動いた後の拡張順

| 次段階 | 拡張するドメイン | 完了条件 |
|---|---|---|
| B1 | Listening＋Frontdesk | 発話中1.5秒更新、区切り即時、後着final、同一発話の訂正。無更新生成0、重複job0 |
| B2 | Listening＋Speaking | 実スピーカー/マイクで自声除去、本人の重なり発話、pause/resume。保持位置から再開し句を読み直さない |
| B3 | Frontdesk＋Reasoning | 質問IDでの回答、仕事の条件変更、音声による停止、複数FIFO。宛先分類の評価を伴う |
| B4 | Reasoning | Tool、Context、Memory、World、embeddingを一つずつ接続。未接続時の単純会話を引き続き通す |
| B5 | 保存・復旧 | process再起動を跨ぐ仕事継続・配送復元。Tool・生成・再生を二重実行しない。同一process内の接続復旧・未配送の回復はI0〜I3で実装済みを条件とする |

本書のI0〜I5にB1〜B5の全要件を前倒ししない。公開本文の境界、ID/版、期限、取消という共通契約を保って拡張する。Memory・Worldを接続し直す判断は最小経路の完成後に行う。

## 8. 着手時の具体的な一単位

最初の実装依頼は **I0とI1まで**を一つの到達点にする。D1の資源境界を受け取り、新core/hostの最小骨格・保存・実Qwen・共通Speakingを接続し、固定テキストから実際に声が返るところまで進める。I0のinterfaceだけ、またはmockだけで完了にしない。

次の単位でI2、続いてI3へ進む。実Providerが利用できずlive検証が止まったら、失敗したstageと必要な資源側の対応を示す。別Providerへのすり替えや旧経路の復活で進捗を作らない。

今回の成果物はこの計画文書と参考先の追加試験ログ。SAAAの製品実装、回帰・実機検証は実行していない。

運用ツールの会話累計：initial_instructions 1回、context_compile 8回、compile_eval 8回（今回の評価を含む）。
