# WebFetch WebView移行 — progress

計画: `spec/docs/saaa-webview-webfetch-implementation-plan.md`（2026-09-22）

## HEAD / ベースライン（W0）

- SAAA HEAD: `bdedfbd`（Fix voice capture stop lifecycle）
- llm-fetch HEAD: `d806b17`（fix: distinguish technical descriptions from guard directives）
- 既存dirty（本作業開始前から存在・巻き戻し禁止）: artifact-viewer系の変更
  （`src-tauri/src/artifact_preview/` untracked、`ipc_contract.rs`、`command_registry.rs`、
  `quality_eval.rs`、`test_state.rs`、`capabilities/default.json` ほかのM）。
  作業中に別セッションとみられるartifact_preview編集が継続しており、
  一時的に `cargo check --lib` が E0364/E0433 で失敗する（本計画の範囲外）。
- sidecar baseline: `bundled_sidecar_executes_the_llm_fetch_protocol` PASS
  （`http://127.0.0.1/private` → `UNSAFE_URL`、IP非漏洩）。
- sidecar binary: `src-tauri/resources/bin/webfetch` 約60MiB（2026-09-22時点）。
- web_fetch deterministic tests（移行前）: 2 passed（definitions, envelope projection）。

## カード別記録

| ID    | 変更ファイル                                                                                                                                                                                   | 試験                                                    | 状態                                             |
| ----- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------- | ------------------------------------------------ |
| WF-00 | evidence 2件新規                                                                                                                                                                               | sidecar baseline PASS                                   | 完了                                             |
| WF-01 | llm-fetch `config.rs`（`debug_worker_visible/devtools`+release拒否）、SAAA `lib.rs`（`SAAA_LLM_FETCH_DEBUG_WINDOW=1`+debugのみBuilder override）、`tauri.conf.json`（製品上限。debug表示なし） | config test 7 passed（新規1）                           | 実装・offline完了。visible実WebView未実施        |
| WF-02 | llm-fetch `webview.rs`（presentationのみ切替）、`README.md`                                                                                                                                    | —                                                       | 実装完了。visible/hidden一致は未実施             |
| WF-03 | SAAA `web_fetch/contracts.rs`（strict input/Backend/Cancel/safe error）、`content.rs`投影、`search.rs`投影                                                                                     | contracts 3、content 2、search 5 passed                 | 完了（offline）                                  |
| WF-04 | SAAA `content.rs`（`TauriWebViewContentFetcher` one-shot）、`lib.rs` setupでmanager接続                                                                                                        | fake E2E 1 passed                                       | 実装完了。実WebView未実施                        |
| WF-05 | `content.rs`（select!/cancel/cleanup budget）、`contracts.rs`（`bridge_run_cancellation`）、`app_state.rs`（`cancelled`→`pub(crate)`）、`agent_dispatch.rs`配線                                | fake cancel path（`is_cancelled` fast path）のみ        | 実装完了。registry残存0の実証は未実施            |
| WF-06 | `mod.rs` dispatcher（開始前選択・途中fallbackなし）、`sidecar.rs`隔離                                                                                                                          | sidecar baseline PASS                                   | 完了（offline）。`auto`=macOS webview/他 sidecar |
| WF-07 | `search.rs`（DDG HTML/Lite bounded POST+手書きanchor parser、Brave JSON）                                                                                                                      | fixture 2 passed                                        | offline完了。live canary未実施                   |
| WF-08 | `search.rs`（URL filter/重複排除/rank/上限）、llm-fetch `security.rs`+`lib.rs`（`inspect_plain_text_bounded`公開wrapper）                                                                      | url filter/guard 2 passed、plugin wrapper test 1 passed | offline完了                                      |
| WF-09 | `search.rs`（keyがある場合のみBrave、429→RATE_LIMITED、keyなし送信0）                                                                                                                          | Brave fixture parse passed                              | offline完了（送信0はコード構造上。live未実施）   |
| WF-10 | `mod.rs`（両tool接続・schema変更0）、`agent_dispatch.rs`                                                                                                                                       | fake fetch E2E passed、definitions一致                  | 実装完了。会話E2E未実施                          |
| WF-11 | 未実施（`llm-fetch:default` 維持）                                                                                                                                                             | —                                                       | 残件。webview E2E後に削除                        |
| WF-12 | 未実施（sidecar build/resource/npm維持）                                                                                                                                                       | —                                                       | 残件。W4 gate後に撤去                            |
| WF-13 | 未実施                                                                                                                                                                                         | —                                                       | 残件                                             |
| WF-14 | 未実施                                                                                                                                                                                         | —                                                       | 残件                                             |
| WF-15 | 本ファイル + results.md                                                                                                                                                                        | —                                                       | 進行中                                           |

## 依存メモ

- SAAA `src-tauri/Cargo.toml` は開発中のみ path 依存
  （`../../llm-fetch/crates/tauri-plugin-llm-fetch`）に切替。
  受入時に再現可能なcommit固定へ戻すこと。`Cargo.lock` 更新済み。
- llm-fetch側の変更は未commit（6 files、+131/-2）。
- SAAA側の新規 `src-tauri/src/runtime/web_fetch/` は untracked、
  `src-tauri/src/runtime/web_fetch.rs` は削除（D）。

## 2026-09-22 追加検証（第2ラウンド）

- live到達性: 同一NWからDDG HTMLは202+anomaly challenge、`curl`のLiteは200+12件。
  Rust移植の初版（HTML→Liteのbare `q` POST）は`BOT_CHALLENGE`で失敗、
  一方sidecarは成功 → parity差分と確定。
- 原因: sidecarはWeb経路（bootstrap GET→VQD→`/d.js`→`pageLayout.load`）優先、
  POST formは`q+kp=-1`、`Accept-Language`付き。Rust側へ同順序・同form・
  Web payload parser（bracket matcher付）を移植 → live canary PASS（約8秒）。
- URL正規化をTS parity化（uddg unwrap/hash除去/trailing slash/`utm_*`除去）。
  広告URL（`y.js`/`aclick`）はsidecar同様にhitとして返す（parity維持）。
- 実WebView hidden fast smoke（example）: PASS（exit 0、macOS supported×4）。
- visible（`LLMFETCH_DEBUG_VISIBLE=1`+debug）: PASS、hiddenと2 checkpoint完全一致（W1）。
- one-shot 100回 lifecycle（`--self-test-leak`）: PASS（100/100、exit 0）。
- SAAAスタックE2E（`src-tauri/src/bin/webfetch_e2e.rs`新設、要network+WebView）:
  初回で契約差を検出 — pluginは`timeout_ms` 500..=30s・`maxChars` 1000..を要求、
  model契約（deadline最長・200〜）と不一致 → adapterで吸収
  （timeoutは30s clamp、maxCharsは1000 floor+投影時trim+`truncated`反映）。
  再実行待ち（別作業のworktree破損でlib全体がcompile不可のため一時停止中）。
- WF-11完了: frontend参照ゼロを確認し`llm-fetch:default`をmain capabilityから削除。
- llm-fetch: lib 43 passed、clippy warning 0（redundant closure修正済み）。

1. ~~visible/hidden実WebViewの結果一致~~ → 完了（2 checkpoint完全一致、W1）。
2. ~~100回lifecycle~~ → 完了（100/100）。10分完走は環境DNSで未達（上記）。
   Cookie分離・終了時残存0はplugin boundary suiteの既存PASSを引用、SAAA経路では未単独測定。
3. ~~live search canary~~ → 完了（Web経路でlive PASS、約8秒）。Brave liveはkeyなしのため未実施（送信0は構造上担保）。
4. Windows/Linux実機評価 → **ブロッカー**: 実機なし。`auto`は非macOSでsidecarへfallback実装済み。
5. 全gate:
   - `cargo test -p tauri-plugin-llm-fetch` 43 passed、clippy 0 warning → 完了。
   - SAAA `web_fetch` 14 passed、自ファイルclippy 0・rustfmt clean → 完了。
   - `size:check`自文件 → 完了（残failは他作業分）。
   - `bun run check:local`全量 → 未実行（他作業の破損を含むため、スコープ外として記録のみ）。
   - `cargo clippy -D warnings`全量 → 同上。
6. **ブロッカー（要push権）**: path依存→git rev固定の戻し。
   llm-fetch変更は未commit（`git status`で6 files + example main.rs）。
   commit/pushは明示指示なし＋read-only制約のため未実施。

## 2026-09-22 追加検証（第3ラウンド）

- SAAAスタックE2E（`src-tauri/src/bin/webfetch_e2e.rs`）: **PASS**（exit 0）。
  live `web_search` → `https://rust-lang.org/` → 実worker `fetch_content`（2044 chars）、
  すべてuntrusted/tainted、hidden既定。W2/W4/W10の会話直前ゲート相当。
- 契約差の修正: plugin要求（`timeout_ms` 30s以下、`maxChars` 1000以上）に対し
  adapterで吸収（timeout 30s clamp、1000 floor＋投影trim＋`truncated`反映）。
- WF-12完了（macOS）: `build.rs`はmacOSでBun compile/stagingをskip
  （"staging skipped"警告で実証、`resources/bin/webfetch`再生成なし）。
  ロールバック実証: backup binary復元→両toolが同一schemaで成功→再撤去。
  sidecar unit testはbinary不在時にfail-safe（unavailable）断言へ更新。
- `pub mod runtime`化に伴うprivate-interfaces警告を`pub(crate)`化で解消。
  exampleのdebug-visible分岐に実行マーカー追加（visible採取の証拠）。
