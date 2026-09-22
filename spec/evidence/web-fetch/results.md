# WebFetch WebView移行 — 最終検証結果

- 検証日: 2026-09-22
- SAAA実装commit: `e0063bd`（後続の依存固定commitを本書末尾に記録）
- llm-fetch固定revision: `64afbce5e99d24475fd25fceb9bd187a9777599b`
- llm-fetch PR: https://github.com/ugnoguchigxp/LLM-fetch/pull/17

## 結論

macOS 14以降では、Rust検索とTauri WebView content fetchを既定経路として使用できる。workerは既定で非表示であり、debug buildだけ明示設定で表示できる。Windows/LinuxはWebViewを既定化せず、既存Bun sidecar fallbackとtarget別資材を維持する。

## 修正した問題

- `deny` / `require_approval`を`allow_with_warning`へ弱めていたcontent投影を修正。
- 検索hitをguardで除外した場合もwarning categoryを保持し、全件blockは`deny`、一部blockは`allow_with_warning`にした。
- HTTP responseを一括読込せず、2 MiBを超える前にstreamを停止する実上限へ変更。
- 固定検索endpointのredirectを禁止し、Brave credentialの別origin転送を防止。
- DuckDuckGo HTML parse失敗時もLiteへfallbackし、Brave件数をmodel指定limitへ一致。
- DDG redirect URLのUTF-8 percent decodeを修正。
- `WebFetchCancel`のNotify lost-wakeup raceを解消。
- WebView fetchをowned task化し、応答期限後もplugin cleanupが完了するようにした。
- sidecar processにもcancelを伝播し、cancel/timeout時は`kill_on_drop`で終了するようにした。
- DNS/Proxy failureを`UNSAFE_URL`へ誤分類せず、retryableな安全コードへ分離。
- 製品のmacOS minimum system versionを14.0へ明示し、`auto`判定と配布条件を一致。
- 検索provider初期化失敗を黙殺せず、Tauri setup errorとして扱うようにした。

## 合格した検証

- llm-fetch unit: 43 passed。
- llm-fetch workspace clippy: all-targets/all-features、warning 0。
- Rust 1.90.0 workspace/all-targets check: PASS。
- rustdoc `-D warnings`: PASS。
- Cargo package: 38 files、Node/TypeScript/別browser binary混入なし。
- GitHub required CI: Node 22/24、Bun、Chromium sandbox、Windows packed consumer、Cargo、macOS hidden WebView smoke/boundaryが全てPASS。
- visible/hidden debug comparison: 2 checkpointが同一結果。
- one-shot lifecycle: 100/100、終了時registry/window残存なし。
- reusable lifecycle: 100/100、終了時registry/window残存なし。
- hidden background 10分: 0/2/6/10分すべて同一session・同一127文字・`decision=allow`。終了時one-shotとcleanupもPASS。
- Rust live search canary: PASS。
- SAAA WebFetch offline: 20 passed、1 ignored（live canary）。
- SAAA dispatcher→Rust search→実WebView content fetch E2E: PASS。

## DNS_FAILUREの扱い

過去2試行は3秒のDNS期限で一過性`DNS_FAILURE`となった。直後のOS resolver/curlは正常だった。製品設定とlong testのDNS期限を10秒へ揃え、30秒のrequest deadline内で0/2/6/10分を完走した。無制限retryは追加していない。

## platform判定

- macOS 14+: WebView既定、macOS bundleからWebFetch sidecarを除外。
- Windows/Linux: sidecar既定を維持。GitHubのWindows consumerとSAAA platform compile/smokeで継続検証する。
- Windows/Linuxの実WebViewは正式経路にしていないため、本移行のacceptance条件には含めない。

## rollback

`SAAA_WEBFETCH_BACKEND=sidecar`を設定し、対象platformの`webfetch` binaryを配置すれば次のtool callからsidecarへ戻せる。tool開始後の途中fallbackは行わない。
