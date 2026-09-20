# Situation TTS ゲート進捗

作成日: 2026-09-20。並行差分は巻き戻さない。

## ST-00 baseline

| 項目 | 値 |
| --- | --- |
| HEAD | `84ead9752a509cec4d5a81810816cd17cefc507f` |
| Situation 試験 | `cargo test --lib situation:: -- --list` → **36** tests |
| snapshot | `snapshot_locked` は ledger 履歴を DB から読む。TTS には使わない |
| TTS 入口 | `voice_response::start` / `complete`（`completion_state`）と `start_turn` の `streaming_tts.begin`（`begin_turn_speech_policy` → `upper_policies_allow_speech`） |
| `meeting.blocks_tts` | Meeting セッション Active/Paused/Stopping。Situation scene `MEETING` とは別 |
| 凍結 | Role Routing、D5 dirty、classifier / hysteresis 定数、進行中 mute |

## カード

| ID | 状態 |
| --- | --- |
| ST-00 | 本記録 |
| ST-01〜07 | 実装中 |
