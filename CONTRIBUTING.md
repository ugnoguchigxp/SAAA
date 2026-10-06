# Contributing to SAAA

Use the Bun version pinned in `package.json` and the Rust toolchain in `rust-toolchain.toml`. See [README](README.md) for macOS prerequisites and local setup.

Verification and supported build entry points share a repository lock, including Git worktrees and alternate output directories. Concurrent invocations wait without adding success output. Use `bun run build:desktop` for desktop builds and `bun scripts/serial-command.ts -- <command>` for other special build commands. Do not invoke Cargo directly or move build output to evade a wait. `bun run start` and `bun run tauri dev` keep the lock for the development session; exit the session before separate verification. Nested commands inherit the lock. Interrupted commands clean up their compiler process group; after an abrupt parent exit, the next build waits for surviving children. The lock does not prevent multiple rust-analyzer indexing processes.

Use the existing checkout and current branch unless the user asks otherwise. Commit and push save and share checkpoints; they do not require a new branch, a merge, or successful advance/full verification. Run relevant lightweight checks where practical and record passed, failed, and unperformed checks in the commit. Requested checkpoints may save incomplete work or known failures, with that status made explicit. Do not start a whole-project build solely to commit or push, or describe a saved checkpoint as build-ready without successful verification.

Match verification to the changed code and its consumers. Whole-project static checks use `bun run --silent verify`, without builds or tests. Whole-project build readiness and integration acceptance require `bun run --silent verify:advance`, which adds builds and unit/contract tests. Partial build readiness uses scoped advance for every changed crate and affected dependent, for example `bun run --silent verify advance --package crates/larm-session`; use `--scope typescript` for frontend changes and include the applicable IPC/quality contracts. Documentation-only changes require link, command, and factual checks.

Define the affected scope before checking it. Fix failures in that scope before claiming readiness, report known whole-project failures and unperformed checks, and describe a scoped pass as scoped. Shared dependencies, build tooling, IPC, and changes across domains require consumer verification for readiness; use whole-project advance when their impact spans the project. Until feature domains become independent crates, `--package src-tauri` still verifies the whole desktop crate, even with filtered tests. Generated-context and module-size checks remain project-wide where no scoped entry point exists. For version upgrades and major whole-project changes, run `bun run --silent verify:full`, which adds provider/conversation E2E and desktop smoke; major domain changes require relevant integration/E2E checks for that domain and its consumers. Routine partial implementation does not require all E2E.

Individual `lint`, `typecheck`, `build`, `test`, and `format` scripts and their Rust-only `:rust` variants all delegate to the same verify script. Standalone `build:desktop` also goes through verify. Success prints one `OK`; failure prints complete diagnostics and stops. Specification changes also require `bun run spec:check`; permission and audio behavior require the manual evidence described under `spec/docs`. Report unperformed checks explicitly.

Rust IPC contracts are the source of generated frontend bindings. After changing them, run `bun run ipc:generate`, review the generated files, and run `bun run ipc:check`. Receiver fixtures are serialized by Rust; update them with `bun run ipc:fixtures`, then run the frontend IPC validation tests. Do not edit generated bindings directly. Register new source modules with `bun run size:register`; preserve existing size budgets.

Keep behavioral changes, generated output, and mechanical formatting identifiable in the review. Include the problem, resulting behavior, and relevant validation. Never commit credentials, recordings, local databases, or private diagnostics. Preserve the [MIT license](LICENSE) and bundled third-party notices.

Security and conduct reporting contacts have not yet been designated; see [SECURITY](SECURITY.md) and [CODE_OF_CONDUCT](CODE_OF_CONDUCT.md).

## Before opening a pull request

Small documentation fixes can go directly to a pull request. For a new provider, storage migration, or major interaction change, first describe the user problem and proposed behavior in an issue. Keep the scope focused and explain compatibility or data-migration effects.

Use `bun run --silent format` to format TypeScript and Rust, or `bun run --silent verify format --package src-tauri --write` for one Rust package. `bun run --silent format:check` checks without changing files. Focused tests use `bun run --silent verify test --scope typescript -- tests/<file>` or `bun run --silent verify test --package src-tauri -- --lib <filter>`. HTML specifications use `bun run spec:check`; review any fixes before including them. Changes to System Context sources require `bun run s11tnext:build`. Do not raise existing module-size limits to hide growth.

When submitting code, identify a regression test or a concrete validation procedure for the changed behavior. Documentation-only changes need link, command, and factual checks rather than a full desktop rebuild. Explain skipped checks in the pull request template. Contributions are reviewed under this repository's [MIT license](LICENSE); preserve third-party attribution for any imported material.

## 日本語

ドキュメントの小さな修正はPRから始められます。新しいProvider、保存形式の移行、大きな操作変更は、先にIssueで目的と変更後の動作を共有してください。

既存の作業フォルダと現在のブランチを使います。commit・pushは作業の保存・共有であり、新しいブランチやマージ、advance・fullの成功を必須条件にはしません。実行可能な範囲の軽い確認を行い、成功・失敗・未実施をcommitに記録します。依頼された保存では、作業途中や既知の検証失敗も状態を明記して保存できます。commit・pushだけを理由に全体ビルドを始めたり、保存できたことを完成確認済みと扱ったりしないでください。

検証範囲は変更したコードとその利用先に合わせます。全体の静的確認は`bun run --silent verify`、全体ビルドの完成確認・統合の受入確認はbuildとユニット・契約テストを含む`bun run --silent verify:advance`を実行します。部分ビルドの完成確認では、変更したcrateと影響を受ける依存先ごとに`bun run --silent verify advance --package <crateのディレクトリ>`を実行します。フロントエンドは`--scope typescript`を使い、IPC・品質契約に関わる場合は該当する契約検証も加えます。

完成確認済みとする前に対象範囲の失敗を修正し、既知の全体検証失敗と未実施の確認は報告します。共通依存、ビルド基盤、IPC、複数ドメインの変更は完成確認時に利用先まで確認し、影響が全体に及ぶ場合は全体advanceを実行します。crate分割前の`src-tauri`はテストを絞ってもcrate全体がコンパイル対象です。生成context・ファイルサイズの確認には、まだ全体共通の検証が残っています。バージョンアップなど全体の大きな更新ではE2Eを含む`bun run --silent verify:full`、ドメイン内の大きな更新ではそのドメインと利用先の統合・E2E確認を行います。

仕様書の変更では`bun run spec:check`も実行します。`lint`・`typecheck`・`build`・`test`・`format`とRust用の`:rust`コマンド、単体の`build:desktop`も同じverifyを経由します。成功時は`OK`のみ、失敗時は診断を省略せず表示して後続を停止します。文書だけの変更ではリンク、コマンド、実装との一致を確認してください。未実施の確認は理由を記載します。

Rust IPC変更時は`bun run ipc:generate`と`bun run ipc:fixtures`で生成物を更新してください。認証情報、会話、録音、ローカルDBをcommitせず、MIT本文と第三者NOTICEを保全してください。
