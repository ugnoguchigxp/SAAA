# Ornith 単独会話への移行計画

状態: 単一の Ornith 会話仕事へコードを移行。2026-09-27、`codex/single-llm-runtime` のコードを確認して作成。実 LARM・実音声の確認は未実施。

実装上の現在形: 新規入力は `conversation/user_input` 一件で、Ornith の思考、ツール選択、結果確認、回答保存まで行う。直接会話 API も同じ仕事を待つ。旧 `qwen` / `ornith` レーンの未完了仕事は起動時に `conversation` へ移し、旧 worker は起動しない。既に生成された旧 `ornith_result` だけは再推論せず公開する互換処理を残す。

回答生成は SSE を使い、`answer.content` の増分だけを画面と TTS に渡す。ツール指示の JSON は読み上げない。文の区切りで音声合成を開始し、生成完了後に回答を検証・保存する。途中で実際に音声が始まった後の失敗は自動再試行しない。

## 1. 目的と範囲

Qwen 3.5 2B による一次判定をなくし、Ornith の `llm` 一つで短い会話、思考、Web・記憶ツールの選択、最終回答を担う。SAAA は入力の受付、処理キュー、ツールの実行と権限、回答の保存、TTS を引き続き担う。短い依頼も Ornith が回答し、モデル間の引き継ぎは発生しない。

対象は `conversation_check` のテキスト・音声会話、Role Routing の標準構成、LARM セッションの要求 Provider、設定画面と関連テスト。ASR・TTS・embedding のモデル変更、Memory・WorldModel の正本、認証方式、公開 IPC 契約の変更、汎用 Role Routing から複数 actor 機能を撤去することは対象外とする。特に ASR に Qwen 名のモデルが残ることは、会話用 Qwen 2B の撤去とは別件である。

既存の保存済み Provider 設定をリセットしたり、接続失敗を理由に別 Provider へ自動交換したりしない。DB 移行はコピーまたは隔離 DB で先に検証する。

### 成功条件

1. 新規の会話入力から `backchannel` への推論要求が 0 件で、回答とツール判断は `llm` の Ornith が行う。
2. 挨拶、短い質問、調査、ツール利用、前の依頼の中止・訂正が、テキストと音声の両方で完了する。回答は一度だけ保存・発話される。
3. Qwen 2B を含まない LARM の会話 Profile に接続できる。ASR・TTS・embedding の利用は維持する。
4. 更新前の未完了キュー仕事と保存済み Role Routing 設定を失わず、更新後に Qwen 2B の起動を要求しない。
5. TTS のみの再生は ASR 発話を作らず、再生中の人の声は ASR に届く。

## 2. 現状と変更点

| 現在の経路 | コード上の根拠 | 変更方針 |
|---|---|---|
| 入力は `qwen/user_input` に入り、Qwen が `quick`・`think`・`cancel`・`replace` を選ぶ | `src-tauri/src/runtime/conversation_check/queue_runtime.rs` の `enqueue`、`process_qwen` | 新規入力を Ornith の仕事へ直接渡す。中止・置換の処理はキューの状態に基づいて維持する |
| Qwen の `web_query` で Ornith の前に検索を開始する | 同 `process_qwen`、`process_ornith` | 先行検索をなくし、Ornith の `web_search` / `fetch_content` 判断に一本化する |
| Ornith が JSON の `answer` またはツール操作を返し、SAAA がツールを実行する | 同 `process_ornith`、`queue_context.rs` | この有限ループと権限・根拠検証を維持し、短い返答にも適用する |
| Ornith の結果は `qwen/ornith_result` に戻るが、Qwen は再推論せず保存する | 同 `process_qwen` | 結果公開をモデル名に依存しない仕事へ分離する |
| 直接会話経路でも `backchannel` の後に `llm` を呼ぶ | `src-tauri/src/runtime/conversation_check.rs` の `respond_with_larm` 付近 | `llm` 一回を入口にし、同等の応答・失敗契約に揃える |
| LARM セッションは既定で `backchannel` を含む五つの Provider を要求する | `crates/larm-session/src/contract.rs`、`src-tauri/src/runtime/conversation_check.rs` の `connect_larm` | 会話に必要な四つを要求し、`backchannel` を必須から外す |
| 標準 Role Routing と画面の執事構成は `frontend + reasoner` | `src-tauri/src/role_routing/contracts.rs`、`src/features/settings/settingsRoleRoutingDefaults.ts`、`settingsRoleRouting.ts` | 既定レシピを `reasoner` 単独へ変更し、保存済み設定を個別に扱う |

## 3. 実装段階

### P0: 基準値と契約の確認

- 現行の決定的 fixture で、挨拶・調査・ツール・中止・訂正・音声割り込みの結果、`backchannel` / `llm` 呼び出し数、初回発話までの時間、総応答時間を記録する。失敗中のテストは変更前からの失敗として分けて記録する。
- LARM の対象 Profile が `llm`、`asr`、`tts`、`embedding` の要求を受け付け、`backchannel` を省いても claim と lease が成功することを fixture と実接続で確認する。サーバー側 Profile の変更が必要なら、アプリ変更と分けて実施条件を記録する。
- 新規会話と更新前の未完了仕事の状態を列挙し、DB コピーで移行を試す。実 DB は検証に使わない。

### P1: Ornith の会話契約を統一

- `queue_context.rs` の役割説明から Qwen による引き継ぎ・最終公開の記述を除き、Ornith が短い相槌から長い依頼まで最終回答を作ると明記する。回答・Web・記憶ツールの JSON 契約は一つに保つ。現在の発話、履歴、Memory、WorldModel の信頼境界と、回答後・保存前の根拠検証を維持する。
- Qwen が付与した `web_query` を入力契約から外す。最新情報が必要なときは Ornith が `web_search` を選び、必要なら `fetch_content` へ進む。ツール回数とタイムアウトは SAAA が制限する。
- 短い返答にも現在の会話履歴と必要な個人設定を渡す。単純な挨拶のために別のモデル呼び出しや別 SystemContext を作らない。

### P2: 処理キューを単一モデルの経路へ変更

- 新規入力は直接 Ornith の会話仕事に登録する。`qwen` と `ornith` というレーン名に依存した責務を、推論と結果公開の仕事として整理する。Ornith の結果を確定する処理は再推論せず、既存の `commit_answer`、発話予約、監査を使う。
- 中止・訂正を受けた場合は、処理中の仕事のキー・世代・取消し状態を SAAA が確認する。モデルの文字列だけで既存仕事を取り消さない。置換時は旧結果の公開と旧音声を止め、新しい入力を一つの仕事として処理する。
- 初回の「少し考えます」等は Qwen の出力に依存しない進捗発話として扱う。待機中にだけ一度出し、短い回答が先に確定した場合は抑止する。時刻による ASR 停止やマイク入力ゲートを設けない。
- 永続キューには更新前の `qwen/user_input` と `qwen/ornith_result` が残り得る。隔離 DB で回復手順を検証し、未完了入力は新経路へ、既に生成済みの結果は再推論せず公開処理へ写す。実行中断後の再開、重複 enqueue、キャンセル済み仕事を確認してから旧 worker を撤去する。

### P3: LARM と Role Routing を揃える

- SAAA の会話接続は、LARM Session の要求 Provider 指定を用いて `llm`、`asr`、`tts`、`embedding` を取得する。`backchannel` がなくても ready・claim・再接続が成功することを確認する。LARM ライブラリの既定必須集合も用途に合わせて見直すが、明示的に `backchannel` を使う他のクライアントの受け入れは壊さない。
- Rust と TypeScript の標準 Role Routing を `reasoner` 一人の `respond` に揃える。画面の「執事構成を適用」も同じ構成を作り、表示から Qwen 受付が標準であるかのような文言を外す。
- 保存済み設定は無条件に上書きしない。旧標準構成と一致する設定だけを識別して変換し、利用者が編集した actor・recipe・Provider は残す。変換不能な設定は明示的な診断を返し、別 Provider への暗黙の切替をしない。
- 汎用 Role Routing の `frontend` / `tool_specialist` 型や歴史的 migration は、この段階で一括削除しない。実運用からの Qwen 2B 除去と、汎用機構の廃止を分ける。

### P4: 残存箇所と利用体験の整理

- 直接会話経路、Provider 接続テスト、fixture、監査名、画面表示、文書を横断し、新規会話が `backchannel` を呼ぶ箇所をなくす。過去の監査記録と実験計画は履歴として保持する。
- 初回発話までの時間と回答完了時間を P0 と同条件で比較する。単一モデル化で短い返答が遅くなる場合は、Ornith のストリーミング、進捗発話のタイミング、モデルの起動状態を測定して調整する。Qwen 2B の再導入を自動的な性能回避策にはしない。

## 4. 検証と完了ゲート

| 試験 | 期待結果 | 失敗時 |
|---|---|---|
| LARM Session の四 Provider fixture と接続テスト | `backchannel` なしで claim・`llm` 実推論・ASR/TTS/embedding 取得に成功 | Profile 側とアプリ側のどちらが拒否したか分けて修正。保存済み Provider を交換しない |
| キュー E2E: 挨拶、短い質問、Web、記憶ツール | 各入力で `llm` が応答し、`backchannel` 呼び出し 0。回答と発話は一度だけ | レーンと再試行の監査を確認し、重複公開を修正 |
| キュー E2E: 中止、訂正、失敗、再起動 | 旧結果は公開されず、新しい依頼だけが確定。失敗は明示され、仕事が実行中のまま残らない | DB コピーで各 job 状態と移行を再現 |
| Role Routing 設定・保存済み設定の移行 | 新規既定は reasoner 単独。編集済み設定は保持し、旧標準構成だけ変換 | 移行を止め、コピーの差分を確認。実 DB を初期化しない |
| 音声 E2E | TTS のみでは ASR 発話 0。TTS と重なる人の発話は ASR に到達し、割り込み後も会話できる | PCM 参照と AEC の経路を調べ、再生状態による blanket gate を追加しない |
| 実機の短文・調査の時間比較 | P0 と同じ条件で初回発話、完了時間、失敗率を報告 | 数値をもとに遅延箇所を特定し、採否を判断 |

対象テストの入口は `cargo test --locked --manifest-path crates/larm-session/Cargo.toml`、`bun run e2e:conversation-queue`、`bun run typecheck`。Role Routing の Rust 単体テストと設定画面の関連 Bun テストを追加・更新し、変更箇所が通った後に `bun run check` をフルゲートとして実行する。実機 LARM・音声試験は fixture とは別に行い、未実行なら未確認と記録する。既存の全体コンパイルエラーがある場合は、変更前の再現結果と区別して報告する。

完了報告には、変更前後の経路図、`backchannel` 呼び出し数、LARM Profile の実接続結果、キュー移行結果、対象テストとフルゲートの結果、音声二条件、応答時間の比較を残す。
