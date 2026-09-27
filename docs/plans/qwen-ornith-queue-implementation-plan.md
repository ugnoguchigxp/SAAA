# Qwen 2B・Ornith 会話キュー実装計画

状態: 実装前の計画。2026-09-27 の作業ツリーを確認して作成。既存の未コミット変更は本計画の対象外。

## 1. 目的と適用範囲

会話画面で、確定した ASR テキストと手入力を Qwen 2B の処理キューへ受理する。Qwen は自分で答えられる依頼には応答し、調査・深い思考・ツールが必要な依頼を Ornith の仕事キューへ登録する。Ornith の結果は Qwen の入力キューへ戻し、Qwen がユーザー向けの発話文を確定する。TTS に渡す本文は、原則として Qwen が確定した可視発言だけとする。

Queue 自体を独立したドメインとして実装する。キューの更新と各ワーカーの処理完了はスケジューラへの起床通知にする。通知そのものを仕事の正本にしない。スケジューラは SQLite の状態を読み、実行可能な仕事を原子的に取得する。アプリ再起動時にも同じ確認を行う。

最初の利用者は現在の `ConversationCheckPage` / `conversation_check` 経路とする。Queue ドメインはモデル名や会話画面に依存させない。通常会話の旧ランタイム、`role_routing`、`steward` の汎用キューをこの計画の中で全面移行しない。会話履歴、Provider 認証、LARM selector、ASR/TTS 接続設定は維持する。新しい外部キューサービスやライブラリは導入せず、既存の SQLite と Tokio を使う。

### 成功条件

1. ASR の確定発話または手入力が、LLM の稼働状況と無関係に一意の受付 ID で永続化される。画面を閉じても受理済み入力を失わない。
2. Qwen の `quick` は Ornith を呼ばず、`delegate` は一件の Ornith 仕事を登録する。Ornith の結果は元の受付 ID と結び付いて Qwen に一度だけ届く。
3. Ornith の本文が直接、ユーザー可視の回答または TTS 入力にならない。Qwen が意味を保って発話文を作り、根拠不足・失敗もそのまま伝える。
4. キュー更新とワーカー完了で仕事が進む。空キューで LLM を再起動しない。通知の欠落、アプリ再起動、同じ通知の重複でも停止・重複回答しない。
5. 新着発話、明示的な中止、Ornith 結果、TTS 再生の競合時に、古い結果を誤って読み上げない。失敗は履歴と画面で確認できる。

## 2. 現状と変更点

| 現状の経路 | Queue 化で必要な変更 |
|---|---|
| `src/lib/conversationAsrCapture.ts` は partial/final の認識をメモリで順番に処理し、`queueConversationAsrDelivery` もメモリ上の重複防止。 | ASR の **final** テキストが得られた後、同じ発話 ID で SQLite へ受理する。partial は表示専用。ASR 音声サンプルは仕事キューへ保存しない。 |
| `ConversationCheckPage.tsx` の `audioResponseQueue` は Promise の直列化。`send` が LLM と TTS の完了まで待つ。 | 送信は受理確認で返す。画面は永続スナップショットとイベントから `queued / running / waiting / ready / speaking / failed` を描く。ページがスケジューラを所有しない。 |
| `conversation_check.rs` は `BUSY` で全体を一件に制限し、Qwen 判定→Ornith 呼出し→回答保存を一回の IPC 内で行う。 | 受付、Qwen、Ornith、回答確定、音声再生を別の状態遷移に分ける。処理中の追加入力を拒否しない。 |
| `speak_conversation_answer` は `reply_<input_id>` を検索して直接再生する。 | Qwen が確定した `message_id` を再生対象にする。発話単位の音声状態を保存し、再生前に世代・取消しを検査する。 |
| `role_routing` には永続的な待機 root と原子的 claim がある。`conversation_events` など旧会話スキーマも残る。 | claim・復旧の考え方を参照する。ただし別経路の状態を今回の仕事の正本として兼用せず、旧ランタイムを再接続しない。 |

着手前に `git status` と対象ファイルの差分を記録する。現在の `conversation_check.rs`、`ConversationCheckPage.tsx`、`conversationAsrCapture.ts` には未コミット変更があるため、変更を消したり旧版で上書きしたりしない。削除済みの旧計画を復活させない。

## 3. 実行契約

### 3.0 Queue ドメインと保存方式

`src-tauri/src/queue/` を候補に、仕事の状態遷移・容量・公平性・重複排除・lease・再試行・取消し・起床を所有する独立モジュールを置く。Queue ドメインは `Qwen`、`Ornith`、`TTS` の意味を知らず、`lane` と型付き payload 参照だけを扱う。会話側のアダプターが `user_input` 等を各 lane に変換する。公開する操作は `enqueue`、`claim`、`finish_and_enqueue`、`fail_or_retry`、`cancel_generation`、`snapshot`、`recover`、`subscribe` に限定する。`finish_and_enqueue` は上流の完了と下流への登録を一つの transaction にする。UI や LLM ワーカーが SQL を直接書き換えない。

| 保存方式 | 体感速度 | この用途での問題 |
|---|---|---|
| メモリのみ | enqueue/claim は最も軽い。 | アプリ終了・クラッシュで受理済み発話、Ornith 結果、音声状態を失う。画面再接続時の正本も別に必要。 |
| SQLite のみ・定期 polling | ディスク書込みより polling 間隔が待ち時間を増やす。 | 短い間隔にすると無駄な確認が増え、長い間隔にすると体感が悪い。 |
| **SQLite 正本 + メモリの起床通知** | commit 直後に worker を起こせる。LLM・ASR・TTS の待ち時間を妨げない設計にできる。 | 書込み時間と競合を測る必要がある。通知を取りこぼしても DB 再確認で回復させる。 |

採用案は **SQLite 正本 + Tokio `Notify`**。メモリは起床合図、稼働中の worker、必要なら再生成可能な読み取りキャッシュに限り、第二の仕事正本にしない。SQLite WAL と既存 writer を使い、enqueue の transaction を小さくする。ユーザーには commit 完了時点で「受け付けた」と返し、その直後に通知する。最初の反応を速くするために優先度・Provider 接続の再利用・Qwen の出力長を調整する。保存方式による差はまだ測定していないため、P0/P1 で受付 commit 時間、受付から worker 開始まで、発話開始までの p50/p95 を実測する。SQLite の待ちが体感上支配的だと測定されたときだけ、Queue ドメインの storage 実装を見直す。

音声サンプルや ASR partial は短命のストリームであり、この Queue ドメインへ入れない。ASR final から作る確定テキストと、その後の LLM/TTS の仕事が対象。既存の ASR 内部バッファは音声処理の責務として残す。

### 3.1 仕事の種類

| kind | 作成者 → 消費者 | 入力 | 成功時の遷移 |
|---|---|---|---|
| `user_input` | ASR final / UI → Qwen | 会話・発話 ID、確定テキスト、入力種別 | Qwen が `quick` または `delegate` を決定 |
| `ornith_task` | Qwen → Ornith | 元の発話 ID、目的、必要な調査・思考、許可されたツール範囲 | 結果参照を保存し `ornith_result` を作成 |
| `ornith_result` | Ornith → Qwen | 完了状態、検証可能な結果・根拠・限界 | Qwen がユーザー向け本文を確定 |
| `speech` | Qwen の確定発言 → TTS | `message_id`、会話の世代 | 再生完了または明示的な失敗 |

`ornith_result` は成功だけでなく、ツール拒否、期限超過、接続失敗も表す。これらを Qwen に返し、Qwen が利用できなかった事実を伝える。Ornith の内部思考は保存・表示・読み上げしない。

Qwen の出力は `quick`、`delegate`、`answer_from_result`、`ask_user`、`cancel_or_update` など、実装する遷移だけを厳格な JSON 契約で表す。JSON 解析失敗を `quick` とみなさず、上限付き再試行後に失敗または安全な委譲とする。Qwen に Ornith 結果の根拠参照を提示し、発話文にない事実を足さない契約を付ける。回答本文の長さと TTS に渡せる形式を検証する。

### 3.2 永続状態と同一性

`conversation_messages` はユーザー発言と Qwen の確定発言の正本とする。Queue ドメイン共通の仕事台帳と、会話固有の結果・音声状態を追加し、本文の二重正本を避ける。表名は `queue_jobs`、`conversation_queue_results`、`conversation_speech_jobs` を候補とし、既存スキーマと衝突しないことを migration 実装時に確かめる。他ドメインの仕事を直ちに移行するための汎用 payload 保存や公開 API は作らない。

- 共通仕事には `job_id`、`scope_kind`、`scope_id`、`dedupe_key`、`parent_job_id`、`lane`、`kind`、`state`、`generation`、`created_seq`、`attempt`、`available_at`、`lease_until`、`deadline_at`、`error_code` を持たせる。会話アダプターは `scope_id = conversation_id` とし、発話 ID と `message_id` を型付き payload 参照に置く。本文は既存 `message_id` またはサイズ制限付きの専用 payload を参照する。
- `input_id` は UI/ASR の再送でも変えない。`user_input` には `(scope_kind, scope_id, input_id)`、委譲と結果には `(parent_job_id, kind, generation)` の一意制約を置き、同じ受付・委譲・結果を二重作成しない。別 kind の仕事が同じ `input_id` を参照できるようにする。
- `queued → running → completed / failed / cancelled / outcome_unknown` を原子的な更新で行う。`running` は期限付き lease と実行 owner を持つ。claim と完了は `job_id + generation + owner` の条件付き更新にする。
- 元の発話、委譲、結果、可視発言、音声は ID で関連付ける。`conversation_events` は既存の互換スキーマであり、使用するなら今回の `job_id` と順序を表せることを確認する。無理に共有せず、新しい台帳から UI 用イベントを生成してよい。
- ツールの副作用は通常の LLM 再試行と区別する。dispatch 済みで成否が不明なら `outcome_unknown` として停止し、自動再実行しない。既存のツール認可・監査境界を通す。

履歴へのユーザー発言登録と `user_input` 登録、Ornith 結果保存と `ornith_result` 登録、Qwen 発言保存と `speech` 登録は、それぞれ一つの SQLite transaction で確定する。下流の登録が確認できないまま上流の仕事を `completed` にしない。

### 3.3 スケジューラと起床通知

各 transaction の commit 後に一つの `Notify` へ通知する。スケジューラは通知をまとめて受け、実行可能な仕事を DB から claim し、空になるまで確認する。ワーカー終了時には成功・失敗・取消しのどの場合も状態を確定して再通知する。通知待ちの登録と DB 再確認の順序を定め、commit と待機の間の lost wakeup を防ぐ。起動時に未完了の仕事を走査し、期限切れ lease を回収する。補助的な短い再確認を設けても、時刻だけを正本にしない。

Qwen の判断実行は会話ごとに直列化する。Ornith は独立したワーカーとして、LARM/provider の容量に合わせた上限で動かす。TTS は全体で同時再生を一件にする。キュー満杯時は受付前に明示的に拒否し、受理済み発話を黙って捨てない。上限値は現行の ASR final 8 件や既存ルーティング設定を参考に、実測後に確定する。

「LLM 完了時に Queue チェックイベントを再発動」は **LLM を自分で再呼び出す**意味にはしない。ワーカーが結果を commit しスケジューラを起こす。スケジューラは次の仕事がある場合だけ対応する LLM を呼ぶ。処理失敗後の再試行は上限・間隔・失敗種別を持ち、無限ループにしない。

### 3.4 新着発話・割り込み・音声

Ornith の実行中にも新しい `user_input` を受理する。Qwen は次の判断境界で新着を読み、同じ依頼の修正・中止・別の依頼を区別する。新着があるだけで既存の仕事を自動取消ししない。修正・中止を確定した場合は対象仕事の `generation` を進め、古い Ornith 結果が届いてもユーザー向け回答と音声を作らない。既に実行したツールの副作用は取消し済みと偽らない。

TTS は Qwen の確定した発言だけを取得する。再生開始直前に `generation`、発言状態、再生済み ID を検査する。再起動後に自動で再生を繰り返すと危険なため、状態不明の `playing` は `interrupted` として表示し、明示的な再生要求がある場合だけ再生する。再生中の新発話については、現行のマイク・TTS 抑制挙動を保持し、ユーザーの割り込みが確定した場合に音声を止める経路を設ける。画面には Qwen の確定本文と音声状態を同じ `message_id` で示す。

## 4. 実装順序と完了ゲート

| 段階 | 作業 | 完了ゲート |
|---|---|---|
| P0: 現状測定 | 現行の quick、think、ASR 連続入力、TTS 失敗、画面を閉じた場合を fixture と実機で記録。入力受理から Qwen 開始、Ornith 開始、回答表示、再生開始までを測る。 | 同じ条件で移行後と比較できるログ・時間分布・既知の失敗を保存する。 |
| P1: Queue ドメインと永続受付 | 独立した Queue の状態遷移・storage・scheduler・起床通知を実装し、会話アダプターから受理する。原子的 claim、重複排除、lease 回収を含む。UI/ASR 送信は受付結果を返す。既存経路はこの段階では維持。 | Queue 単体の lost wakeup・多重 claim・公平性、コピーした旧 DB の migration、同一 ID 再送、満杯、起動復旧の試験が通る。保存済み設定を変更しない。 |
| P2: Qwen ワーカー | `user_input` 消費と `quick / delegate` 契約を実装。quick の発言を永続化し、delegate を transaction で登録。 | malformed JSON、重複通知、Qwen 失敗を含む fixture で一発話につき一つの判断。 |
| P3: Ornith ワーカー | 委譲の取得、LARM `llm` 呼出し、結果・失敗の登録、Qwen への受け渡しを実装。まず思考のみ、その後、認可済みツールを既存の監査・権限境界で接続。 | 結果が Qwen に一度だけ届く。ツールの失敗・不明結果を再実行せず伝えられる。 |
| P4: Qwen 回答と TTS | Qwen が Ornith 結果から可視発言を確定し、音声ジョブを作る。UI を永続スナップショット基準に変更し、旧 Promise 直列化・`BUSY` による受付拒否を外す。 | Ornith が直接可視化・読み上げされない。画面の再読み込み後も状態が一致し、TTS は一度だけ再生する。 |
| P5: 追加入力と復旧 | 新着入力の取り込み、明示的な修正・中止、世代 fencing、再起動・クラッシュ復旧を統合。 | 各 commit 境界での停止と再開、新旧結果の競合、再生中割り込みの試験が通る。 |
| P6: 切替と実機評価 | 新経路を内部フラグで限定して試し、固定シナリオで品質と遅延を比較して既定化。 | 下記ゲートを満たす。劣化時はフラグで旧経路に戻せ、受理済み仕事の扱いが明確。 |

各段階で Queue ドメインの schema → repository → scheduler、次に会話アダプター → worker → IPC → UI の依存順に変更する。既存の `src-tauri/src/runtime/conversation_check.rs` は肥大化しているため、Queue の状態遷移と SQL は独立モジュールへ分け、会話固有のプロンプト・結果変換は会話側へ置く。テストも対応するモジュールの近くに置く。P1 の段階で feature flag を閉じ、P4 までは旧経路を利用可能にする。ただし同じ発話を新旧両経路へ同時 dispatch しない。

## 5. 検証計画

### 決定的な試験

| ケース | 期待結果 |
|---|---|
| ASR final と手入力、同一 ID の再送 | 各発話が一件だけ受理され、partial は仕事にならない。異なる本文の同一 ID は拒否。 |
| quick / delegate / Ornith 失敗 | quick は Ornith 呼出しゼロ。delegate は一件。失敗は Qwen に届き、存在しない成果を発話しない。 |
| commit 直後の通知欠落・重複 | 起動時・完了時の DB 再確認で進み、LLM 呼出しと回答の重複はない。空キューで再起動しない。 |
| claim 中・LLM 完了直後・結果 commit 前の停止 | lease 回収または失敗状態へ収束する。確定した発言は失わず、未確認のツール副作用を自動反復しない。 |
| Ornith 中の追加入力と取消し | 入力は受理される。取消し後の古い結果は可視回答・音声にならない。別件の入力は不必要に取消されない。 |
| TTS 再生前・再生中・再起動後 | 同じ確定発言だけを再生し、古い進捗や状態不明の音声を自動再生しない。 |
| キュー満杯、Provider 不可、タイムアウト | 明示的な失敗を返し、受理済み入力を消さない。再試行回数に上限がある。 |
| 旧 DB migration | コピーした既存 DB で履歴・Provider 設定・認証参照・ルーティング設定が維持される。空の新規 DB でも起動する。 |

Provider 資格情報不要の SQLite repository / scheduler / worker fixture を先に作る。ライブの Qwen・Ornith・ASR・TTS は別レーンで、固定した短い挨拶、要調査の質問、途中の訂正、TTS 割り込みを確認する。実機評価では P0 と同じ条件を使い、初回音声までの時間、最終回答までの時間、重複・欠落率、誤委譲率、事実保持率を比較する。許容幅は P0 測定後、P6 の結果を見る前に固定する。

### 実行コマンドと判定

- P1–P5 の都度: 該当 Rust モジュールの `cargo test --manifest-path src-tauri/Cargo.toml <targeted-filter>` と該当 UI/ASR の `bun test tests/conversation-asr-continuous.test.ts <追加した対象テスト>`。各段階の決定的なケースが成功すること。失敗したら job/event/lease の DB 状態を照合して修正する。
- IPC 変更後: `bun run ipc:check`、`bun run typecheck`。生成契約と UI の型が一致すること。
- P4–P6: `bun run quality:check`、`bun run build:frontend`。会話品質の既存ゲートと画面ビルドが通ること。
- 統合時: `bun run check`、`bun run spec:check`。既存ゲートが通ること。`spec:check` が対象外の既存エラーで失敗した場合は差分と原因を明記する。
- 実機: ASR→Qwen quick→TTS、ASR→Qwen delegate→Ornith→Qwen→TTS、途中訂正、Provider 障害、再起動を同じ記録方式で確認する。自動試験の代わりに実機成功のみで合格にしない。

## 6. 切替・戻し方・残る判断

旧経路を削除する前に、新経路の受理済み仕事がある状態でのフラグ切替を試験する。切替時は新規受付先だけを変更し、旧世代の受理済み仕事を黙って破棄しない。戻す場合は未完了仕事を停止・表示し、必要に応じて同じ `input_id` から明示的に再開する。データベースを巻き戻したり、保存済み設定や Provider を初期化したりしない。

実装前に確定する運用値: キュー容量、Qwen/Ornith の同時実行上限、lease 長、各失敗種別の再試行上限、入力が続いた場合の公平性、回答の最大文字数、P0 に対する許容遅延。会話の意味判断が必要な「訂正か別件か」は Qwen の明示的な判断として記録し、ランタイムは ID と世代で安全性を保証する。
