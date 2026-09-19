# SAAA Personal World Model M1 進捗記録

## ベースライン (WM-00)

- HEAD: `b3c4606`（計画作成時 `1b595ff` から進行。差分は capability/invocation 関連で、
  World 契約・Scope 契約の正本は未変更）
- DB版: 実装時点で `20` → M1 で `21` へ追加型更新。
- 現行上限: patch 16 KiB / assertion+transition 32件 / payload 2,000 byte を維持。
- 既存の未追跡ファイル（`generated_capabilities/*`、`providers/chat_completions/
  generated_tools_tests.rs` 等）は M1 の所有外。size ratchet 超過は未解消として報告。

## カード状態

| カード | status | 主な変更 | 検証 |
| --- | --- | --- | --- |
| WM-00 | done | spec/evidence/world-model/m1-baseline.md, m1-progress.md | K1 |
| WM-01 | done | core world/{model,identity,mod}.rs, lib.rs, tests/world_contract.rs | T01,T02,K1 |
| WM-02 | done | core model.rs Kind, worker/projection/commands/product_extract/live_harness/task_bundle の隔離 | T03,K1,K3 |
| WM-03 | done | core world/validation.rs | T04,T05,K1 |
| WM-04 | done | core world_contract.rs, world/traversal.rs, adapter tests T06/T07 | T06,T07,K1 |
| WM-05 | done | world/schema.sql, personal_state/schema.rs, persistence/schema.rs 21, tests T08 | T08,K4 |
| WM-06 | done | world/projection.rs, test_support | T09 |
| WM-07 | done | store.rs commit hook + rebuild, world/validation.rs | T10,T11,K2 |
| WM-08 | done | world/schema.sql forget trigger, store rebuild | T12,T13,K4 |
| WM-09 | done | world/query.rs（名称解決・境界・pending/stale） | T14,T15 |
| WM-10 | done | core world/traversal.rs, world/query.rs | T16,T17 |
| WM-11 | done | traversal causal / conditions | T18,T19 |
| WM-12 | done | core world/relevance.rs（Focus 順位・Gap） | T20 |
| WM-13 | done | WorldSlice, relevance trim, query | T21 |
| WM-14 | done | world/tests.rs 統合 + m1-results.md + 性能測定 | T22,K1-K5 |
| WM-15 | done | 回帰確認・引き渡し | `bun run check:local` ok / `cargo test` 653 passed / core 26 passed / `test:rust-packages` ok / `size:check` ok |

## 変更ファイル

core:
- `crates/personal-state-core/src/model.rs`（World Kind 3種 + is_world/is_continuity）
- `crates/personal-state-core/src/lib.rs`（world module 公開）
- `crates/personal-state-core/src/world/{mod,model,identity,validation,traversal,relevance}.rs`
- `crates/personal-state-core/tests/{world_contract,world_traversal}.rs`

adapter (`src-tauri/src`):
- `memory/personal_state/mod.rs`（world module 宣言）
- `memory/personal_state/schema.rs`（recover 前の World DDL + forget trigger）
- `memory/personal_state/store.rs`（共通 commit の World 検証 hook + rebuild 接続）
- `memory/personal_state/worker.rs`（current/依存から World 除外 + World Kind 拒否）
- `memory/personal_state/{projection,commands,product_extract,task_bundle,live_harness}.rs`
- `memory/personal_state/world/{mod,validation,projection,query,schema.sql,test_support,tests}.rs`
- `persistence/schema.rs`（DB版 21）

evidence / baseline:
- `spec/evidence/world-model/{m1-baseline,m1-progress,m1-results}.md`
- `scripts/module-size-baseline.json`（World 新規 module のみ登録）

その他: `src-tauri/src/providers/chat_completions/generated_tools_tests.rs` の
trailing blank line を rustfmt で整えた（M1 所有外・整形のみ）。

## レビュー指摘の反映 (R1〜R10)

同じ World 実装に対するレビュー修正が並行して入り、以下を統合済み。

- 認可: principal・classification・失効 Scope の拒否、時間切れ Focus/Entity の除外。
- 探索: Edge 取得が scan 予算を消費しないこと、条件不一致でも有効な短経路を残すこと、
  causal reverse が forward edge を辿らないこと、prefix 経路が返却上限を消費しないこと。
- Gap: 無関係な Focus の Gap を混ぜない `(focus, edge)` 対応。
- Slice: Node 上限、byte 予算不足の明示エラー、参照欠落 Focus/Gap の prune。
- 追加テスト: `r01,r02,r03,r05,r06,r07,r08`（adapter）、`r04,r09,r10`（core）。

## 仕様との差分・未解決

- World の実際の Provider/Context Broker 接続、会話抽出、Runtime 取り込みは M1 対象外。
  `world::query::activate` はテストからのみ呼ぶ。
- 10,000 項目測定は fixture 構築費が中断条件を超えるため未実施（m1-results.md に理由）。
- `bunx --bun spec-html check ./spec/docs --warnings-as-errors` は別作業の
  `spec/docs/saaa-llang-dynamic-capability-m2b-transport-reference.md` の
  MD001 二重 H1 のみ失敗する。M1 の文書は通過しており、無関係な修正は行わない。
- 並行作業による既存差分（generated_capabilities/* 等）は M1 の所有外。

## レビュー対応追記（2026-09-20）

`m1-review-2026-09-20.md` の R1〜R10 を修正し、回帰試験と再検証を
`m1-results.md` の「レビュー指摘 R1〜R10 の対応」に記録した。カード状態は
WM-00〜15 のまま変更しないが、R1・R2（読み取り時の認可・現在性）、
R3〜R5（探索予算・逆探索・条件）、R6〜R10（Focus/Gap/byte/経路上限）は
それぞれ専用テストで確認済みである。T13 の対応先と既存 Continuity 件数の
記述は `m1-results.md` で訂正した。
