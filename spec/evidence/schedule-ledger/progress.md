# Butler Schedule Ledger 進捗

作成日: 2026-09-21。計画: `spec/docs/saaa-butler-schedule-ledger-plan.md`。

## SL-00 着手時

- 作業ツリーは dirty（Role Routing / 生成クローズアウト / steward が並行）。
- Goal / Delegation の正本は `steward_goals` / `steward_delegations`。schedule の `delegation_ref` は文字列・NULL 許容。
- Situation TTS hold は `speech_holds_tts` を共用。tick は独自の会議判定を持たない。
- 並行変更は巻き戻していない。

## カード

| ID | 判定 |
| --- | --- |
| SL-00 | 本表 |
| SL-01〜04 | offline合格（型・DDL・CAS・due） |
| SL-05〜09 | offline合格（A/B/C/D/E/F/G と slot 繰り越し） |
| SL-10 | IPC 追加。zod は frontend API schema |
| SL-11〜18 | fake HTTP + 純関数。live 未実施 |
| SL-19〜21 | forget / 分類 / OFF |
| SL-22 | Settings `ScheduleSection`。default OFF |
| SL-23 | `schedule::` 32 passed |
| SL-24 | **未実施**（実 Google アカウント未接続） |
| SL-25 | 本記録 |

## 試験名

`sl_01_*` `sl_02_*` `sl_03_*` `sl_04_*` `sl_05_a_*` `sl_05_d_*` `sl_05_f_*` `sl_05_g_*` `sl_06_e_*` `sl_07_b_*` `sl_08_c_*` `sl_09_*` `sl_10_*` `sl_11_*` `sl_12_*` `sl_15_*` `sl_17_*` `sl_18_*` `sl_19_o_*` `sl_20_s_*` `sl_21_r_*` `sl_23_*`
