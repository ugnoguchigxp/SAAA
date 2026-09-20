# Situation TTS ゲート進捗

作成日: 2026-09-20。並行差分は巻き戻さない。完了記録は `tts-gate-results.md`。

## ST-00 baseline

| 項目 | 値 |
| --- | --- |
| HEAD | `84ead9752a509cec4d5a81810816cd17cefc507f` |
| Situation 試験（着手前） | `cargo test --lib situation:: -- --list` → **36** tests |
| snapshot | `snapshot_locked` は ledger 履歴を DB から読む。TTS には使わない |
| TTS 入口 | `voice_response::start` / `complete`（`completion_state`）と `start_turn` の `streaming_tts.begin`（`begin_turn_speech_policy` → `upper_policies_allow_speech`） |
| `meeting.blocks_tts` | Meeting セッション Active/Paused/Stopping。Situation scene `MEETING` とは別 |
| 凍結 | Role Routing、D5 dirty、classifier / hysteresis 定数、進行中 mute |

## カード

| ID | 状態 | 証拠 |
| --- | --- | --- |
| ST-00 | 完了 | 本記録 |
| ST-01 | 完了 | `st_01_reads_memory_without_reclassify` |
| ST-02 | 完了 | `st_02_holds_only_meeting_ignore_or_observe` |
| ST-03 | 完了 | `st_03_hold_blocks_voice_start_and_streaming_policy` / `st_03_non_hold_allows_speak_policy` |
| ST-04 | 完了 | `st_04_held_audit_has_no_body` |
| ST-05 | 完了 | `st_05_start_none_then_some_after_hysteresis` |
| ST-06 | 完了 | `st_06_shadow_invariants_and_no_side_channels` |
| ST-07 | 完了 | size 681、clippy、spec:check、check:local。`tts-gate-results.md` |
