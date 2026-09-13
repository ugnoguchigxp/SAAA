# Contributing to SAAA

Use the Bun version pinned in `package.json` and the Rust toolchain in `rust-toolchain.toml`. See [README](README.md) for macOS prerequisites and local setup.

For code changes, run `bun run check:local` (format check, lint, then the existing full check), `bun run test:rust-packages`, and `bun run spec:check`. Desktop lifecycle changes also require `bun run desktop:smoke`; permission and audio behavior require the manual evidence described under `spec/docs`. Report unperformed checks explicitly.

Rust IPC contracts are the source of generated frontend bindings. After changing them, run `bun run ipc:generate`, review the generated files, and run `bun run ipc:check`. Receiver fixtures are serialized by Rust; update them with `bun run ipc:fixtures`, then run the frontend IPC validation tests. Do not edit generated bindings directly. Register new source modules with `bun run size:register`; preserve existing size budgets.

Keep behavioral changes, generated output, and mechanical formatting identifiable in the review. Include the problem, resulting behavior, and relevant validation. Never commit credentials, recordings, local databases, or private diagnostics. Preserve the [MIT license](LICENSE) and bundled third-party notices.

Security and conduct reporting contacts have not yet been designated; see [SECURITY](SECURITY.md) and [CODE_OF_CONDUCT](CODE_OF_CONDUCT.md).

## Before opening a pull request

Small documentation fixes can go directly to a pull request. For a new provider, storage migration, or major interaction change, first describe the user problem and proposed behavior in an issue. Keep the scope focused and explain compatibility or data-migration effects.

Use `bun run format` for managed TypeScript sources and `cargo fmt --manifest-path src-tauri/Cargo.toml` for the desktop Rust crate. HTML specifications use `bun run spec:check`; review any fixes before including them. Changes to System Context sources require `bun run s11tnext:build`. Do not raise existing module-size limits to hide growth.

When submitting code, identify a regression test or a concrete validation procedure for the changed behavior. Documentation-only changes need link, command, and factual checks rather than a full desktop rebuild. Explain skipped checks in the pull request template. Contributions are reviewed under this repository's [MIT license](LICENSE); preserve third-party attribution for any imported material.

## 日本語

ドキュメントの小さな修正はPRから始められます。新しいProvider、保存形式の移行、大きな操作変更は、先にIssueで目的と変更後の動作を共有してください。

コードの変更では`bun run check:local`、`bun run test:rust-packages`、`bun run spec:check`を基本確認とします。デスクトップの起動・終了を変更した場合は`bun run desktop:smoke`も実行します。文書だけの変更ではリンク、コマンド、実装との一致を確認してください。未実施の確認は理由を記載します。

Rust IPC変更時は`bun run ipc:generate`と`bun run ipc:fixtures`で生成物を更新してください。認証情報、会話、録音、ローカルDBをcommitせず、MIT本文と第三者NOTICEを保全してください。
