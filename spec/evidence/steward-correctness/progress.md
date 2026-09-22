# Steward 整合性 — progress

着手時 HEAD: `bdedfbd4b53eebf429c6b547ca3d66ac4bcb30d0`（Fix voice capture stop lifecycle）。
その時点の dirty は artifact viewer / web fetch / voice / schedule など別作業で、steward モジュールは未変更だった。

確認時点の HEAD は `314625d`。C1〜C6 は着手時のコードに残っていた。外したカードはない。

| カード | 状態 | 変更 |
| --- | --- | --- |
| SC-00 | 完了 | C1〜C6 は計画の関数に残っていた。対象は SC-01〜SC-06 |
| SC-01 | 完了 | `reduce.rs` の publish / continue、`report.rs` の `flush_held_reports` を `SqliteWriter::transact` にした。TTS と UI emit は commit の後 |
| SC-02 | 完了 | `remaining_deadline_ms` はミリ秒文字列の差。dispatch が `budget::arm` し、runner の待受は `wait_budget`。委任実行の 1800 秒固定は外した。非委任の待受上限は残す |
| SC-03 | 完了 | `request_for_task` が Goal の summary と step recipe を依頼文に含める |
| SC-04 | 完了 | `next_eligible` は `ORDER BY t.queue_rank, t.rowid` |
| SC-05 | 完了 | forget の未配送報告は、その source の task に限る |
| SC-06 | 完了 | Goal 未指定の撤回は `goal_required`。指定 Goal の job を先に cancel し、`withdraw_goal` する。画面の「最新の Goal を撤回」は外した |
| SC-07 | 完了 | 下の results を参照。module 予算の上限は上げていない |

`repository.rs` の `withdraw`（最新 Goal を superseded する経路）は、既存の lineage 試験が呼ぶため残している。本番 IPC からは呼ばない。
