# SAAA

Situation-Aware Ambient Agent Runtime

[English](README.md) | 日本語

SAAA は、会話、音声、ミーティングの文字起こし、作業状況の観測を一つのデスクトップアプリにまとめる、ローカルファーストの AI ランタイムです。React と Tauri で作られており、会話と設定は端末内の SQLite に保存します。モデルの接続先は、プライベートネットワーク上のローカル LLM サーバー、OpenAI 互換 API、または機能フラグで有効にする LARM から選べます。

SAAA が目指しているのは、入力された質問へ答えるだけでなく、利用者の状況に応じて「今は支援するべきか、何もしないべきか」を判断できる常駐型のランタイムです。ただし、現在の実装はその途中段階にあります。状況の観測は評価用のシャドーモードに限定され、アプリの自動操作や自動通知は行いません。

## 現在の状態

このリポジトリは開発中の MVP（実用に必要な一部機能へ範囲を絞った試作版）です。テキスト・音声会話、マイクによるミーティング文字起こし、Situation の記録と校正、ローカルデータのバックアップと診断情報の出力まで実装されています。

通常の開発とオフライン検証は実行できますが、LARM 経由の本番利用はまだ承認されていません。API 契約、隔離環境で段階的にトラフィックを流す 30 分の canary、2 時間連続で安定性を確かめる soak test などに未完了項目があります。現在の判定は [MVP 2.6 Release Evidence](spec/docs/mvp-2.6-release-evidence.html) を参照してください。

## できること

| 画面 | 主な機能 | 現在の安全上の制約 |
| --- | --- | --- |
| Chat | テキスト入力、マイク入力、ストリーミング応答、OS の音声合成による読み上げ | ローカル接続を選んだ場合、Cloud への自動フォールバックは既定で無効 |
| Meeting | マイク音声の途中・確定文字起こし、一時停止、確認後の保存 | 明示的に開始したセッションだけを対象とし、録音中は TTS を停止 |
| Situation | 前面アプリ、入力の有無、SAAA 自身の動作などを分類し、介入判断を記録・再生 | 既定で無効。自動のモデル呼び出し、通知、読み上げ、Meeting 開始、アプリ操作は行わない |
| Settings | モデル経路、音声、Situation、プライバシー設定を管理 | 認証情報は設定画面や SQLite に保存しない |

音声会話と Meeting の音声認識には、LAN 内のローカル ASR サーバーを使います。ASR は音声をテキストへ変換する仕組みです。現在はマイクのみを扱い、システム音声、翻訳、フローティングオーバーレイには対応していません。

## 必要なもの

- [Bun](https://bun.sh/)
- Rust toolchain
- 対象 OS 用の Tauri 2 ビルド環境
- ローカル会話経路を使う場合は、プライベートネットワークから接続できるローカル LLM サーバーと `LARM_API_TOKEN`
- 音声入力または Meeting を使う場合は、SAAA から接続できるローカル ASR サーバー

主な検証対象は macOS です。OS の音声合成は macOS、Linux、Windows に実装がありますが、Situation の前面アプリ・入力状態の取得と、音声プロファイルの鍵管理は macOS の機能に依存します。

## ローカルで起動する

依存関係をインストールします。

```sh
bun install
```

ローカル LLM 経路を使う場合は、同じシェルでトークンを環境変数に設定してから起動します。

```sh
export LARM_API_TOKEN="<token>"
bun start
```

アプリが開いたら、Settings → Model Providers でローカル LLM Provider を設定し、有効にしてください。入力するのはホスト名またはプライベート IP だけです。接続先の詳細とモデル名はサーバーから取得するため、設定画面には保存しません。端末固有の接続先やモデルは既定で有効になりません。

## モデル接続を設定する

### ローカル LLM サーバー

SAAA は設定したホストの接続 API を通じて、会話に使うローカルモデルへの接続を取得します。応答で得た OpenAI 互換の接続先、モデル名、短時間だけ有効な認証情報はメモリ上で使い、各ターンの終了時に接続を解放します。これらの値は SQLite に保存しません。

ローカル LLM サーバーとの通信には `LARM_API_TOKEN` が必要です。SAAA は SSH トンネルを作成しないため、接続 API と、サーバーが返すモデル接続先の両方へプライベートネットワークから到達できる必要があります。

音声会話と Meeting の ASR 接続先は `SAAA_ASR_BASE_URL` から読み込みます。たとえば `http://10.0.0.42:8081` のように、認証情報やパスを含まないプライベートネットワーク上の HTTP origin を指定してください。

### OpenAI 互換 API

Settings から接続先とモデル名を追加できます。認証情報は次の環境変数から読み込みます。

```text
SAAA_PROVIDER_<PROVIDER_ID>_API_KEY
```

`<PROVIDER_ID>` は大文字に変換され、英数字以外は `_` になります。たとえば Provider ID が `local-llm` なら、環境変数名は `SAAA_PROVIDER_LOCAL_LLM_API_KEY` です。Cloud に分類した Provider では `OPENAI_API_KEY` も利用できます。

### LARM Provider

LARM Provider は既定で無効です。オフライン検証済みの経路を開発環境で試す場合だけ、起動時に次の値を設定します。

```sh
export SAAA_LARM_ENABLED=1
export LARM_API_TOKEN="<token>"
bun start
```

機能フラグは起動時に一度だけ読み込まれます。無効へ戻す場合もアプリを再起動してください。本番トラフィックを有効にする前に、[LARM Operations Runbook](spec/docs/mvp-2.6-larm-operations-runbook.html) と現在の release evidence を確認する必要があります。

## 音声プロファイル

Settings → Voice → My voice profile では、利用者本人の声を端末内で照合するフィルターを設定できます。有効化には、3〜12 秒の有効なサンプルを 4 件以上、合計 20 秒以上登録する必要があります。サンプルは最大 5 件です。

音声サンプルは AES-256-GCM で暗号化した WAV としてアプリのデータディレクトリに保存します。話者埋め込みも暗号化して SQLite に保存し、マスターキーは macOS Keychain だけに保存します。フィルターが有効な間は、ローカル照合に通った音声だけをローカル ASR サーバーへ送ります。モデル、鍵、タイムアウト、話者判定で問題が起きた場合、フィルターを迂回して音声を送ることはありません。

この機能は文字起こし時のプライバシーフィルターです。本人確認や、録音した声によるなりすましを防ぐ認証機能ではありません。

## 開発と検証

変更前後の基本確認には次を使います。

```sh
bun run check
bun run build
bun run desktop:smoke
```

- `bun run check`: モジュールサイズ、生成物、型、Rust の format・Clippy、フロントエンドと Rust のテストを確認します。
- `bun run test:coverage`: ローカル用の HTML/LCOV レポートを `coverage/` に出力します。`bun run check` には含まれません。
- `bun run build`: TypeScript を検査し、フロントエンドの production build を作成します。
- `bun run desktop:smoke`: debug 版デスクトップアプリを一時データディレクトリで起動し、IPC の準備完了を確認します。macOS では同梱した話者照合ランタイムも確認します。
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

データベースには、設定、会話、確定済みメッセージ、実行状態、暗号化した話者埋め込みを保存します。モデルの API key、`LARM_API_TOKEN`、ローカル LLM サーバーから受け取った一時的な接続情報は保存しません。

Settings → Privacy & Security から、SQLite の整合性を保ったバックアップと、内容を伏せた診断 JSON を作成できます。診断情報には会話本文、ローカルパス、認証情報を含めません。

データベースのバックアップには、暗号化した音声サンプルと macOS Keychain の鍵が含まれません。そのため、バックアップだけを戻しても音声プロファイルは復元できません。古いスキーマを開く前には、移行前のデータベースバックアップを自動作成します。

## 現在の制約

- Situation は評価用のシャドーモードです。自動介入やアプリ操作は実装していません。
- Meeting はマイク入力だけに対応します。システム音声、翻訳、話者分離、フローティングオーバーレイは利用できません。
- Meeting の文字起こしは、停止後に保存内容を確認して Save を選ぶまでメモリ上に保持されます。保存しなければ破棄されます。
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
- [Internal Design Documents](spec/docs/README.html)
- [MVP 2.6 Release Evidence](spec/docs/mvp-2.6-release-evidence.html)
- [LARM Operations Runbook](spec/docs/mvp-2.6-larm-operations-runbook.html)
- [Runtime Boundary ADR](spec/docs/adr/0001-mvp-runtime-boundaries.html)
- [Situation Privacy ADR](spec/docs/adr/0002-situation-signal-privacy.html)
- [Input Activity Privacy ADR](spec/docs/adr/0003-input-activity-signal-privacy.html)
