# SAAA WebView WebFetch 実装計画 — Rust接続、検索移植、バックグラウンド化

作成日: 2026-09-22。状態: 実装計画。コードは本計画の作成では変更しない。

対象は SAAA と隣接リポジトリ `../llm-fetch`。本書は、既にSAAAへ登録されている `tauri-plugin-llm-fetch` を実際の会話ツール経路へ接続し、現在のBun製WebFetchサイドカーを段階的に置き換えるための正本とする。

開発中は動作確認のためworker WebViewを表示できるようにする。製品動作では非表示を既定かつ必須とし、React画面やメインWebViewをクロールに使用しない。

## 1. 完成させる製品動作

SAAAがモデルへ公開する既存の二つのツールを維持する。

1. `fetch_content`: OS標準WebViewでページを読み込み、JavaScript実行後のDOMから読みやすい本文を取得する。
2. `web_search`: Rustの検索Providerからタイトル、URL、スニペットを取得する。検索結果のURLとテキストも、本文と同じく信用済みデータとして扱わない。

WebViewはTauriのRust backendが所有する専用workerとする。通常は非表示、非focusable、taskbar非表示、incognitoで動作し、メインウィンドウの表示内容、Cookie、認証状態、Tauri capabilityを引き継がない。

取得結果は常に `untrusted=true` 相当、`tainted=true` とする。ページ本文や検索結果に含まれる命令を、SAAAの権限、承認、記憶更新、ツール実行の根拠にしない。

## 2. 現状と不足実装

### 2.1 既にあるもの

| 項目 | 現状 |
| --- | --- |
| Cargo依存 | `src-tauri/Cargo.toml` に `tauri-plugin-llm-fetch` がGit revision固定で存在する |
| Tauri登録 | `src-tauri/src/lib.rs` で `tauri_plugin_llm_fetch::init()` を登録済み |
| capability | main windowに `llm-fetch:default` を付与済み |
| worker | hidden / incognito / proxy固定の専用WebView実装が `../llm-fetch` に存在する |
| 本文抽出 | JavaScript実行後DOMのvisible text、title、final URL、言語、切詰め理由を取得できる |
| 防御 | public network制限、DNS検査、通信量上限、navigation上限、popup/download拒否、content guardがある |
| lifecycle | one-shot、再利用session、cancel、idle cleanup、shutdownが実装済み |
| 現行モデル契約 | `web_search` と `fetch_content` のstrict tool schemaがSAAAにある |

### 2.2 足りないもの

| ID | 不足 | 影響 |
| --- | --- | --- |
| G01 | 会話ツール実行経路からRust plugin managerを参照するadapter | pluginを登録していてもモデルの取得処理には使われない |
| G02 | `fetch_content` 引数と `FetchRequest`、取得結果と既存compact resultの相互変換 | 現行のモデル向けJSON契約を維持できない |
| G03 | Provider deadline、ユーザーcancel、plugin cancel、cleanupの統合 | futureをdropするだけではrequest/session registryが残る可能性がある |
| G04 | worker表示を開発時だけ切り替える設定 | 現在は `visible(false)` が固定で、実画面を見た初期デバッグができない |
| G05 | Rust版 `web_search` | Tauri pluginは本文取得だけで、検索Providerを持たない |
| G06 | 検索結果用のURL検証、サイズ制限、guard、compact projection | サイドカー撤去後も現在の安全境界を維持する必要がある |
| G07 | macOSとWindows/Linuxの切替方針 | pluginの正式対象はmacOS 14以降。Windows/Linuxはbest-effort |
| G08 | 段階的切替とロールバック用のbackend選択 | 一括置換すると取得不能時の比較と復帰が難しい |
| G09 | Bun sidecarのbuild・resource・npm依存撤去条件 | `fetch_content` だけを置き換えても `web_search` のためにsidecarが残る |
| G10 | 実WebViewを含むSAAA側の受入試験 | plugin単体の検証記録はあるが、会話tool経路のend-to-end gateがない |

## 3. 固定する設計判断

### 3.1 worker WebView

- メインWebViewは使わない。クロールごとにplugin所有のworkerを使う。
- 製品既定値は常に非表示とする。
- 初期実装・手動検証ではdebug buildに限り表示可能にする。
- 表示切替で通信、proxy、incognito、navigation、抽出、guardの処理を分岐させない。違ってよいのはwindow presentationとDevToolsだけとする。
- release buildで表示要求が指定された場合は、黙って表示せず起動時に設定エラーとする。
- 表示中でもremote pageへSAAAのIPC権限を付与しない。workerから副作用付きcustom protocolへ遷移させない。

`../llm-fetch` の `Config` に次を追加する。

```rust
pub debug_worker_visible: bool // default false
pub debug_worker_devtools: bool // default false
```

両項目は `cfg!(debug_assertions)` のときだけ `true` を許可する。SAAAでは `SAAA_LLM_FETCH_DEBUG_WINDOW=1` を明示した開発起動だけでBuilder overrideへ反映する。`tauri.conf.json` に `true` を常設しない。

初期の目視確認は表示モードで行い、同じ試験を非表示モードでも通した後に統合完了とする。表示モードだけの成功をバックグラウンド動作の証拠にしない。

### 3.2 Rust backendからの呼び出し

モデルtoolはfrontend IPCを経由せず、Rustから `LlmFetchManager` を直接呼ぶ。plugin登録後のmanagerをSAAAのWebFetch runtimeへ一度だけ渡し、frontend専用の第二状態を作らない。

adapterはテスト可能なtrait境界にする。

```rust
#[async_trait]
trait ContentFetcher: Send + Sync {
    async fn fetch(
        &self,
        request: FetchContentInput,
        deadline: Duration,
        cancellation: RunCancellation,
    ) -> Result<FetchContentResult, WebFetchFailure>;
}
```

実装は `TauriWebViewContentFetcher`、単体試験は `FakeContentFetcher` を使用する。`AppState` の多数のfixtureへTauri型を直接追加しない。

SAAA側には `OnceLock<Arc<WebFetchRuntime>>` 相当の初期化slotを置き、Tauri `setup` 中に `app.llm_fetch().0.clone()` を渡す。toolロジック本体は `execute_with_runtime` を受け取る形にして、試験ではglobal slotを使わずfakeを直接渡す。未初期化、二重初期化、shutdown後の呼出しはpanicではなく安全な `web-fetch-unavailable` にする。

### 3.3 session方針

初期製品経路はone-shotを使用する。任意の公開URLを扱うため、host固定が必要なreusable sessionを会話全体で共有しない。

- 1 tool call = 1 request ID = 1 one-shot worker。
- request終了時は成功・失敗・cancelを問わずworkerを破棄する。
- 同一hostの連続取得を高速化するsession poolは初期範囲外とする。
- debug表示でもone-shotの意味は変えない。必要なら手動検証専用コマンドだけhost限定sessionを使う。

### 3.4 検索

WebViewは本文レンダリングに使う。`web_search` は固定された検索endpointへRust HTTP clientで問い合わせる。検索ページを汎用workerへ読み込ませてDOM全体や任意のhrefを返す方式は採用しない。

理由は、現在のextractorが意図的にhrefを本文へ含めず、検索用href抽出を追加すると汎用取得契約と攻撃面が広がるためである。

初期Providerは現在と同じ順序を維持する。

1. DuckDuckGo HTML/Lite
2. `BRAVE_SEARCH_API_KEY` がある場合だけBrave Searchをfallbackとして利用

Rust移植では `reqwest` を再利用する。HTML parserが必要な場合は直接依存を1件だけ追加し、raw HTMLサイズ、DOM node相当、結果数、title、snippet、URL長を先に制限する。正規表現だけでHTMLを解析しない。

検索結果URLは `http` / `https` 以外、userinfo、literal IP、localhost/local名を破棄する。実際に本文取得するときはWebView pluginのDNS・public network検査をもう一度通す。

検索guardをSAAA側へ複製しない。`../llm-fetch` に、既存のRust guardを使ってbounded plain textを検査する公開関数を追加する。入力はprovider、title、snippet、URLを連結した文字列と `RequestedContextUse::AnswerWithCitation` に限定し、DOM segment型や内部ruleは公開しない。各hitについて `deny` / `require_approval` を除外し、除外数を `blockedResultCount` に反映する。全hitのdecisionとwarning categoryのmerge規則は現行TypeScript実装のfixtureで固定する。

### 3.5 現行契約の維持

モデルへ見せるtool名とschemaは変えない。

`fetch_content` の出力:

```json
{
  "type": "fetch_content_result",
  "security": {
    "trust": "untrusted",
    "tainted": true,
    "decision": "allow_with_warning",
    "warningCategories": []
  },
  "document": {
    "url": "https://final.example/",
    "text": "...",
    "fetchedAt": "...",
    "truncated": false
  }
}
```

`web_search` の出力も現在の `web_search_result`、`hits`、`blockedResultCount` を維持する。plugin固有のsession ID、worker label、proxy URL、内部stage、raw例外はモデルへ返さない。

## 4. タイムアウトとキャンセル契約

外側のProvider deadlineとplugin内部timeoutを競合させない。

1. tool開始時に残りdeadlineを計算する。
2. pluginの `timeout_ms` は残りdeadlineからcleanup予算を引いた値とする。
3. `RunCancellation`、外側deadline、plugin resultを `tokio::select!` で待つ。
4. cancel/deadline時は、生成したrequest IDで `manager.cancel()` を呼ぶ。
5. cancel受理後は短いcleanup deadline内でfetch futureの終了を待つ。
6. cleanup未確認なら同じ操作を自動再実行せず、`web-fetch-unavailable` と診断ログを残す。

fetch futureを単にdropして完了扱いにしてはならない。`active_requests=0`、one-shot session数=0、worker window registry残存なしまでをcleanup成功とする。

request IDはSAAA内部でUUIDから生成し、モデル入力として受け取らない。同じtool callのretryは新しいrequest IDとし、元requestのcleanup確認後に限る。

## 5. backend切替とプラットフォーム

内部設定として次のbackendを持つ。利用者向け設定画面は初期範囲外とする。

| 値 | 動作 |
| --- | --- |
| `sidecar` | 現行実装。ロールバック用 |
| `webview` | Rust検索 + WebView本文取得。未対応platformでは明示エラー |
| `auto` | macOS 14以降はwebview、それ以外はsidecar |

初期既定は `auto`。macOSの受入完了後も、Windows/Linuxの実WebView gateが通るまではsidecarを残す。macOS bundleではRust検索移植完了後に `webfetch` binaryを同梱しない。Windows/Linux向けbuildではfallbackが必要な間だけtarget別に同梱する。

同一tool callの途中でwebview失敗後にsidecarへ自動再実行しない。ページ側に副作用があり得るためである。fallbackは呼出開始前のplatform/backend選択だけで行う。

## 6. 変更ファイル

### 6.1 `../llm-fetch`

| ファイル | 変更 |
| --- | --- |
| `crates/tauri-plugin-llm-fetch/src/config.rs` | debug表示・DevTools設定、release拒否validation |
| `crates/tauri-plugin-llm-fetch/src/webview.rs` | window presentationだけを設定値で切替 |
| `crates/tauri-plugin-llm-fetch/src/webview/tests.rs` | hidden既定、debug可視、release拒否可能部分の試験 |
| `crates/tauri-plugin-llm-fetch/src/security.rs` / `src/lib.rs` | bounded plain text guardの狭い公開wrapper。内部rule/DOM segmentは非公開のまま |
| `crates/tauri-plugin-llm-fetch/README.md` | debug表示方法、製品では非表示であることを記載 |
| `docs/TAURI_PLUGIN_VALIDATION.md` | visible/hidden双方の実WebView結果を追記 |

SAAAが固定しているrevisionは、現在の隣接リポジトリHEADより1件古い。実装開始時にsecurity guard修正を含むcommitを選び、Git revisionを更新する。開発中だけpath dependencyへ切り替えてよいが、受入時は再現可能なcommit固定へ戻す。

### 6.2 SAAA Rust

| ファイル | 変更 |
| --- | --- |
| `src-tauri/src/runtime/web_fetch.rs` | sidecar単体実装をdispatcherとcompact projectionへ分割 |
| `src-tauri/src/runtime/web_fetch/content.rs` | `ContentFetcher`、plugin adapter、入力検証、cancel統合 |
| `src-tauri/src/runtime/web_fetch/search.rs` | `SearchProvider`、DuckDuckGo、Brave fallback、bounded parser |
| `src-tauri/src/runtime/web_fetch/contracts.rs` | 入出力型、safe error、backend enum |
| `src-tauri/src/runtime/web_fetch/sidecar.rs` | 移行期間だけ現行process実装を隔離 |
| `src-tauri/src/lib.rs` | plugin managerをruntimeへ接続、debug設定、不要なresource解決の段階削除 |
| `src-tauri/src/providers/stream/agent_dispatch.rs` | `RunCancellation` をWebFetch executeへ渡す |
| `src-tauri/Cargo.toml` | plugin revision更新。必要なHTML parserのみ直接依存追加 |
| `src-tauri/build.rs` | target/backendに応じたsidecar生成。最終的にmacOS生成を削除 |
| `src-tauri/tauri.conf.json` | plugin上限を明示。debug表示は書かない |
| `src-tauri/capabilities/default.json` | backend直呼び完成後、main windowの `llm-fetch:default` を削除 |

`AppState`、DB schema、frontend状態、公開IPCは原則変更しない。plugin managerを保持するために既存fixtureの大量変更が必要になる設計は避け、runtime adapterの初期化slotを使う。

`tauri.conf.json` には少なくとも次を固定する。モデル契約の上限が20,000文字なのでplugin側も同じ上限にし、HTTPは許可しない。

```json
{
  "plugins": {
    "llm-fetch": {
      "allowedHosts": ["*"],
      "maxSessions": 2,
      "maxQueueDepth": 32,
      "requestTimeoutMs": 30000,
      "sessionIdleTimeoutMs": 300000,
      "maxCharacters": 20000,
      "requireReliableBackground": true,
      "allowHttp": false
    }
  }
}
```

`allowedHosts: ["*"]` はpublic network全体という意味であり、private/local宛てを許可する意味ではない。再利用sessionにはwildcardを渡さない。

### 6.3 SAAA TypeScript・resource

| ファイル | 変更 |
| --- | --- |
| `scripts/webfetch-sidecar.ts` | 移行期間のみ維持。Rust parity後に削除 |
| `tests/webfetch-sidecar.test.ts` | sidecar期間中は維持し、撤去commitでRust契約試験へ置換 |
| `package.json` / `bun.lock` | 他用途がなければ `llm-fetch` npm依存を削除 |
| `src-tauri/resources/bin/webfetch*` | 生成物。手編集せず、target別build撤去後に消えることを確認 |

## 7. 実装マイルストーン

### W0: ベースラインと固定fixture

- 現行sidecarで `web_search`、静的HTML、JavaScript描画ページ、redirect、unsafe URL、timeout、cancelを記録する。
- compact JSON fixtureを保存し、フィールド名とエラーcodeを固定する。
- 現在のbinaryサイズ、1回取得時間、10回連続時のprocess数とメモリを記録する。
- dirty worktreeの既存変更を記録し、本作業で巻き戻さない。

合格: fixture件数が0ではなく、現在のsidecar試験が通る。

### W1: debug表示切替

- pluginへdebug表示設定を追加する。
- debug build + 明示環境変数でworkerを表示する。
- release buildでは表示設定を拒否する。
- 表示・非表示で同じURL、本文、final URL、guard decisionが返ることを確認する。

合格: visibleとhiddenの結果差がwindow presentation以外にない。hidden時にfocus/taskbar/window registry残存がない。

### W2: `fetch_content` Rust接続

- `ContentFetcher` とplugin adapterを実装する。
- strict入力検証、`FetchRequest` 変換、compact projectionを実装する。
- deadline/cancel/cleanup契約を接続する。
- backend設定で `fetch_content` だけwebviewへ切り替え可能にする。

合格: static、JS描画、redirect、unsafe URL、timeout、cancel、連続100回で期待契約を満たす。モデル向けschemaは変更0。

### W3: `web_search` Rust移植

- fixed endpoint HTTP client、bounded response reader、HTML parserを実装する。
- DuckDuckGo HTML/Liteのfixture parserを移植する。
- Brave keyがある場合のfallbackを実装する。
- URL filter、重複排除、rank、文字数制限、guard、blocked countを実装する。
- networkを使わないparser試験と、明示的なlive canaryを分ける。

合格: committed fixtureで現行compact resultと意味的に一致する。challenge、parse変更、429、timeout、oversizeがtyped errorになる。

### W4: macOS sidecar撤去・hidden既定化

- `auto` のmacOS選択をwebviewにする。
- macOS buildから `stage_web_fetch_runtime` とresource解決を外す。
- main windowのplugin capabilityを削除する。
- npm `llm-fetch` が他用途で不要なら削除する。
- debug表示なしで全gateを再実行する。

合格: macOS app bundleに `bin/webfetch` がなく、Bun WebFetch compileなしでbuildできる。会話から両toolが成功する。

### W5: Windows/Linux判定

- 実機でhidden background、proxy強制、Cookie分離、100回lifecycleを実行する。
- 合格platformだけ `auto -> webview` へ変更する。
- 不合格platformはsidecar fallbackを維持し、未対応を明記する。

合格前に全platformからsidecarを削除しない。

## 8. 作業カード

| ID | 対象 | 実装 | 完了条件 |
| --- | --- | --- | --- |
| WF-00 | evidence | HEAD、dirty、現行fixture、性能、binaryサイズを記録 | sidecar baseline成功 |
| WF-01 | plugin config | debug visible/devtoolsとrelease validation | default hidden、release可視化不可 |
| WF-02 | plugin worker | presentation切替、security経路共通化 | visible/hidden結果一致 |
| WF-03 | runtime contracts | strict input、compact output、safe failure | snapshot fixture一致 |
| WF-04 | content adapter | manager参照、one-shot fetch | static/JS/redirect成功 |
| WF-05 | cancellation | outer cancelからplugin cancel、cleanup await | registry/window残存0 |
| WF-06 | dispatcher | `sidecar/webview/auto` の開始前選択 | 途中fallback 0 |
| WF-07 | DDG client | bounded request、HTML/Lite parser | offline fixtures成功 |
| WF-08 | search guard | URL filter、text guard、blocked count | unsafe hitが結果に出ない |
| WF-09 | Brave fallback | credential時だけ有効、rate limit処理 | keyなしで送信0 |
| WF-10 | tool integration | 両toolをRust backendへ接続 | schema変更0、E2E成功 |
| WF-11 | permissions | frontend capability削除 | main windowからplugin invoke拒否 |
| WF-12 | macOS packaging | sidecar build/resource/npm依存整理 | bundle内webfetch 0 |
| WF-13 | background gate | hidden fast/leak/reuse/long | focus/taskbar/残存window 0 |
| WF-14 | cross-platform | Windows/Linux実機評価 | 合格platformだけ既定変更 |
| WF-15 | closeout | 結果、残件、rollback確認 | 実装済み/未検証を分離報告 |

## 9. 試験計画

### 9.1 deterministic

- Rust input validation: unknown field、空文字、文字数、整数範囲。
- compact projection: title等のplugin内部情報を過剰にモデルへ返さない。
- search parser: committed DuckDuckGo HTML/Lite fixture。
- unsafe URL: localhost、private IP、userinfo、explicit port、非HTTP scheme。
- guard: hidden instruction、tool invocation、benign mention、切詰め時fail-closed。
- cancellation: queued、navigation中、extraction中、cleanup timeout。
- backend selection: OS/version/configのtruth table。
- release configuration: debug visible拒否。

### 9.2 実WebView

- debug visible fast smoke。
- hidden fast smoke。
- one-shot 100回。
- reusable 100回はplugin回帰として維持するが、SAAA製品経路はone-shotを確認する。
- 10分background取得。
- cross-origin subresource拒否、redirect上限、network budget。
- Cookieが別workerへ漏れない。
- SAAA終了時にactive request/session/windowが0。

### 9.3 会話tool E2E

- モデルが `web_search` を呼び、返ったURLを `fetch_content` で読む。
- tool resultが `untrusted/tainted` のままProviderへ戻る。
- cancel操作で会話runとworkerの両方が停止する。
- unsafe URLの安全なエラーに、入力URLや内部proxy情報を含めない。
- WebView表示中でもメイン画面の会話、Cookie、focusが変化しない。

### 9.4 コマンドゲート

実装時点の正確なfilter名をprogressへ記録し、0件実行を合格にしない。

```sh
cargo test -p tauri-plugin-llm-fetch
cargo test --locked --manifest-path src-tauri/Cargo.toml --lib web_fetch
cargo check --locked --manifest-path src-tauri/Cargo.toml --all-targets
cargo clippy --locked --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
bun run check:local
bun run size:check
```

実WebViewとlive searchはoffline gateと分け、環境・時刻・OS/WebView versionを記録する。public canaryの変化による失敗をコード回帰と混同しない。

## 10. 受け入れ条件

macOS段階は以下をすべて満たして完了とする。

1. `web_search` と `fetch_content` の既存schema・出力形を維持する。
2. JavaScript描画後の本文が取得できる。
3. 製品起動ではworkerが表示されず、focus、Dock/taskbar、always-on-topへ影響しない。
4. debug buildでは明示設定時だけworkerを表示できる。
5. 表示・非表示で抽出・guard・network policyが同一である。
6. cancel/timeout/終了後にactive request、session、worker windowが残らない。
7. 全結果がuntrusted/taintedであり、guardが権限を付与しない。
8. private network、localhost、literal IP、許可外navigationを拒否する。
9. main windowからplugin IPCを直接呼べない。
10. macOS app bundleにWebFetch用Bun binaryを含めず、WebFetchのためにBun compileを実行しない。
11. deterministic試験とhidden実WebView gateが成功する。
12. Windows/Linuxは実証済みplatformだけWebViewを既定にし、その他はsidecarを維持する。

## 11. ロールバック

- backend選択を `sidecar` に戻す。DB downgradeやデータ変換は不要とする。
- Rust経路が開始したtool callをsidecarで再実行しない。次のtool callから切り替える。
- plugin revision更新に問題がある場合は、SAAAの固定revisionを直前の検証済みcommitへ戻す。
- debug表示設定は機能本体と独立して無効化できるようにする。
- sidecar resourceとnpm依存は、macOSのhidden E2Eおよび検索parityが合格するまで削除しない。

ロールバック対象にDB schema、会話履歴、記憶、Provider設定を含めない。本計画ではそれらを変更しない。

## 12. 証跡と完了報告

次を新規作成する。

- `spec/evidence/web-fetch/progress.md`
- `spec/evidence/web-fetch/results.md`

progressにはカードごとのHEAD、変更ファイル、試験件数、失敗、未確認事項を記録する。resultsは次を分ける。

- 実装済み
- offline合格
- visible WebView合格
- hidden/background合格
- live search合格
- platform未検証
- sidecar fallback継続中

「表示モードで取得できた」だけでは完成にしない。hidden/background gateとcleanup確認が揃った時点で、バックグラウンドWebView実装の受入完了とする。
