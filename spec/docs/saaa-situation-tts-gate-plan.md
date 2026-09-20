# Situation TTS ゲート実装計画 — 会議中の発話開始抑止

作成日: 2026-09-20。状態: Step 3 の実装計画。コードは本計画の作成では変更しない。

前提は M3B 完了記録（`spec/evidence/world-model/m3b-results.md` 引き渡し）。上位は [執事循環フェーズ](saaa-steward-loop-phase-plan.md) §7。カードは本書 §8（ST-00〜07）。作業カード文書の Step 3 表は本書への索引とする。

## 1. 次に完成させるもの

Interaction Policy の最初の実体。会議中に SAAA が勝手に話し始めないこと。

条件（この論理式以外で抑止しない）:

`scene == MEETING` かつ `proposed_attention` が `IGNORE` または `OBSERVE`。

動作:

1. 音声応答の **開始** を止める（ack / thinking / complete の TTS、および turn 開始時の streaming TTS）。
2. 抑止した事実を既存監査へ残す（本文なし）。
3. テキスト turn、分類器、シグナル、hysteresis 定数、Meeting セッション所有者、`ShadowDecision.mode` / `actual_execution` / `actual_presentation` は変えない。

利用者向けの新フラグは作らない。Situation 監視が OFF なら scene は初期 `UNKNOWN` のままなので本ゲートは発火しない（default OFF）。

live 実会議・実マイクでの品質は完了条件にしない。

## 2. フェーズ計画から固定する判断

カード内で再議論しない。

| # | 判断 |
| --- | --- |
| 1 読取 | 最新の **メモリ上** `SituationState.scene` と `ShadowDecision.proposed_attention` を読む。分類を再実行しない。ledger / snapshot 本文 / 履歴を TTS 経路へ返さない。`snapshot()` は DB を読むので使わない。 |
| 2 強制の場所 | 分類結果を書き換えない。runtime が TTS 開始を止める。`mode=shadow`、`actual_execution=NONE`、`actual_presentation=SILENT` のまま。 |
| 3 明示入力 | 既存 `shadow_policy` が `RESPOND`（`explicit-saaa-interaction`）なら本ゲートは通さない。`input_origin == "voice"` だからといって attention を RESPOND に書き換えない。tick 未反映で OBSERVE のままなら抑止する（分類遅延は本 Step の欠陥にしない）。 |
| 4 Meeting セッション | `MeetingRuntime::blocks_tts()` は **別政策**（Active/Paused/Stopping の会議セッション）。Situation scene `MEETING`（カレンダー / 前景）とは一致しない。既存 `meeting_blocked` は残す。本ゲートで置換しない。streaming_tts 内の `MEETING_POLICY_TTS_BLOCKED` も触らない。 |
| 5 入口 | TTS 開始は `voice_response::start` / `complete`（既存 `speech_allowed` → `completion_state`）と `start_turn` の `streaming_tts.begin`（`begin_turn_speech_policy`）の **二つ**。片方だけ止めると穴になる。`voice_response.rs` に大きな分岐を足さず、voice_behavior の speak / streaming 判定の **前** で Situation を見る。 |
| 6 進行中発話 | 既に出ている streaming チャンクの即時 mute は完了条件にしない。deferred の一行を維持する。 |
| 7 監査 | 既存 `audit_events`（7 日、component `situation` は許可済み）。属性は既存 allowlist（`reasonCode`、`state`、`proposedAttention`、`runtime_run_id`）。会議名、発話本文、transcript を書かない。schema version を上げない。新 IPC 0。 |
| 8 World | WorldFrame / M3B を TTS 判定に使わない。 |

## 3. 実装調査で分かった接続点

- `situation/classifier.rs` の `shadow_policy`: Sensitive → IGNORE。conversation が Idle 以外、または mic が SaaaCapturing / Transcribing → RESPOND。UNKNOWN / 低信頼 → IGNORE。MEETING または busy → OBSERVE（`user-busy`）。分類器・hysteresis（enter 3 / exit 5 / cooldown 10_000 ms）は変更禁止。
- `SituationRuntime::snapshot` / `snapshot_locked` は **毎回 ledger 履歴を DB から読む**。TTS ホットパスに使わない。`RuntimeInner` の `state` と `decision` だけを Copy して返す関数を `situation/speech.rs` に置く。`mod.rs` を大きくしない。
- `voice_response.rs` の `start` は `voice_response_enabled`、`input_origin=="voice"`、`speech_allowed`（`completion_state.decision=="speak"`）のあと ack@250ms と thinking@4s を spawn する。試験は現状なし。
- `voice_behavior/completion.rs` の `resolved_presentation` は `meeting.blocks_tts()` のあと auto_speak / override を見る。Situation は **その前**。`upper_policies_allow_speech` も同じく先に見ないと `streaming_speech=true` のまま `streaming_tts.begin` が走る。
- `runtime/start_turn.rs` は `begin_turn_speech_policy` のあとに `streaming_tts.begin` する。ここが第二入口。
- 監査 `validate_event` は component `situation`、phase `decision`、outcome `blocked`、属性 `proposedAttention` を既に許可している。allowlist 拡張は原則不要。event_name は `tts-held`（小文字・ハイフン、80 字以内）。
- Situation 無効時は監視 tick が scene を進めない。初期 scene は `UNKNOWN`。誤抑止しない。
- SUGGEST かつ MEETING は現行 classifier では出ない（MEETING は OBSERVE）。論理式は IGNORE/OBSERVE のみ。SUGGEST では抑止しない。FOCUS のみ・UNKNOWN では抑止しない。

## 4. 契約（B0–B7）

**B0 入口と順序。** `situation::speech_holds_tts(state) -> SpeechHold`。hold なら `upper_policies_allow_speech` は false、`resolved_presentation` は `decision=silent` / `reason_code=situation_hold`。そのあと既存の meeting / auto_speak / override。hold でなければ既存どおり。classifier を呼ばない。

**B1 読取。** `SituationRuntime` の Mutex を短く取り、`scene` と `proposed_attention` だけ Copy。ロック中に SQLite / 分類 / tick をしない。ロック失敗は hold しない（黙り過ぎより話し過ぎ。失敗は監査しない）。

**B2 論理。** hold ⇔ `MEETING` ∧ (`IGNORE` ∨ `OBSERVE`)。RESPOND / SUGGEST / 他 scene は hold しない。

**B3 開始抑止。** hold のとき `voice_response::start` は `None`（ack/thinking spawn 0）。`complete` の speak 経路に入らない。`begin_turn_speech_policy` の streaming と enabled はともに false。テキスト生成は続ける。

**B4 監査。** hold で実際に開始を止めたとき 1 件。`component=situation`、`event_name=tts-held`、`phase=decision`、`outcome=blocked`。属性: `reasonCode`（`user-busy` または decision の既存 reason をコピー。本文にしない）、`state`（scene の SCREAMING_SNAKE）、`proposedAttention`。`runtime_run_id` は列へ。会議タイトル・ユーザー発話・モデル本文・calendar 詳細は禁止。同一 run の start と complete で二重に止める場合はイベント 2 件まで可（本文なし）。

**B5 非対象。** 自動通知、Meeting start/stop、アプリ操作、Situation mode 昇格、新環境変数、新 IPC、schema bump、Role Routing、World 配線、第二 Writer、会話 Runtime 新設。

**B6 試験。** 接頭辞 `st_`。実 DB（既存 `AppState` / 一時 SQLite）。clock は `tick_sampled` の `observed_ms` 注入。hysteresis 定数は変えない。filter 0 件を合格にしない。

**B7 規模。** 新規は `situation/speech.rs` と試験。`classifier.rs` / `tick.rs` は行を足さない。`situation/mod.rs` は `mod speech` と薄い委譲のみ。size 閾値を緩めない。追加ファイルは size baseline へ登録。

## 5. ファイル分担（実装 5 を超えたら枝番）

| ファイル | 役割 |
| --- | --- |
| `situation/speech.rs` | hold 判定、監査発行。試験 `st_01`〜`st_02`、`st_04`、`st_06` |
| `voice_behavior/completion.rs` | B0 を `resolved_presentation` と `upper_policies_allow_speech` に接続 |
| `voice_behavior.rs` | `begin_turn_speech_policy` が streaming 側も hold を見ることを確認。ロジック重複を作らない |
| `runtime/voice_response.rs` | 原則変更なし。`speech_allowed` 経由で受ける。コメントや分岐を増やさない |
| `situation/speech_tests.rs` または `voice_response` 側試験 | `st_03` / `st_05`: start が None、hysteresis 後に Some |

`start_turn.rs` と `streaming_tts/` と `classifier.rs` は本 Step で編集しない。

## 6. ゲート

| ゲート | コマンド | 合格 |
| --- | --- | --- |
| 本 Step | `cargo test --locked --manifest-path src-tauri/Cargo.toml --lib st_` | 件数 > 0、failed 0 |
| Situation 回帰 | 同 `--lib situation::` | 既存失敗を本 Step の欠陥としない。新規失敗 0 |
| Clippy | `cargo clippy --locked --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings` | 警告 0 |
| size | `bun run size:check` | 閾値未緩和 |
| spec | `bun run spec:check` | 警告をエラー |
| 全体 | `bun run check:local` | 機能カード通過だけで Step 完了としない。host timeout は再実行し記録 |

並行 dirty（Role Routing / D5）は触らない。0 件実行を合格にしない。

## 7. 完了報告

`spec/evidence/situation/tts-gate-progress.md` と `tts-gate-results.md`。Spec HTML から `../evidence/` へはリンクしない。

報告は「実装済み / offline合格 / live未検証 / 未着手」を分ける。実マイク・実カレンダー・実会議は live未検証。

引き渡し（results に一行）: Step 4 最小循環は本ゲートを通知保留に再利用してよい。分類精度は本 Step の完了条件にしない。

## 8. カード（ST-00〜07）

| ID | 対象 | 契約 | 実装すること | 合格条件 |
| --- | --- | --- | --- | --- |
| ST-00 | `spec/evidence/situation/tts-gate-progress.md` | §6 | HEAD、dirty、Situation 試験件数、`snapshot` が DB を読むこと、TTS 二入口、`blocks_tts` が別政策であることを記録 | 0 件実行を合格にしない。並行差分を巻き戻さない |
| ST-01 | `situation/speech.rs` | B1 | `speech_holds_tts`。scene と attention のみ。serde / IPC なし | 分類 0 回。ledger 本文 0 |
| ST-02 | 同 | B2 | 論理式のみ。FOCUS / UNKNOWN / RESPOND / SUGGEST は hold しない | 誤抑止 0 |
| ST-03 | `voice_behavior/completion.rs`（必要なら `voice_behavior.rs`） | B0/B3 | Situation を meeting / voice_behavior より先に見る。streaming と speak の両方 | hold 時 spawn 0、`streaming_tts.begin` 相当の enabled 0。テキスト turn 継続 |
| ST-04 | `speech.rs` 監査 | B4 | `tts-held`。本文なし | 会議名 / transcript 0。allowlist 外キー 0 |
| ST-05 | 実DB試験 | B6 | 合成 MEETING+OBSERVE で `voice_response::start` が None。hysteresis（定数変更なし）経過後の非 MEETING で再び Some（voice origin と既存 speak 条件を満たす fixture） | clock 注入。classifier 定数変更 0 |
| ST-06 | 境界試験 | B5 | mode が shadow のまま。actual_execution NONE。Meeting start API を呼ばない。新 IPC 0 | 文字列検査または API 非呼び出し |
| ST-07 | size / check:local / spec:check / results.md | §6/7 | baseline 登録。閾値未緩和 | 対象 suite 件数 > 0。offline合格と live未検証を分ける |

ST-03 の実装漏れとして残すもの: 進行中 TTS の即時 mute。開始抑止が完了条件。

指示例: 「ST-03 だけを実装してください。`classifier.rs` を編集しないでください。`meeting.blocks_tts` を Situation 判定に使わないでください。`snapshot()` を TTS 経路から呼ばないでください。」
