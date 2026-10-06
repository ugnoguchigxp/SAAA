# 自己診断 v2 実装記録

作業日: 2026-10-06。契約: `spec/docs/saaa-self-diagnosis-v2-contract.md`。旧実装（2026-09-22）は全面置換した。

## 通過

- `cargo test --lib diagnosis` — 集約規則、store、engine（個別タイムアウト、quick に外部送信 check が無いこと、新規インストールで緑にならないこと）、各 check。
- `cargo test --test ipc_contract_bindings`
- `bun test tests/diagnosis-api.test.ts tests/diagnosis-model.test.ts tests/diagnosis-hook.test.tsx tests/diagnosis-page.test.tsx tests/diagnosis-nav.test.tsx tests/conversation-behavior-menu.test.tsx tests/ui-shell-redesign.test.ts`
- `bun run typecheck`、`bun run lint`、`bun run format:check`

## 既知の失敗（今回の対象外）

`bun run size:check` と `cargo clippy` は、他の作業による既存の違反で止まる。診断関連ファイルの `scripts/module-size-baseline.json` は現在の行数で更新した。

## 未確認

実アプリ（`bun run tauri dev`）での起動から画面表示までの確認。
