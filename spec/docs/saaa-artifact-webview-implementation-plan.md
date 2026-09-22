# Artifact WebView 実装計画 — 隔離 Interactive Preview

作成日: 2026-09-22。状態: **評価用 Go（3 OS live は評価者判断で対象外）**。

上位方針は [SAAA Interface / Artifact Runtime Concept](saaa-interface-artifact-runtime-concept.md)、表示面は [Artifact Viewer 実装計画](saaa-artifact-viewer-plan.md) に従う。本計画は、Artifact Workspace の右側パネルでローカル HTML / CSS / JavaScript 成果物を対話的に確認できるようにする。任意サイトを閲覧する汎用ブラウザーは作らない。

## 0. 結論と導入判断

Tauri 2 の子 WebView を使えば実装できる。ただし、現行の Capability は `windows: ["main"]` に権限を付けており、同じウィンドウへ追加した子 WebView にも権限が及ぶ。したがって、**権限分離を先に完了し、隔離を試験で証明できた場合だけ本実装へ進む**。

初期リリースで扱うものは、SAAA が管理する単一ファイルの Interactive HTML Artifact とする。HTML はローカルからだけ読み込み、外部 URL、外部 asset、通信、popup、download、Tauri IPC を許可しない。JavaScript はプレビュー内部で動かせるが、アプリ本体の権限やデータへ到達できない状態を受入条件とする。

判定は次の通り。

| 対象 | 判断 |
| --- | --- |
| SAAA 管理下のローカル Interactive HTML | 条件付き Go |
| 静的 HTML だけの表示 | WebView ではなく sandbox 付き `iframe` を優先 |
| 任意の外部 Web サイト | 初期範囲外 |
| LLM が指定した URL の直接表示 | No-Go |
| メイン WebView と同じ Capability を持つプレビュー | No-Go |
| HTML を既存 `UiNode` / `ui_views` に直接保存 | No-Go |

## 1. 目的

- Coding や委任作業が作った Interactive HTML 成果物を、アプリを離れず Artifact Workspace で確認できる。
- 既存のタブ、閉じる操作、狭幅時の全面表示、Artifact の選択体験を維持する。
- Semantic UI の「任意 HTML、URL、実行式を受け付けない」境界を維持する。
- プレビューを侵害されても、会話、設定、credential、LLM Fetch、Rust command へ到達できないようにする。
- macOS、Windows、Linux のシステム WebView 差を、再現可能な試験と live 検証で管理する。

## 2. 初期範囲外

- 外部サイトを開くブラウザー、アドレスバー、戻る・進む、bookmark
- login、cookie の共有、OAuth、SSO
- microphone、camera、位置情報、clipboard、通知
- file picker、drag and drop、download、print
- 外部 font、CDN、remote image、WebSocket、HTTP fetch
- 複数 HTML / module / asset bundle、Service Worker、WebAssembly
- WebView 内から SAAA Action や Tool を呼ぶ bridge
- 既存 Markdown / Mermaid / Semantic UI の WebView 化
- モバイル対応

これらは初期版の隔離が成立した後、用途と追加権限を個別に審査する。

## 3. 現状と差分

現行 Artifact Workspace は React DOM の二列 layout で、`ArtifactPanel` が `SemanticRenderer` を呼ぶ。最大 8 タブ、改訂切替、行 diff、Escape で閉じる操作がある。WebView は DOM の子要素ではなく native view なので、右パネル中の表示領域へ座標と大きさを同期する必要がある。

現行の安全境界は次の通り。

- `generative_ui/parser.rs` は URL、任意 HTML、実行可能 Tool を受け付けない。
- `present_ui` は検証済み Semantic UI JSON だけを保存する。
- Markdown の生 HTML は escape され、Mermaid の SVG は allowlist で再構築される。
- Tauri CSP はメイン WebView のローカル asset と IPC だけを許可する。
- `capabilities/default.json` は `windows: ["main"]` に `core:default` と `llm-fetch:default` を付与している。

WebView 導入で変えるのは、Artifact Workspace の表示対象、子 WebView の lifecycle、プレビュー用の読み取り経路、Capability の対象指定である。既存 `ui_views`、Semantic UI parser、Markdown renderer、認証方式は変えない。

## 4. 設計原則

### 4.1 Semantic UI と Interactive Preview を分離する

`WebView(url, html)` のような `UiNode` kind は追加しない。Artifact Workspace が扱うタブを次の union に拡張する。

```ts
type ArtifactTab =
  | {
      kind: "semantic-ui";
      conversationId: string;
      instance: UiInstance;
    }
  | {
      kind: "interactive-preview";
      descriptor: ArtifactPreviewDescriptor;
    };
```

LLM が `present_ui` で Interactive Preview を生成する経路は作らない。Coding / Steward / Artifact runtime が検証済み Artifact revision を登録した後、その参照からだけ開く。

### 4.2 原本と表示状態を分ける

- 正本: immutable な Artifact revision payload と digest
- 台帳: Artifact ID、revision ID、media type、size、scope、lifecycle
- 実行時派生: preview token、子 WebView label、座標、visible 状態
- フロント一時状態: 開いているタブ、active tab

HTML 本文や preview token を `localStorage`、React state、`ui_views` に複製しない。タブを開くたびに Rust が revision と scope を照合し、短命な preview token を払い出す。

### 4.3 権限は default deny にする

プレビュー子 WebView は、Tauri core API、plugin、SAAA command のどの Capability にも一致させない。親のメイン WebViewだけが子 WebViewの作成、位置変更、size変更、表示、非表示、focus、closeを行える。

### 4.4 URL ではなく Artifact 参照を受け取る

フロントが指定できるのは `artifactId` と `revisionId` だけとする。file path、HTML、URL、CSP、WebView label は Rust が決定する。scope 不一致、archive / delete 済み、digest 不一致、上限超過は表示しない。

## 5. データ契約

初期版の media type は `text/html`、encoding は UTF-8、payload は単一ファイル、上限は 1 MiB とする。NUL、UTF-8 不正、digest 不一致を拒否する。

Rust からフロントへ返す契約は次を基準にする。

```ts
type ArtifactPreviewDescriptor = {
  artifactId: string;
  revisionId: string;
  title: string;
  mediaType: "text/html";
  digest: string;
  previewToken: string;
  expiresAt: string;
  policy: {
    network: "none";
    navigation: "preview-only";
    popup: "deny";
    download: "deny";
    tauriIpc: "deny";
  };
};
```

`previewToken` は十分な entropy を持つ single-session token とし、DB へ保存しない。Artifact、revision、呼出元 scope、期限へ binding し、WebView close、タブ close、アプリ終了、期限切れで無効化する。

新しい IPC は次の最小集合とする。

| Command | 入力 | 出力 / 効果 |
| --- | --- | --- |
| `prepare_artifact_preview` | Artifact ID、revision ID | 検証済み descriptor を返す |
| `release_artifact_preview` | preview token | token を失効させる。idempotent |

子 WebView の操作は `@tauri-apps/api/webview` の core API を親から呼ぶ。HTML 本文を IPC response として JavaScript へ返さない。

## 6. プレビュー配信

### 6.1 技術スパイク

WVP-00 では、同梱した固定 HTML fixture をローカル app route から子 WebViewへ表示する。目的は content pipeline の完成ではなく、位置同期、focus、tab lifecycle、Capability 分離を最小構成で確認することに限定する。

### 6.2 本実装

本実装では Rust 側の専用 custom protocol から、preview token に対応する Artifact revision の bytes を読み取り専用で返す。概念上の URL は次の形式とし、実際の scheme 名は Tauri / Wry の3 OS spikeで確定する。

```text
saaa-artifact-preview://preview/<opaque-token>/index.html
```

response には少なくとも次を付与する。

```text
Content-Type: text/html; charset=utf-8
Cache-Control: no-store
X-Content-Type-Options: nosniff
Referrer-Policy: no-referrer
Content-Security-Policy:
  default-src 'none';
  script-src 'unsafe-inline';
  style-src 'unsafe-inline';
  img-src data: blob:;
  connect-src 'none';
  font-src 'none';
  media-src 'none';
  object-src 'none';
  frame-src 'none';
  worker-src 'none';
  manifest-src 'none';
  form-action 'none';
  base-uri 'none'
```

初期版は単一 HTML を対象とするため、inline script / style は隔離 WebView 内に限って許可する。外部 asset と通信は許可しない。`unsafe-inline` をメイン WebView の `script-src` へ追加してはならない。

custom protocol が3 OSで安定しない場合は本実装を止める。`data:` URL 対応や localhost server へ黙って切り替えず、別の設計判断として再審査する。

## 7. Capability 分離

`capabilities/default.json` は window 全体ではなくメイン WebView label を対象にする。

```json
{
  "identifier": "main-webview",
  "webviews": ["main"],
  "permissions": [
    "core:default",
    "llm-fetch:default",
    "core:webview:allow-create-webview",
    "core:webview:allow-set-webview-position",
    "core:webview:allow-set-webview-size",
    "core:webview:allow-set-webview-focus",
    "core:webview:allow-webview-show",
    "core:webview:allow-webview-hide",
    "core:webview:allow-webview-close"
  ]
}
```

これは方向を示す例であり、実際の permission identifier は現在固定されている Tauri schema で検証する。`artifact-preview-*` label に別 Capability を付けない。SAAA の登録 command が Capability 未一致の WebView から呼べないことも、core/plugin と分けて試験する。

権限変更後は、メイン画面の既存 IPC、event、window操作、LLM Fetchが退行していないことを確認する。権限不足を解消するために wildcard の `windows: ["*"]` や `webviews: ["*"]` を使わない。

## 8. Frontend 構成

新規 module の責務を次のように分ける。

| Module | 責務 |
| --- | --- |
| `artifacts/artifactTab.ts` | Semantic UI / Interactive Preview の tab union と reducer |
| `artifacts/InteractivePreview.tsx` | placeholder、loading、error、retry、focus説明 |
| `artifacts/useArtifactWebview.ts` | create / position / size / show / hide / close lifecycle |
| `artifacts/artifactPreviewApi.ts` | prepare / release IPC |
| `artifacts/webviewGeometry.ts` | DOMRectからlogical position / sizeへの変換 |

`ArtifactDrawer.tsx` は tab shell と focus contract を維持し、active tab が Interactive Preview のときだけ WebView host placeholder を描画する。`ArtifactPanel.tsx` と `SemanticRenderer` は変更しない。

子 WebView は同時に1個だけ作る。8タブ分を常駐させず、activeなInteractive Previewへ切り替える際に前のWebViewをcloseしてtokenをreleaseする。初期版ではWebView内部状態のタブ間保持を保証しない。再表示時はArtifact revisionから再読込する。

### 8.1 Geometry 同期

- placeholderを`ResizeObserver`で監視する。
- window resize、device scale factor変更、sidebar / Artifact 幅変更、狭幅 breakpointで再計算する。
- `getBoundingClientRect()`を基準に、親WebViewのlogical座標系へ変換する。
- 幅または高さが0、tab非active、document hidden、Artifact閉鎖中は子WebViewをhideする。
- 連続resizeはanimation frame単位でcoalesceし、古い非同期更新が新しい座標を上書きしないようgeneration番号を持つ。
- native viewはDOMのborder radiusやoverflowでclipされない前提とし、placeholder内側の矩形だけを渡す。

### 8.2 Lifecycle

```text
tab activate
  -> prepare descriptor
  -> placeholder measurement
  -> create child WebView
  -> created event後にshow/focus

resize
  -> latest geometryをsetPosition/setSize

tab switch / close / component unmount
  -> hide
  -> close child WebView
  -> release preview token
```

React Strict Mode、create途中のtab close、IPC失敗、created event前のunmountを含めてidempotentにする。cleanup失敗はUIを閉じることを妨げず、Rust側の期限切れで回収する。

## 9. Navigation と操作制限

- 初回URLと同じpreview tokenへのfragment変更だけを許可する。
- `http:`, `https:`, `file:`, `data:`, `blob:`へのtop-level navigationを拒否する。
- `window.open`、target `_blank`、popup生成を拒否する。
- download handlerは常に拒否する。
- permission requestはcamera、microphone、geolocation、notification、clipboardを含めて拒否する。
- context menu、link preview、devtoolsはrelease buildで無効とする。
- main WebViewへのevent送信、`postMessage` bridge、共有cookieを設けない。

OS WebViewで拒否hookが利用できない項目はCSPとHTML正規化だけで「対応済み」とせず、negative evidenceとして記録しNo-Go判定へ含める。

## 10. Backend 構成

新規Rust moduleの候補を次とする。

| Module | 責務 |
| --- | --- |
| `artifact_preview/contracts.rs` | descriptor、prepare/release input |
| `artifact_preview/service.rs` | scope、revision、media type、size、digest検証 |
| `artifact_preview/tokens.rs` | token発行、binding、期限、失効 |
| `artifact_preview/protocol.rs` | custom protocol response、security headers |
| `artifact_preview/policy.rs` | navigation / popup / download判断 |

DB schemaはContent Artifact基盤の正本を使用する。本計画だけの`webview_artifacts`テーブルは作らない。Content Artifact基盤が未実装の間、WVP-00〜WVP-05は同梱fixtureだけで進め、永続Artifactを扱うWVP-06以降を開始しない。

token台帳はprocess memoryに持つ。各entryはArtifact revision identity、scope、digest、expiry、WebView labelを保持し、payload本体やcredentialを保持しない。最大同時token数を制限し、期限切れを定期またはアクセス時に回収する。

## 11. エラーとフォールバック

- 未対応platform / WebView生成不可: HTML sourceを実行せず、Artifact情報と「プレビューを利用できません」を表示する。
- revision検証失敗: stale contentを表示せず再読込を促す。
- 子WebView crash: 1回だけ明示的retryを提供する。自動reload loopにしない。
- geometry更新失敗: 子WebViewをhideし、ずれた位置に表示し続けない。
- token期限切れ: 新しいtokenをprepareし直す。古いtokenを再利用しない。
- policy違反navigation: 現在表示を維持し、監査用のreason codeだけを記録する。URL全文やHTML本文はlogへ出さない。

WebViewを作れない場合にSemantic RendererへHTMLを流し込むfallbackは禁止する。

## 12. 作業カード

| ID | 内容 | 受入条件 |
| --- | --- | --- |
| WVP-00 | 固定fixtureを子WebViewで表示する3 OS spike | macOS / Windows / Linuxで作成、表示、resize、closeの可否が記録される |
| WVP-01 | 現行Capabilityと全登録commandの到達性監査 | 子WebViewから到達可能なcore/plugin/SAAA command一覧が得られる |
| WVP-02 | Capabilityをmain webview labelへ限定 | メインの既存機能が通り、fixture childからIPCが全拒否される |
| WVP-03 | `webviewGeometry.ts` と単体試験 | scale、0 size、rapid resize、stale updateが試験される |
| WVP-04 | `useArtifactWebview` lifecycle | create途中close、tab switch、unmount、失敗時cleanupが通る |
| WVP-05 | Artifact Workspaceへfixture tabを接続 | Semantic UI tabとInteractive tabを切り替え、同時WebViewが1個以下 |
| WVP-06 | Content Artifact基盤との依存確認 | revision、payload、scope、digestの正本が固定される。未完なら停止 |
| WVP-07 | preview契約とprepare/release IPC | 未知field、scope不一致、bad media type、size超過、digest不一致を拒否 |
| WVP-08 | token ledger | single-session、expiry、release、上限、並行race試験が通る |
| WVP-09 | custom protocolとsecurity headers | tokenなし/期限切れ/別scopeが読めず、CSPが期待通り付く |
| WVP-10 | navigation/popup/download/permission拒否 | adversarial fixtureが外部遷移、通信、popup、download、device accessできない |
| WVP-11 | Interactive Preview UI、i18n、accessibility | loading/error/retry、tab/focus/Escape、screen reader labelが確認される |
| WVP-12 | crash、cleanup、memory検証 | 100回切替後にorphan WebView/tokenがなく、memory増加が許容内 |
| WVP-13 | full gateとdesktop smoke | targeted test、build、IPC contract、desktop smokeが通る |
| WVP-14 | 3 OS live evidence | layout、DPI、maximize、minimize、複数monitorの結果を保存 |
| WVP-15 | Go / No-Go review | §15を全項目判定し、未確認をGoにしない |

実施順は WVP-00 → 01 → 02 → 03 → 04 → 05。ここで技術Goを判定する。Content Artifact基盤の準備後、WVP-06 → 07 → 08 → 09 → 10 → 11 → 12 → 13 → 14 → 15と進める。

## 13. テスト計画

### 13.1 Frontend unit / component

- tab reducer: 異種tab、最大8、select、close、active fallback
- geometry: scale factor、fractional座標、0 size、viewport外、rapid resize
- lifecycle: create成功/失敗、created前close、二重cleanup、retry
- Artifact Drawer: Semantic UIとの切替、Escape、focus復帰、狭幅全面表示
- error: prepare失敗、create失敗、crash、release失敗

Tauri APIは境界moduleだけでmockし、Artifact Drawer全体でnative挙動を模倣しない。

### 13.2 Rust unit / integration

- descriptor decodeの未知field拒否
- Artifact / revision / scope / media type / size / digest検証
- tokenの発行、expiry、release、二重release、上限、scope binding
- protocol path traversal、token推測、別revision差替えの拒否
- security headersの完全一致
- reason codeにHTML、path、token、URLが混ざらないこと

### 13.3 Security fixture

最低限、次を試みるHTML fixtureを用意する。

- `window.__TAURI_INTERNALS__`と直接IPC
- `@tauri-apps/api`相当のinvoke request
- `llm-fetch` plugin command
- 全登録SAAA commandの代表read/write command
- `fetch`, XHR, WebSocket, EventSource, beacon
- external image / CSS / script / iframe / object
- form submit、top navigation、`window.open`、download link
- camera、microphone、geolocation、notification、clipboard
- `file:`読取、local path画像、custom protocol横断

「画面上で失敗した」だけで合格にせず、host側にcommand到達、network接続、file accessが発生していないことを観測する。

### 13.4 Desktop live matrix

| 観点 | macOS WKWebView | Windows WebView2 | Linux WebKitGTK |
| --- | --- | --- | --- |
| create / close | 必須 | 必須 | 必須 |
| resize / DPI | 必須 | 必須 | 必須 |
| maximize / minimize | 必須 | 必須 | 必須 |
| multiple monitor | 必須 | 必須 | 可能な環境で必須 |
| focus / keyboard / IME | 必須 | 必須 | 必須 |
| navigation / popup拒否 | 必須 | 必須 | 必須 |
| IPC / network拒否 | 必須 | 必須 | 必須 |
| 100回tab切替 | 必須 | 必須 | 必須 |

## 14. 性能と運用上限

- active child WebView: 最大1
- open Artifact tabs: 既存どおり最大8
- HTML payload: 最大1 MiB
- preview token: 1 Artifact tabにつきactive時のみ1、短命
- geometry更新: animation frame単位で最大1回
- create timeout: 5秒
- 初回表示目標: prepare開始から2秒以内。ただしOS WebView cold startを別計測する

変更前にSemantic UI Artifactのopen/close、メイン画面memory、初期bundle sizeを測る。同じbuild modeとfixtureで変更後を測り、子WebViewを一度閉じた後もmemoryが単調増加する場合はreleaseしない。

## 15. Go / No-Go ゲート

### 技術Go

- 3 OSすべてで子WebViewのcreate、位置同期、resize、closeが成立する。
- Capabilityをwebview labelへ限定してもメイン機能が退行しない。
- 子WebViewからcore/plugin/SAAA IPCがすべて拒否される。
- external navigation、network、popup、download、device permissionが拒否される。
- tab切替とunmount後にorphan WebViewとtokenが残らない。
- custom protocolのoriginとCSPが3 OSで同じ安全特性を持つ。

### No-Go

- 子WebViewへメインのCapabilityが継承される、または独自commandへ到達できる。
- navigation / popup hookがOS間で揃わず、CSPでも閉じられない。
- native viewをArtifact panelへ安定して追従させられない。
- crashやrapid tab switchでorphan WebViewが残る。
- Content Artifactの正本を使えず、HTML専用の並行storeが必要になる。
- security fixtureの失敗理由を観測できない。

No-Go時は、WebView導入を見送り、静的snapshot、画像、検証済みSemantic UIへの変換を代替手段とする。

## 16. 完了条件

- WVP-00〜WVP-15の証拠がカード単位で残る。
- Interactive HTML ArtifactをArtifact Workspaceで開き、tab切替、resize、closeできる。
- Semantic UI、Markdown、Mermaidの既存試験が退行しない。
- 子WebViewがTauri IPC、network、外部navigation、file/device機能へ到達できない。
- Artifact revisionとdigestが同一の内容だけを表示する。
- 3 OS live matrixを完了する。
- Security reviewで未確認項目がなく、Go / No-Go判断が記録される。

証拠は `spec/evidence/artifact-webview/` に保存し、`spike-<platform>-YYYYMMDD.md`、`security-results.md`、`performance-results.md`、`go-no-go.md`へ分ける。HTML本文、token、credential、ユーザーのfile pathは証拠へ記録しない。

## 17. 公式参照

- [Tauri Webview JavaScript API](https://v2.tauri.app/reference/javascript/api/namespacewebview/)
- [Tauri Capability](https://v2.tauri.app/reference/acl/capability/)
- [Tauri Core Permissions](https://v2.tauri.app/reference/acl/core-permissions/)
- [Tauri Content Security Policy](https://v2.tauri.app/security/csp/)

