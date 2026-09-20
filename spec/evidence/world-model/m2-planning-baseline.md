# World Model M2 計画作成時の確認

確認日: 2026-09-20。HEAD: `f5a1069`。これは計画の根拠であり、M2実装の完了記録ではない。

## 実行した試験

| コマンド | 結果 |
| --- | --- |
| `cargo test --locked --manifest-path crates/personal-state-core/Cargo.toml` | 81 passed、0 failed。lib38、continuity17、world_contract2、world_traversal9、world_v2_validation15。doc-test0 |
| `cargo test --locked --manifest-path src-tauri/Cargo.toml --lib memory::personal_state::world` | 51 passed、0 failed、1 ignored、723 filtered out |

性能試験、全check:local、全Rust package試験、live Provider試験はこの計画作成では実施していない。旧報告の全体ビルド不能を現行HEADの問題として転載しない。

## 実装から確認した制約

- `sources::load / revalidate` は会話Sourceに依存する。Runtime / Externalのenumがあっても、会話以外の正本の再検証は完成していない。
- `store::load` は全ledger表を読む。探索上限だけでは読取費用を制限できない。
- `scope::resolve` は書込みを行う。読取経路では既存のrecorded scopeをloadして現在epoch・直接リンクを検査する必要がある。
- MeetingRuntimeの既存snapshotにはcapture token等が含まれる。World専用の最小snapshotが必要。再起動時の整理関数は `meeting::reconcile`。
- codingのrevisionだけではすべてのstate / delivery変更を検出できない。正本の状態tupleによるdigestも必要。
- 現行contextStill memory-recall-v1は安定Source ID・版・削除照会・Scope証明を返さない。既存parserは未定義sourceRefも拒否する。
- SqliteReadersのSerialized試験backendと製品のPersistent backendはtransactionの扱いが異なる。snapshot整合性試験には一時file DBを使う。

## 前段報告との差

`v2-progress.md / v2-results.md` はD00〜D42完了を報告するが、全check:localは未実施と記載する。D41はwarm-upなし・5 sampleで、1,000件は構築途中に中断している。規定の性能認定とは区別し、M2-01で試験と記録を補う。今回の計画作成では性能値を再測定していない。

このためM2Aは現在状態をその都度読む内部WorldFrame、M2Bは再検証可能な会話外Sourceによる永続更新、と分割した。通常会話とBrokerへの接続はM3で行う。

## 文書と実装の境界

今回の変更はM2計画・固定契約・30枚の作業カードと既存文書からの案内だけ。productionコード、DB、公開APIは変更していない。計画中の新規型・関数・試験は実装予定であり、存在確認済みAPIではない。
