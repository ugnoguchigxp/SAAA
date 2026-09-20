# Situation TTS ゲート完了記録

作成日: 2026-09-20。状態: Step 3 実装完了（会議中の TTS **開始** 抑止）。live 実会議・実マイクは未検証。計画は `spec/docs/saaa-situation-tts-gate-plan.md`。カード別は `tts-gate-progress.md`。

## 1. 結論

`scene == MEETING` かつ `proposed_attention` が `IGNORE` または `OBSERVE` のとき、runtime が TTS 開始を止める。判定はメモリ上の scene / attention のみ。分類器・hysteresis 定数・`meeting.blocks_tts()` は変えていない。`ShadowDecision.mode` は `shadow`、`actual_execution` は `NONE` のまま。

入口は二つ。`voice_response` の ack/thinking/complete（`completion_state`）と `begin_turn_speech_policy` の streaming（`upper_policies_allow_speech`）。Situation を meeting / voice_behavior より先に見る。公開する `VoicePresentationDecision.reasonCode` は `situation_hold`（既存 union へ追加。新コマンドなし）。会話ポリシー snapshot の reason は変えない。

## 2. ゲート

| ゲート | コマンド | 結果 |
| --- | --- | --- |
| 本 Step 試験 | `cargo test --locked --manifest-path src-tauri/Cargo.toml --lib situation::speech` | **7 passed** / 0 failed |
| filter `st_` | `... --lib st_` | 78 passed（他モジュールの名前部分一致を含む。本 Step のカード試験は上記 7） |
| Situation 回帰 | `... --lib situation::` | **42 passed** / 0 failed / 1 ignored（着手前 36 + 本 Step 7。ignored は既存 soak） |
| Clippy | `cargo clippy --locked --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings` | passed |
| size | `bun run size:check` | passed（681 files）。閾値未緩和。`speech.rs` / `speech_tests.rs` を登録 |
| spec | `bun run spec:check` | passed |
| check:local | `RUST_TEST_THREADS=2 bun run check:local` | passed。saaa lib **972 passed** / 16 ignored |

filter 0件を成功扱いしていない。

## 3. 実装済み / offline合格 / live未検証 / 未着手

| 区分 | 内容 |
| --- | --- |
| 実装済み | `situation/speech.rs` の hold 判定と `tts-held` 監査。`voice_behavior/completion.rs` への配線 |
| offline合格 | 合成 MEETING+OBSERVE で `voice_response::start` が None。hysteresis（定数変更なし）後の CODING で Some。FOCUS/UNKNOWN/RESPOND は誤抑止しない。監査に本文なし |
| live未検証 | 実カレンダー、実会議、実マイク、実 TTS 再生 |
| 未着手 | 進行中チャンクの即時 mute（deferred）。分類精度。Step 4 最小循環 |

## 4. 未接続

- `streaming_tts` 内部の途中 mute
- Situation mode の enforce 昇格
- WorldFrame を TTS 判定に使うこと
- 新 IPC / schema bump / 新環境変数

## 5. 引き渡し

Step 4 最小循環は本ゲートを通知保留に再利用してよい。分類精度は本 Step の完了条件にしない。
