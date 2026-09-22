# WebFetch WebView移行 — 実装進捗

計画: `spec/docs/saaa-webview-webfetch-implementation-plan.md`

| ID | 内容 | 状態・証跡 |
|---|---|---|
| WF-00 | ベースラインと証跡 | 完了 |
| WF-01 | debug表示設定、hidden既定、release拒否 | 完了。visible/hidden比較PASS |
| WF-02 | worker presentation切替 | 完了 |
| WF-03 | strict input、backend、safe error、compact投影 | 完了。判定downgrade修正済み |
| WF-04 | `TauriWebViewContentFetcher` | 完了。実WebView E2E PASS |
| WF-05 | deadline/cancel/cleanup | 完了。lost wakeup・detached cleanup・sidecar cancel修正 |
| WF-06 | call開始前backend選択 | 完了。途中fallbackなし |
| WF-07 | DDG Web/HTML/Lite + Brave検索 | 完了。live canary PASS |
| WF-08 | URL filter、guard、blocked count | 完了。全block=`deny` |
| WF-09 | credentialed Brave fallback | 完了。redirect禁止・limit反映 |
| WF-10 | model tool接続、schema維持 | 完了 |
| WF-11 | frontend IPC capability削除 | 完了 |
| WF-12 | macOS sidecar非同梱、rollback維持 | 完了 |
| WF-13 | fast/leak/reuse/10分background | 完了。10分完走 |
| WF-14 | platform方針 | 完了。macOS 14+ WebView、Windows/Linux sidecar維持 |
| WF-15 | documentation/revision固定 | 完了。llm-fetch `64afbce5e99d24475fd25fceb9bd187a9777599b` |

## 最終revision

- llm-fetch: `64afbce5e99d24475fd25fceb9bd187a9777599b`（PR #17、required checks全合格）
- SAAA: `tauri-plugin-llm-fetch`を上記revisionへ固定し、隣接checkout依存を除去。

## 補足

過去の試験履歴と失敗分析はGit履歴に残る。最終的な合否とコマンド結果は`results.md`を正とする。
