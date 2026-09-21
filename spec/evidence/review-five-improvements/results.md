# 再レビュー改善5点 実行結果

日付: 2026-09-21。

## 実行コマンドと件数

| コマンド | 結果 |
| --- | --- |
| `cargo test --manifest-path src-tauri/Cargo.toml --lib rf5_` | **14 passed**, 0 failed |
| `cargo test --manifest-path src-tauri/Cargo.toml --lib dw_` | **49 passed**, 0 failed |
| `cargo test --manifest-path src-tauri/Cargo.toml --test ipc_contract_bindings --test voice_asr_contract_bindings` | **6 passed** |
| `bun test tests/steward-panel.test.tsx tests/adaptive-improvement-section.test.tsx` | **3 passed** |
| `bun run typecheck` | 成功 |
| `bun run ipc:generate` 後 `ipc:check` | 成功（生成 bindings 一致） |
| `bun run size:check` | **失敗**。新規 file は `size:register` 対象。既存 ratchet 超過は今回差分と事前 dirty（role_routing 等）が混在。未実施を成功に数えない |

## rf5_ 内訳

- V: job ID/空digest は Pass にならない。settled+証拠欠損で Goal/後続 step が進まない。Missing は成功ラベルにならない
- N: 内容 digest。禁止・混在・引用説明・hedge。intake が全文を見る
- W: drain 中 signal が pending 放置にならない
- G: 4 intent の口語、malformed JSON / 未知 intent / scope 注入拒否、指示語
- A: 不完全 pair 拒否、baseline 0 拒否、有効化前後の choose、rollback で rules 復帰

## 残件（完了に含めない）

1. 実対応 coding profile での read/test 委任の成果取得（外部ランタイム）
2. アプリ稼働・idle での drain 開始 1 秒 / text report 2 秒の実測
3. 実モデルによる World 解釈精度（fixture 理解器の接続は実装済み）
4. gate を満たす実測 evaluation bundle。現状は「接続実装完了・改善効果未実証」
5. `bun run size:check` の既存 ratchet 超過の整理

## 実装と効果の区別

通常経路への接続（verifier、intake、Wake、query 理解、評価 IPC/UI、choose）は実装した。学習が実データで良くなったこと、自然文理解の実精度、通知遅延の実測は未実証。
