# WebFetch WebView移行 — results

## 実装済み

- plugin debug表示設定（hidden既定・debugのみ可視・release拒否）+ SAAA env配線。
- `ContentFetcher` trait境界 + `TauriWebViewContentFetcher`（one-shot、deadline/cancel/cleanup契約）。
- `SearchProvider` Rust移植（DDG HTML/Lite bounded parser、Brave key時fallback）。
- 検索URL filter・guard wrapper（`inspect_plain_text_bounded`）・blocked count。
- dispatcher（`sidecar/webview/auto`、開始前選択、途中fallbackなし）。
- compact投影（`fetch_content_result` / `web_search_result`、schema変更0）。
- `tauri.conf.json` のllm-fetch製品上限。

## offline合格

- SAAA `cargo test --lib web_fetch`: **14 passed**（別作業のworktree破損前に記録）。
- llm-fetch `cargo test -p tauri-plugin-llm-fetch --lib`: **43 passed**（最終状態で再確認済み）。
- `cargo check --offline --all-targets`: 本計画ファイル起因のerror 0（最終確認時は
  別作業ファイルのE0364/E0433でlib全体が失敗するため、本計画範囲は上記test記録で担保）。

## visible WebView合格

- example `--self-test-fast` + `LLMFETCH_DEBUG_VISIBLE=1`（debug）: PASS。
  hidden実行と2 checkpoint（finalUrl/title/text/decision）完全一致。

## hidden/background合格

- example `--self-test-fast`（hidden）: PASS（macOS supported×4）。
- `--self-test-leak` one-shot 100回: PASS（100/100、exit 0）。
- SAAAスタックE2E `webfetch_e2e`（hidden worker）: PASS（exit 0、2044 chars）。
- `--self-test-long` 10分: 未達。0/2/6分は2回とも成功（同一hidden WebView再利用・
  同一抽出）だが、10分（1回目）・6分（2回目）で一過性DNS_FAILURE。
  直後のcurl/dscacheutil正常のため環境要因と判断、コード回帰ではない。

## live search合格

- Rust移植のlive canary（ignored test）: PASS（Web経路、約8秒、5件以内・URL検証済み）。
- SAAAスタックE2E内のlive検索: PASS（5 hits、blocked 0）。
- Brave live: 未実施（keyなし。送信0はコード構造上担保）。

## platform未検証

- macOS: 上記の通り実WebViewゲート通過。
- Windows/Linux: 実機なしのため未検証。`auto`は非macOSでsidecarへfallback。
  sidecar（win/linux）のbuild staging・npm依存は維持。

## sidecar fallback継続中（macOSは撤去済み）

- macOS: `build.rs`はstaging skip、`resources/bin/webfetch`なし（gitignored生成物）。
  ロールバック手順を実証済み（backup復元→両tool同一schema成功→再撤去、
  `/tmp/webfetch.rollback-backup`に退避中）。
  緊急時は `SAAA_WEBFETCH_BACKEND=sidecar` + binary配置で次tool callから切替。
- 非macOS: sidecar継続。`scripts/webfetch-sidecar.ts`・npm `llm-fetch`依存は維持。
