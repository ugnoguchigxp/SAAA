# Artifact WebView Go / No-Go — 2026-09-22

## Decision

**Go。** 評価者が 3 OS live matrix を Go 条件から外した。macOS 上の実装を、同梱 Interactive HTML fixture のプレビューとして使える状態にする。

Windows / Linux の実機、DPI、複数モニタ、100 回切替のメモリ実測は未実施。評価中に問題が出たらその項目だけ止める。

## 使える範囲

- 作業画面のツールバー「対話HTMLプレビューを開く」から、右側 Artifact パネルに fixture を開く。
- Semantic UI タブと切り替えても、子 WebView は同時に 1 つ。
- 権限は webview label `main` のみ。プレビュー label に Capability は付けない。
- HTML 本文は IPC で返さず、短命 token の `saaa-artifact-preview` から読む。

## まだ本番の正本ではない

Content Artifact の SQLite はない。表示対象は `artifact_preview/catalog.rs` の fixture。`webview_artifacts` テーブルは作っていない。
