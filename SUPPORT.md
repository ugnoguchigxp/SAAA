# Getting help / 困ったとき

Start with [README](README.md) / [日本語README](README.ja.md). SAAA is an MVP; a response time or commercial support service is not promised.

## Troubleshooting

| Symptom | First checks |
| --- | --- |
| Dependency installation differs between machines | Use Bun and Rust versions pinned in `package.json` and `rust-toolchain.toml`; run `bun install --frozen-lockfile`. |
| The UI opens but desktop commands fail | Start with `bun start`. The browser-only development server does not provide Tauri IPC. |
| A model connection fails | Check the status shown in Settings: authentication failure, timeout, and unreachable endpoints have different recovery steps. `LARM_API_TOKEN` is required only when the local server enforces Bearer authentication. |
| Voice input is unavailable | Check the ASR route, microphone permission, and input device. If the speaker filter is enabled, verify the enrollment status. |
| Startup reports an existing database owner | Quit the other SAAA process using that data directory. Do not remove the writer lock while the application is running. |
| A generated-file check fails | Follow the generation commands in [CONTRIBUTING](CONTRIBUTING.md); do not hand-edit generated bindings. |

For a reproducible, non-sensitive bug or feature request, use this repository's Issues and the provided templates. Include the commit, OS and architecture, steps, expected and actual results, and checks already attempted. Remove secrets and personal information from screenshots and logs. A minimal reproduction is more useful than a full database or recording.

Settings → Privacy & Security can export redacted diagnostics. Review the export before sharing; do not attach databases, backups, audio samples, tokens, or private conversation text. Security vulnerabilities belong under the process in [SECURITY](SECURITY.md), not a public bug report.

## 日本語

最初に[日本語README](README.ja.md)を確認してください。開発中のMVPのため、回答期限や商用サポートは約束していません。

- 起動・インストール：指定されたBunとRustを使い、`bun install --frozen-lockfile`の後に`bun start`で起動します。
- 接続エラー：Settingsに表示される認証失敗、タイムアウト、到達不能の区別を確認します。`LARM_API_TOKEN`は接続先がBearer認証を要求する場合だけ必須です。
- 音声入力：ASRの経路、マイク権限、入力デバイスを確認します。話者フィルターを有効にした場合は登録状況も確認します。
- データベースの使用中エラー：同じデータを使う別のSAAAを終了します。実行中にwriter lockを削除しないでください。

通常の不具合や機能提案はIssueテンプレートを使い、commit、OS・CPU、再現手順、期待と実際の結果を記載してください。診断JSONも共有前に確認し、データベース、録音、会話本文、認証情報を添付しないでください。脆弱性は公開Issueへ書かず、[SECURITY](SECURITY.md)を参照してください。
