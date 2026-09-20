# Personal World Model M3B 完了記録

作成日: 2026-09-20。状態: M3B 実装完了（通常会話への限定投入）。live Provider の回答品質は未検証。計画は `spec/docs/saaa-personal-world-model-m3b-plan.md`。カード別は `m3b-progress.md`。

## 1. 結論

Memory ON の `conversation.respond` だけ、`source_kind=world-model` を高々 1 件の非命令 Candidate として Broker に渡す。要求は resolved な ScopeSnapshot からだけ（Project 1、meeting/coding の current/focus、graph なし）。TTL は 1,000 ms のまま。Expired を Current にしない。再生成 0。

`world-model-shadow` は `record` 先頭で拒否したまま。`run_shadow` は turns から呼ばない。agent_session / 音声 / Codex 本文へは World を配線していない（generation manifest に `world-model` を selected として残さない）。

## 2. ゲート

| ゲート | コマンド | 結果 |
| --- | --- | --- |
| M3B 試験 | `cargo test --locked --manifest-path src-tauri/Cargo.toml --lib m3b_` | 11 passed / 0 failed |
| M3A 試験 | `... --lib m3_` | 21 passed / 0 failed / 1 ignored |
| Context | `... --lib runtime::context` | 45 passed / 0 failed / 1 ignored |
| Clippy | `cargo clippy --locked --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings` | passed |
| size | `bun run size:check` | passed（679 files）。閾値未緩和 |
| spec | `bun run spec:check` | passed |
| check:local | `bun run check:local` | 初回の全並列 `cargo test --lib` は host timeout / sqlite lock で 62 失敗。`test-threads=2` で 964 passed、残り `coding::...protocol_fixture` と generated tools 5 件は単独再実行で passed。M3B 起因の失敗はなし |

filter 0件を成功扱いしていない。

## 3. Memory と compose

| 条件 | World | compose | prepare |
| --- | --- | --- | --- |
| Memory OFF | 載せない | 1 | 0 |
| Project のみ | `empty_request` で省略 | 1 | 0（inspect 前） |
| meeting または coding + Project 1 | 載る（budget/displace 以外） | 2 | 1 |
| would_displace | 外して baseline | 2 | 1 |
| record 直前 Expired | selected に載せない | （compose 後） | 追加 prepare 0 |

receipt は `GenerationHandle` 上の Prepared。SQLite に Frame 本文は無い。dispatch 後 Expired でも `complete()` は成功し、観測は `expired-after-dispatch`。

## 4. 未接続

- agent_session generation manifest
- 音声 TTS / DynamicLan persistence.world
- Codex Provider 本文
- グラフ seeds / ExactName
- live「今会議中ですか」

## 5. live未検証

固定 fixture の Frame JSON（`project_scope` / run / meeting id / coding id）は envelope から一致を確認した。live Provider の発話内容は測っていない。品質完了ではない。

## 6. 引き渡し

TTS ゲート（Step 3）と最小循環（Step 4）には進めてよい。World の live 正答は M4。M3C は未着手。
