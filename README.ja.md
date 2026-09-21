# SAAA

Situation-Aware Ambient Agent Runtime

[English](README.md) | 日本語

**MIT · 開発中のMVP · 主な検証対象：macOS**

[起動する](#ローカルで起動する) · [開発に参加](CONTRIBUTING.md) · [困ったとき](SUPPORT.md) · [セキュリティ](SECURITY.md) · [ライセンス](LICENSE)


SAAA は、会話と音声を一つのデスクトップアプリにまとめる、ローカルファーストの AI ランタイムです。React と Tauri で作られており、会話と設定は端末内の SQLite に保存します。モデルの接続先は、プライベートネットワーク上のローカル LLM サーバー、OpenAI 互換 API、または機能フラグで有効にする LARM から選べます。

SAAA が目指しているのは、入力された質問へ答えるだけでなく、利用者の状況に応じて「今は支援するべきか、何もしないべきか」を判断できる常駐型のランタイムです。ただし、現在の実装はその途中段階にあります。アプリの自動操作や自動通知は行いません。

## 現在の状態

このリポジトリは開発中の MVP（実用に必要な一部機能へ範囲を絞った試作版）です。テキスト・音声会話、ローカルデータのバックアップと診断情報の出力まで実装されています。

通常の開発とオフライン検証は実行できますが、LARM 経由の本番利用はまだ承認されていません。API 契約、隔離環境で段階的にトラフィックを流す 30 分の canary、2 時間連続で安定性を確かめる soak test などに未完了項目があります。機能別の現行状態は[製品準備状況](spec/docs/product-readiness-status.html)を参照してください。旧[MVP 2.6 Release Evidence](spec/docs/mvp-2.6-release-evidence.html)はLARM経路に限定した過去の証跡として残します。

## できること

| 画面 | 主な機能 | 現在の安全上の制約 |
| --- | --- | --- |
| Chat | テキスト入力、マイク入力、ストリーミング応答、OS の音声合成による読み上げ。監査ログは会話メニューからモーダルで開く | ローカル接続を選んだ場合、Cloud への自動フォールバックは既定で無効 |
| Settings | モデル経路、音声、プライバシー設定を管理 | APIキーはKeychainへ保存し、設定JSONやSQLiteには保存しない |

音声会話の音声認識には、設定したHarness側のASR、または個別のASR Providerを使います。音声入力を使わない場合、ASRや話者登録は不要です。ASR は音声をテキストへ変換する仕組みです。現在はマイクのみを扱い、システム音声、翻訳、フローティングオーバーレイには対応していません。

## 必要なもの

- [Bun](https://bun.sh/) 1.3.14（`package.json`で固定）
- Rust 1.92.0（`rust-toolchain.toml`で指定。rustfmt・Clippyを含む）
- 対象 OS 用の Tauri 2 ビルド環境（macOSでは`xcode-select --install`でXcode Command Line Toolsを導入）
- ローカル会話経路を使う場合は、プライベートネットワークから接続できるローカル LLM サーバー。`LARM_API_TOKEN`は、サーバーがLAN内の匿名接続を許可する場合は任意、Bearer認証を要求する場合は必須です。
- 音声入力を使う場合は、設定するASRサービスへの接続

主な検証対象は macOS です。OS の音声合成は macOS、Linux、Windows に実装があります。

## ローカルで起動する

ソースから起動します。モデルや音声サービスの接続は、アプリを開いてから設定できます。

```sh
git clone https://github.com/ugnoguchigxp/SAAA.git
cd SAAA
bun install --frozen-lockfile
bun start
```

1. Settingsでモデル接続を追加し、接続を確認して会話用の経路に選びます。OpenAI互換APIのキーは設定画面から登録できます。
2. Chatから短いテキストを送り、応答を確認します。
3. 音声を使う場合だけ、ASRの接続、入力デバイス、マイク権限を設定します。話者フィルターも任意です。

ローカルLLMの接続APIがBearer認証を要求する場合は、起動前に同じシェルで`LARM_API_TOKEN`を設定してください。サーバーがLAN内の匿名接続を明示的に許可する場合だけ未設定にできます。`bun run dev`はフロントエンドの開発サーバーです。デスクトップIPCや音声の確認には`bun start`を使います。

## モデル接続を設定する

### ローカル LLM サーバー

SAAA は設定したホストの接続 API を通じて、会話に使うローカルモデルへの接続を取得します。応答で得た OpenAI 互換の接続先、モデル名、短時間だけ有効な認証情報はメモリ上で使い、各ターンの終了時に接続を解放します。これらの値は SQLite に保存しません。

`LARM_API_TOKEN`を設定すると、SAAAはBearer認証情報として送信します。信頼できるLAN内でサーバーが匿名接続を明示的に許可する場合は未設定にできますが、すべてのサーバーが認証不要という意味ではありません。SAAA は SSH トンネルを作成しないため、接続 API と、サーバーが返すモデル接続先の両方へプライベートネットワークから到達できる必要があります。

音声会話は、Settings → Model Providers で設定した LAN host を共用します。SAAA はプライベート ASR 接続先を導出し、`/v1/models` と `/health` からモデル情報を取得して Settings → Voice へ反映します。ASR専用の環境変数は不要です。

### OpenAI 互換 API

Settings から接続先とモデル名を追加できます。API-key 認証を使う場合は、Settings で Provider のキーを保存してください。キーは macOS Keychain の service `com.saaa.provider-api-key` に Provider ID ごとに保存され、Settings JSON や SQLite には保存されません。この認証情報の保存は macOS 専用です。

現行の OpenAI 互換 Provider は、`SAAA_PROVIDER_<PROVIDER_ID>_API_KEY` や `OPENAI_API_KEY` への fallback を行いません。下記の LARM token は別のランタイム経路で使用します。

### LARM Provider

LARM Provider は既定で無効です。オフライン検証済みの経路を開発環境で試す場合だけ、起動時に次の値を設定します。

```sh
export SAAA_LARM_ENABLED=1
export LARM_API_TOKEN="<token>"
bun start
```

機能フラグは起動時に一度だけ読み込まれます。無効へ戻す場合もアプリを再起動してください。本番トラフィックを有効にする前に、[LARM Operations Runbook](spec/docs/mvp-2.6-larm-operations-runbook.html) と現在の release evidence を確認する必要があります。

### WebFetch ツール

会話モデルには、`llm-fetch` を使った `web_search` と `fetch_content` を提供します。DuckDuckGo 検索は API key なしで利用できます。SAAA の起動時に `BRAVE_SEARCH_API_KEY` が設定されている場合は、DuckDuckGo で再試行可能な失敗が起きたときだけ Brave Search をフォールバックとして使います。

同梱ランタイムは、パッケージのモデル向け toolset と strict Context Guard を使います。検索結果と取得本文は、信頼できない tool data として扱われます。取得できるのは標準ポート上の公開 HTTP(S) URL だけで、任意機能の Playwright レンダリングは含みません。

## 音声プロファイル

Settings → Voice → My voice profile では、利用者本人の声を端末内で照合するフィルターを設定できます。有効化には、10〜12 秒の有効なサンプルを 5 件、合計 50 秒以上登録する必要があります。発音やイントネーションの違いを含む 5 種類の長文が順番に表示されます。文章は意図的に録音時間より長く、全文を読み切らず、約 12 秒で自動停止するまで連続して読み上げます。

音声サンプルは WAV としてアプリのデータディレクトリに、話者埋め込みは SQLite に、いずれも暗号化せず保存します。macOS では保存ディレクトリを `0700`、音声ファイルを `0600` に制限します。フィルターが有効な間は、ローカル照合に通った音声だけをローカル ASR サーバーへ送ります。モデル、保存データ、タイムアウト、話者判定で問題が起きた場合、フィルターを迂回して音声を送ることはありません。

この機能は文字起こし時のプライバシーフィルターです。本人確認や、録音した声によるなりすましを防ぐ認証機能ではありません。

## 開発と検証

変更前後の基本確認には次を使います。

```sh
bun run check:local
bun run test:rust-packages
bun run spec:check
bun run desktop:smoke
bun run desktop:e2e --report-dir /absolute/path/to/new-report-directory
bun run readiness:verify --report-dir /absolute/path/to/new-report-directory
```

- `bun run check:local`: oxfmtの整形検査とoxlintを実行してから、既存の`check`を実行します。
- `bun run test:rust-packages`: 独立したRust crateとサービスのテストを実行します。
- `bun run spec:check`: 仕様書の構造と書式を確認します。
- `bun run check`: モジュールサイズ、生成物、型、Rust の format・Clippy、フロントエンドと Rust のテストを確認します。
- `bun run test:coverage`: ローカル用の HTML/LCOV レポートを `coverage/` に出力します。`bun run check` には含まれません。
- `bun run build`: TypeScript を検査し、フロントエンドの production build を作成します。
- `bun run desktop:smoke`: debug 版デスクトップアプリを一時データディレクトリで起動し、IPC の準備完了を確認します。macOS では同梱した話者照合ランタイムも確認します。
- `bun run desktop:e2e`: debug 版デスクトップアプリをビルド・起動し、主要画面、IPC、初期スナップショット、主会話、SQLite、Situation 採取を一時データ上で検証します。macOS では話者照合ランタイムも対象です。結果は `summary.json` とログへ保存します。
- `bun run readiness:verify`: 3つの自動検証laneを実行し、本文を含まず上書きできないJSON証跡を出力します。実行ごとに新しい絶対pathを指定します。dirtyなworking treeの結果はrelease根拠にできません。
- `bun run tauri build`: 対象 OS の配布用デスクトップアプリを作成します。

System Context は `contexts/` で管理しています。変更した場合は `bun run s11tnext:build` を実行してください。Rust 側の IPC 型を変更した場合は `bun run ipc:generate` で TypeScript の型を更新します。通常の build と check は、生成物が古い場合に失敗します。

LARM の canary と soak test、MVP 2 / 2.5 の手動受け入れ検証には専用 runner があります。必要な環境変数、隔離ディレクトリ、実行順は、コマンドを直接試す前に各 runbook と release evidence で確認してください。

## カバレッジレポート

行カバレッジの割合はローカル確認用であり、出荷条件ではありません。次で生成します。

```sh
bun run test:coverage
```

フロントエンドの LCOV は `coverage/frontend/` に出力されます。`cargo-llvm-cov` が入っていれば、Rust の HTML レポートは `coverage/rust/` に出力されます。`coverage/` は git に含めません。

## ローカルデータとプライバシー

SAAA は `com.saaa.desktop` のアプリデータディレクトリに SQLite データベースを一つ作成します。macOS では次の場所です。

```text
~/Library/Application Support/com.saaa.desktop/saaa.sqlite3
```

メインデータベースを read-write で所有するのは、Tauri プロセスの起動時に一度だけ作成し、すべての書込み処理で使い回す Rust の `SqliteWriter` 一つだけです。SQLite を開く前に OS の排他ロックを取得するため、同じデータディレクトリを使う二つ目の SAAA プロセスは、データベースを開いたり移行したりする前に拒否されます。参照処理は read-only flag と `query_only=ON` を設定した別接続を使い、一回の操作を一つの read transaction で包むため、操作内の各クエリは同じスナップショットを参照します。複数の Reader を許可しながら、書込みは単一 Writer を通して直列化されます。隣に作られる `saaa.sqlite3.writer.lock` は終了後も残ることがありますが、ロック自体は OS が自動解放します。SAAA の実行中にこのファイルを削除しないでください。

データベースには、設定、会話、確定済みメッセージ、実行状態、暗号化していない話者埋め込み、構造化した監査イベントを保存します。監査イベントは7日間保持し、7日を過ぎたものは起動時にデータベースを開く際に削除します。マイク、ASR、会話、Provider、TTS、設定変更のライフサイクルを相関・因果IDで関連づけ、イベント名、状態、時刻、結果、失敗コードを記録します。音声品質の評価値、生音声、文字起こし・プロンプト・モデル出力の本文、認証情報、接続先アドレス、一時的なallocation/request IDは監査に保存しません。モデルの API key、`LARM_API_TOKEN`、ローカル LLM サーバーから受け取った一時的な接続情報は保存しません。

会話メニューから、最新200件の監査イベントを読み取り専用のモーダルで確認できます。Settings → Privacy & Security から、SQLite の整合性を保ったバックアップと、内容を伏せた診断 JSON を作成できます。診断情報には最新1,000件の監査イベントを含め、会話本文、ローカルパス、認証情報を含めません。

データベースのバックアップには暗号化していない話者埋め込みが含まれますが、WAV 音声サンプルは含まれません。そのため、バックアップだけを戻しても音声プロファイルは復元できません。古いスキーマを開く前には、移行前のデータベースバックアップを自動作成します。

## 現在の制約

- LARM の本番利用は承認待ちです。API 契約、隔離環境、canary、soak test、セキュリティ、ロールバック、運用手順の確認がすべて終わるまで、本番経路として扱わないでください。

## リポジトリ構成

```text
src/             React UI
src-tauri/       Rust runtime and Tauri desktop shell
contexts/        s11tnext system-context sources
scripts/         smoke tests and readiness runners
tests/           frontend and contract tests
spec/docs/       design documents, ADRs, runbooks, and release evidence
```

## 関連ドキュメント

- [Project Concept & Direction](spec/docs/plan.html)
- [製品準備状況](spec/docs/product-readiness-status.html)
- [製品受入手順](spec/docs/product-readiness-acceptance-runbook.html)
- [Internal Design Documents](spec/docs/README.html)
- [MVP 2.6 Release Evidence](spec/docs/mvp-2.6-release-evidence.html)
- [LARM Operations Runbook](spec/docs/mvp-2.6-larm-operations-runbook.html)
- [Runtime Boundary ADR](spec/docs/adr/0001-mvp-runtime-boundaries.html)
- [Input Activity Privacy ADR](spec/docs/adr/0003-input-activity-signal-privacy.html)

## 開発に参加する

不具合の再現手順、ドキュメントや翻訳の修正、テストの追加も歓迎します。大きな動作変更は、実装前に目的と利用例をIssueで共有してください。

- [CONTRIBUTING](CONTRIBUTING.md)：環境構築、検査、生成物とPRの扱い
- [SUPPORT](SUPPORT.md)：よくある問題と報告時に必要な情報（日英）
- [CODE_OF_CONDUCT](CODE_OF_CONDUCT.md)：コミュニティでの行動指針
- [SECURITY](SECURITY.md)：脆弱性情報の扱い。非公開報告先は未確定です

## ライセンス

SAAAのソースコードは [MIT License](LICENSE) で公開しています。同梱している話者照合コンポーネントはそれぞれの条件に従います。[THIRD_PARTY_NOTICES](src-tauri/resources/voice/THIRD_PARTY_NOTICES.md) を参照してください。

依存パッケージやモデルを含む案内は[第三者ライセンス](THIRD_PARTY_NOTICES.md)を参照してください。
