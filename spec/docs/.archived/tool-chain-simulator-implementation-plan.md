# SAAA ツール実行シミュレーター実装計画

作成日: 2026-09-24
状態: 実装計画

## 目的

ツールを SQLite に登録しただけで「使える」と判定しない。アプリを起動する前に、登録済みの説明がモデルへ提示され、正しい権限と版で取り出され、実際の実行経路を通り、結果と監査記録が一致することを確認する。最初の対象はアーティファクト WebView の `artifact_webview` とし、同じ検証枠組みを後から他のツールにも使えるようにする。

実機で初めて成否を知る開発を避ける。ヘッドレス検証の時点で成功・失敗の予測を出し、実機ではネイティブ WebView の描画、スクロール、遷移制御など、代替できない事実だけを測る。

## 現状から判明した問題

- `webview_catalog.rs` は SQLite の現行 revision から定義を組み立てる。一方、`providers/stream/agent_dispatch.rs` の `artifact_webview` 実行分岐は `webview_ops::execute` を直接呼び、`ToolSelectionService::invoke` が行う参照、権限、schema、版、監査の検査を通らない。
- `ToolSelectionService::search → describe → invoke` には、会話・run に結びつく参照 ID、epoch と grant の再検査、`tool_selection_invocations` の開始・終了記録が既にある。専用ツールの直接提示からこの経路へ接続する仕組みがない。
- `eligible_revisions` などの探索用 SQL は WebView のマウント状態を知らない。直接提示を停止しても、`tools_search` 側にツールの説明が現れ得る。
- `ArtifactWebviewBackend` は `webview_ops::execute` の `{"ok":false}` も `BackendOutcome::succeeded` に包む。この場合、操作失敗と監査上の成功が食い違う。
- `webview_ops` はプロセス全体の静的な session と要求キューを持つ。テストで時刻、UI 応答、会話切替を再現するには、状態所有と待機処理を注入可能にする必要がある。
- 既存の mock catalog、fixture backend、MCP の実通信テストは再利用できるが、WebView の「登録→提示→取り出し→操作→UI 確認」を一つのシナリオで検証していない。

## 実装する検証レイヤー

| レイヤー | 使用する実物 | 判定すること |
| --- | --- | --- |
| カタログ | 隔離 SQLite、実マイグレーション、実登録処理 | source kind、現行 revision、説明、usage、schema、grant、版更新が保存される |
| 提示・探索 | 本番と同じ提示判定と `tools_search/describe` | 会話の状態に応じて定義・使い方が現れる／消える。提示内容は現行 revision に一致する |
| 実行 | 本番の `ToolSelectionService::invoke` と backend router | run・会話・grant・schema・版の検証、操作結果と invocation の終端状態が一致する |
| UI 応答 | WebView 操作ポートの決定的な擬似実装 | 切替、スクロール、閉鎖、応答遅延、会話切替、タイムアウトを予測どおり処理する |
| 実機 | macOS アプリと子 WebView | タブの可視性、ネイティブ描画領域、ページのスクロール、別オリジン遷移の拒否を実測する |

上四層を通常の開発・CI で自動実行する。実機層はその結果と突き合わせる。テスト専用コードが本番の提示・実行を別実装してしまう場合は、この計画の完了条件を満たさない。

## 1. 本番のツール経路を統一する

### 提示時の固定情報

`ToolSelectionService` に、既知のツール ID を直接提示するための限定 API を設ける。入力はホストが確定した `RequestContext` とツール ID、稼働状態であり、モデルが渡す ID は受け付けない。API は SQLite の現行 revision、source、grant と状態条件を同時に検査し、モデルへ渡す定義と、その revision に結びつく短命の実行参照を同じ結果として返す。失敗時はどちらも返さない。

提示結果を一回の provider request の `AgentToolOffer` に保持する。ツール名だけを後から SQLite で再検索して実行しない。既存の `GeneratedToolSnapshot` と同様に、提示された定義と実行対象の版を対応させる。複数の provider request、run、会話が並行しても参照を取り違えない。次の request で再提示するときは状態を再評価する。

提示時点で WebView が閉じていたら定義・usage を返さない。スクロールだけ使えない状態では操作全体を隠すのではなく、利用可能な操作と schema/説明の表し方を決める。第一案は単一ツールを提示し、`scroll` に限り実行時に `webview-not-scrollable` を返す方式。受入テストでは「読み込み中にスクロールを成功扱いしない」を必須とする。

### 実行時の再検査と監査

モデルの `artifact_webview` 呼び出しは offer にある参照を使い、`ToolSelectionService::invoke` へ渡す。そこが schema、scope、grant、現行 revision、epoch を再検査し、invocation を記録したうえで既存の `ArtifactWebviewBackend` を呼ぶ。backend 到達直前にも会話 ID と WebView の状態・世代を確認する。提示後にタブが閉じた場合、または別会話へ移った場合は副作用なしで失敗する。

`{"ok":false}` を `BackendOutcome::failed` とエラーコードへ変換し、`{"ok":true}` のみ succeeded とする。UI 応答待ちの timeout、拒否、取消しもそれぞれ終端状態に写す。`tool_selection_invocations` と provider 側の tool event の両方で同じ呼び出しを追えるよう、invocation ID と run ID を結果に含める。`tools_search/describe/invoke` 経由も同じ稼働状態判定を使い、非アクティブ時の説明漏れを防ぐ。

探索候補の制限は検索後の表示だけで済ませない。会話 ID を受ける `ToolAvailability` 判定を tool selection service の候補集合生成と参照発行へ組み込み、同じ判定を直接提示にも使う。`eligible_revisions`、lexical、embedding の各候補集合が WebView 状態を迂回しないよう、ランキング前に一度絞り、`describe/invoke` でも再検査する。SQL の永続カタログを無効化する方法は他会話にも影響するため採用しない。シミュレーターには本番と同じ availability 実装と擬似 session 状態を渡す。

### WebView 操作状態

静的な単一 `Gate` を会話ごとの操作状態と要求キューを持つ `WebviewOperationPort` に整理する。本番アダプターは現在の Tauri IPC と UI ACK を使い、シミュレーターは決定的な in-memory アダプターを使う。両者は同じ操作検証と状態遷移を共有する。要求 ID、会話 ID、選択世代を照合し、重複 ACK と期限切れ ACK は状態を変えずに破棄する。待機には注入した時計または制御可能な timeout を使い、シミュレーターで実時間の 1.5 秒を待たない。操作拒否の理由は backend の `Failed` 結果にも残し、監査用 error code とモデル向け説明を対応させる。

UI が `applied=true` を返す条件は、タブ切替・閉鎖なら reducer が期待状態になった後、スクロールなら対象子 WebView への命令が受理された後とする。擬似 UI はこの規則で ACK を返し、フロントエンドテストでは実 reducer と IPC 呼び出しの順序を検証する。擬似 ACK だけで実画面の描画成功とは判定しない。

## 2. シミュレーターの入出力

Rust のシナリオ runner を `tool_selection` 配下に置く。CLI は `bun run toolchain:simulate --scenario <名前> [--json]` とし、内部では専用の Rust binary を呼ぶ。binary と runner はテストと共有し、開発者向けの CLI がテストと別の疑似経路にならないようにする。通常の実行は外部 LLM、Web、ユーザーの設定 DB に接続しない。

シナリオの入力は、初期 SQLite 状態、principal/grant、会話と run、WebView の tabs・selected・mounted・scrollable・generation、ユーザー発話に対応する期待ツール呼び出し、UI 応答計画、途中で起こる状態変更、期待結果とする。発話から操作へのモデル判断そのものは決定的シナリオでは固定する。必要なら別の非ゲート評価で実 LLM に選択を試させるが、その結果を経路の成否と混同しない。

runner は次の順で本番 API を呼ぶ。

1. 一時ディレクトリの SQLite に実マイグレーションとカタログ登録を適用する。既存 DB 移行ケースは本番 DB のコピーではなく、旧 schema fixture から開始する。
2. SQLite の現行 revision、`search_text`、usage、schema、grant、source enabled を読み、登録内容を報告する。
3. 状態を設定し、本番のツール提示判定と、探索が有効な場合の `search/describe` を実行する。返った定義と参照を保存する。
4. 必要な競合イベントを挟み、提示済みの参照から本番の invoke と backend router を通す。擬似 UI は要求を観測して ACK、拒否、遅延、無応答を返す。
5. 結果、タブ状態、backend の呼び出し回数、decision と invocation の記録を照合する。期待と異なれば非ゼロ終了する。

人が読む出力は「登録 → 提示/探索 → 説明取得 → 実行 → UI 応答 → 監査」の時系列とし、各段階に `pass/fail` と理由を示す。`--json` は CI 向けに同じ情報を機械可読で出す。URL、ページ本文、秘密、ユーザー DB の内容は出力しない。乱数 ID と時刻はテストでは固定または正規化し、snapshot の差分が意味を持つようにする。

## 3. 最初に固定するシナリオ

| シナリオ | 期待する提示・結果・監査 |
| --- | --- |
| WebView なし | 直接提示も探索結果もなし。実行なし、invocation なし |
| 2 タブで「次のタブ」 | 現行 SQLite 説明が提示され、タブが次へ移り、succeeded の invocation が 1 件 |
| 1 タブで「次のタブ」 | 選択維持を確認して成功。不要な再ロードなし |
| 「スクロールして」 | 選択中の子 WebView にだけ scroll 命令。受理 ACK 後に成功 |
| 読み込み中のスクロール | 成功と報告しない。予め定めた失敗理由と監査状態が一致 |
| 「タブ全部閉じて」 | 現会話の Web サイトタブだけ閉じ、他のアーティファクトを維持 |
| 提示後にタブ閉鎖・別会話へ切替 | 旧参照での操作は副作用なしで拒否。別会話のタブは変わらない |
| grant 取消し、source 無効化、revision 更新 | 古い参照が失効し、backend は呼ばれない |
| schema 不正、範囲外 index、未提示名 | それぞれ決まった検証段階で拒否。backend は呼ばれない |
| UI 無応答・遅延 ACK・重複 ACK | timeout は失敗として記録。後からの ACK で次の操作を変えない |
| SQLite の旧 schema から移行、再起動相当の再登録 | 登録に成功し、現行説明が維持され、不要な revision 増加なし |

各シナリオは、期待する結果だけでなく「どの段階で止まるか」と「invocation が 0 件か 1 件か」を明記する。特に権限拒否を操作失敗と同じ `ok:false` に丸めない。正常系と異常系を CI の必須テストにする。

## 実装順序と完了条件

| 段階 | 作業 | 完了条件 |
| --- | --- | --- |
| 1. 契約 | 提示・探索・実行の状態条件、エラー分類、監査対応表、上記シナリオをテストとして固定 | 失敗地点と期待レコード数が事前に決まる |
| 2. 実行経路 | 直接提示に短命の版付き参照を付け、`ToolSelectionService::invoke` を通す。探索も同じ状態条件を使う | 直接実行の迂回分岐を削除でき、閉鎖後・grant 取消し後に backend が呼ばれない |
| 3. 操作ポート | 会話別状態、決定的な時計・擬似 UI、本番 IPC アダプター、失敗結果の backend 変換 | UI 失敗と監査の成功が食い違わない。遅延 ACK が無害 |
| 4. Runner/CLI | 隔離 DB、シナリオ、時系列レポート、JSON 出力を実装 | CLI と CI が同じ runner で全シナリオを再現する |
| 5. 統合 | React reducer/IPC の契約テスト、macOS 実機の最小スモーク | 実機測定前に予測が確定し、実機との差異を個別に記録できる |


## 実機でのみ測る項目

タブ列がサイトの上に見えること、子 WebView の座標とクリップ、サイトが独自の配色で表示されること、スクロール命令で実ページが動くこと、同一オリジン遷移と別オリジン拒否、タブ再選択時の再ロード・切替時間を測る。ここで不一致が出たら、その条件を可能な範囲でヘッドレスシナリオへ戻してから修正する。
