# M4A 改修・受入進捗

2026-09-21。実装・offline受入は完了。全体リリース判定は `m4a-results.md` の残件を参照。

| 範囲 | 実装・証拠 |
| --- | --- |
| E00 前提改修 | 元TTLの再検証、本文存在とreceipt一致、AgentSession最終境界、MCPのevidence採用記録 |
| E01/E02 runner・report | `scripts/world-eval.ts`、欠落/失敗時complete=false、32ケース |
| E03〜E07 | 本番Frame→Broker→OpenAI互換HTTP。TTL、owner、mixed block、予算 |
| E08 | AgentSession初回/継続、失効、失敗したprimaryから切替。WD契約へ更新 |
| E09〜E12 | Source/Scope、並行fixture、完了時TTL、shadow拒否 |
| E13 | 五要素の実clock・100投影/2,000ledger測定 |
| E14 | 対象Rust/core/packages/IPCを実行。全体失敗を分離 |
| E15 | report、results、review、旧計画のAgentSession条件を更新 |

個々の試験の範囲（特にfallback設定解決、独立DB並行、live Provider）をresultsに明記した。未検証経路をstub成功で埋めていない。
