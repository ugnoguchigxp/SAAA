# 自己診断 実装進捗

作業開始: 2026-09-22T12:57:32Z。HEAD: `8b5159a`。

開始時の未コミット差分は `spec/docs/saaa-startup-self-diagnosis-implementation-plan.md`（未追跡）のみ。`src-tauri/src/persistence/schema.rs` と role routing の schema は変更していない。

| カード | 完了 |
| --- | --- |
| DG-00 | 2026-09-22T12:57:32Z |
| DG-01〜DG-16 | 2026-09-22T13:16:00Z |

## 検証

通過:

- `cargo test --manifest-path src-tauri/Cargo.toml diagnosis` — diagnosis 21 件と `dg_11_export_includes_self_diagnosis` を含む。`dg_06` の `127.0.0.1:9` は `harness.resolve` の Fail 1 件。`dg_08_collect_completes_on_fresh_state` は外部待ちを避けるため、初期化済み DB の Harness アドレスを空にし、到達不能な loopback LLM と System TTS だけを有効にした。
- `cargo test --manifest-path src-tauri/Cargo.toml --test ipc_contract_bindings` — 7 件。`src/lib/generated/diagnosis.ts` の status は `"ok" | "warn" | "fail" | "skipped" | "running"`。
- `bun test tests/diagnosis-api.test.ts tests/diagnosis-hook.test.tsx tests/diagnosis-modal.test.tsx tests/conversation-behavior-menu.test.tsx` — 4 件。
- `bun run typecheck`、`bun run lint`、`bun run format:check`。

`bun run check:local` はリポジトリ既存の `size:check` と `cargo clippy --all-targets -- -D warnings` で止まる。今回追加したファイルは `scripts/module-size-baseline.json` に現在行数で登録した。`ipc_contract/bindings.rs` は 51 行で既存の ratchet 内。自己診断コードの clippy 指摘（冗長なクロージャ）は修正済み。残る clippy 失敗は `tool_selection/mock_tests.rs` の重複 attribute、`providers/session_store.rs` の未使用 `run`、role routing / runtime 側の既存指摘で、この作業の対象外。

手動の `bun run tauri dev` は起動を試したが、`src-tauri/resources/bin/role-routing-codex` の変更検知で再ビルドが繰り返され、`self diagnosis published` まで到達しなかった。プロセスは停止した。起動ログの確認行は `self diagnosis published revision=1 overall=...`。歯車メニューの「自己診断」、再診断、Escape はフロントの試験で確認した。
