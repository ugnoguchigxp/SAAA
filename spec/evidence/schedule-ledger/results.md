# Butler Schedule Ledger 検証結果

作成日: 2026-09-21。計画: `spec/docs/saaa-butler-schedule-ledger-plan.md`。

## 実装済み（default OFF）

- `schedule_entries` が期限の正本。tick は 45s、起動時に一回。Calendar 時刻は契機にしない。
- `delegation_ref` が無い entry は Ask。会議中（既存 TTS hold）は Hold し、終了後に件数だけのダイジェスト。
- 投影は outbox。観測は `calendar_observations` 経由。token は Keychain / メモリ。SQLite に refresh を書かない。

## A〜S

| ID | 結果 |
| --- | --- |
| A〜G, K, L, M, O, R, S | offline合格 |
| H, I, N, P | fake Calendar HTTP で合格。live 未実施 |
| J, Q | fake 上の分類・remote_deleted 経路。live 未実施 |

## ゲート

- `cargo test --locked --manifest-path src-tauri/Cargo.toml --lib schedule::`: 32 passed / 0 failed（隔離 target。2026-09-21）
- due 1,000 件の p95 と tick p95 は試験内で確認
- live Google 確認（SL-24）は未実施。予定名・メールは記録していない

## 未実装のまま引き渡す

Gmail センサー、Discord チャネル、Docs 一枚、Assist Planner 候補生成。
