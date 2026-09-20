# Personal World Model v2 結果

作成日: 2026-09-20。計画: [initial-plan](saaa-personal-world-model-initial-plan.md)。

## ゲート (K1〜K6)

| ゲート | コマンド | 結果 |
| --- | --- | --- |
| K1 | `bun run check:personal-state` | 成功（fmt / clippy -D warnings / test） |
| K2 | `cargo test --locked --manifest-path src-tauri/Cargo.toml memory::personal_state` | **77 passed / 0 failed / 1 ignored** |
| K3 | `cargo test --locked --manifest-path src-tauri/Cargo.toml runtime::context` | **13 passed / 0 failed** |
| K4 | `cargo test --locked --manifest-path src-tauri/Cargo.toml persistence::` | **82 passed / 1 ignored / 0 failed** |
| K5 | `bun run size:check` | `module-size ok (620 files)` |
| K5 | `bunx --bun spec-html check ./spec/docs --warnings-as-errors` | 別作業の `saaa-llang-dynamic-capability-m2b-transport-reference.md` MD001 のみ失敗。本作業の文書は通過 |
| K6 | `bun run test:rust-packages` | 成功 |
| K6 | `cargo fmt --check` / `cargo clippy -D warnings`（src-tauri） | 成功 |
| K6 | `bun run format:check` / `bun run lint` | 成功 |
| K6 | `bun run check:local`（全体） | 未通し（`check` の frontend build + 全 test を含む）。上記の個別ゲートで代替 |

core テスト内訳: lib 38 / continuity 17 / world_contract 2 / world_traversal 9 / **world_v2_validation 15**、failure 0。全 lib `cargo test --lib` は 746 passed / 0 failed / 14 ignored（レビュー修正後、外部変更の breakage 前）。

## カード → 試験対応

core（`crates/personal-state-core`）:

| カード | 試験 |
| --- | --- |
| D06 | `world::model_v2::tests::d06_*`（3） |
| D07 | `world::slice_v2::tests::d07_empty_slice_roundtrips` |
| D08 | `world::versioned::tests::d08_*`（4） |
| D09 | `world::identity_v2::tests::d09_*`（4, golden `wm2:67f6…9fe0`） |
| D10 | `world_v2_validation::d10_*`（2） |
| D11 | `world_v2_validation::d11_*`（4） |
| D12 | `world_v2_validation::d12_*` |
| D13 | `world_v2_validation::d13_*`（2） |
| D14 | `world::conditions_v2::tests::d14_*` |
| D15 | `world::conditions_v2::tests::d15_*` |
| D24 | `world::traversal_v2::tests::d24_*` |
| D25 | `world::traversal_v2::tests::d25_*`（567, comparison, conditions） |
| D26 | `world::traversal_v2::tests::d26_goal_reachable_through_metrics_and_project` |
| D27 | `world::relevance_v2::tests::d28_*`（相関） |
| D28 | `world::relevance_v2::tests::d28_*`（3） |
| D29 | `world::relevance_v2::tests::d29_*` |
| D30 | `world::slice_v2::assemble_tests::d30_*`（5） |
| D34 | `world::outcome_v2::tests::d34_*`（4） |
| D35 | `world_v2_validation::d13_*` / `d12_*`（counterevidence 再計算） |

adapter（`src-tauri`）:

| カード | 試験 |
| --- | --- |
| D01 | `world::tests::r01_*` |
| D02 | `world::tests::r02_*` |
| D04 | `world::tests::r03_* / r05_* / r06_* / r07_* / r08_*` |
| D05 | `personal_state::tests::world_projection_does_not_resurrect_a_deleted_source_on_restore` |
| D16 | `world::v2_tests::d36_/d37_/d39_`（Writer 経由） |
| D17 | `world::v2_tests::d37_*`、`world::tests::t11_*` |
| D18 | `world::tests::t08_*`（projection_version 列・default 1） |
| D19 | `world::v2_tests::d21_/d38_/d39_`（v2 投影） |
| D20 | `world::v2_tests::d20_v1_reader_treats_a_v2_projection_as_stale` |
| D21 | `world::v2_tests::d21_*`（3） |
| D22 | `world::v2_tests::d39_condition_observation_makes_the_relation_satisfied` |
| D23 | `world::v2_tests::d38_goal_retraction_removes_current_importance` |
| D31 | `world::v2_tests::d31_two_hop_relation_is_fetched_from_the_second_frontier` |
| D32 | `world::v2_tests::d32_*`（2）、`d38_*`、`d39_*` |
| D33 | `world::v2_tests::d36_/d37_/d38_/d39_` |
| D36 | `world::v2_tests::d36_*` |
| D37 | `world::v2_tests::d37_*` |
| D38 | `world::v2_tests::d38_*`（2） |
| D39 | `world::v2_tests::d39_*`、`d36_`、`d37_` |
| D40 | `personal_state::tests::world_assertions_stay_out_of_the_public_snapshot` |
| D41 | `world::v2_tests::d41_v2_performance`（ignored） |
| D42 | 本書 |

## 性能 (D41)

環境: 同一端末・debug build。fixture は v2 Entity を 8 件/patch で commit。warm-up なし・5 サンプル median/max。単位 ms。

```sh
cargo test --locked --manifest-path src-tauri/Cargo.toml --lib d41_v2_performance -- --ignored --nocapture
```

| scale | build | load median/max | rebuild median/max | query median/max | 中断 |
| --- | --- | --- | --- | --- | --- |
| 0 | 0 ms | 0.09 / 0.95 | 0.21 / 0.29 | 0.22 / 0.84 | no |
| 100 | 530 ms | 7.98 / 9.03 | 31.87 / 37.78 | 17.02 / 18.11 | no |
| 1,000 | 6,220 ms | 19.28 / 19.30 | 119.75 / 124.33 | 49.39 / 50.18 | **yes（build > 5s、部分投影）** |
| 10,000 | — | — | — | — | 未実施（1,000 で中断条件に到達） |

- 10,000 件を製品容量として認定しない。commit は 1 Writer transaction で全投影再構築を含むため、件数に超線形。
- 対話遅延の合格値はこの段階では定めない。

## 受入シナリオ §9 の達成

- A/B: D38 で Goal 経由の関連経路（技術→指標→Goal←Project）と相関の unknown_causal_direction、Goal 撤回反映を確認。
- C: D25 で 900/800/700 の 3 hop → decrease・567、comparison 不一致で unknown/null。
- D: D22/D31 で availability の available/unavailable/unknown を観測から区別（Node 存在は使わない）。
- E: D21/D28 で未知 seed は missing_knowledge、架空 edge なし。不成立条件は有効な短い経路を消さない（R5 既存）。
- F: D34/D36/D37/D39 で期待 increase・実測 decrease → disputed・640、再送 no-op、旧版再利用拒否。

## 外部変更による一時的なビルド不能（本作業外）

レビュー最終確認中に、同時進行中の別作業と思われる未コミット変更
`src-tauri/src/providers/openai_compatible.rs`（変更）と
`src-tauri/src/providers/openai_compatible/structured.rs`（新規）が
`cargo clippy --all-targets -D warnings` でコンパイル不能になった
（unused import ×2、`Zeroizing<String>` の `Display` 未実装）。World とは無関係。

この影響で最終時点の K2/K3/K4・全 lib の再実行ができないため、最後に成功した
数値（K2 77 / K3 13 / K4 82 / 全 lib 746、core lib 38 / world_v2_validation 15 /
v2_tests 24）を記録する。World 所有ファイルは fmt / clippy / test 済み。

## 残課題

- 全 lib `cargo test --lib` は **734 passed / 0 failed / 14 ignored**。途中 `codex_app_server_contract_*` が一度だけ失敗したが、単体・再実行では成功する World 外の既存 flake。
- `spec-html` の外部 MD001、`check:local` の全体通しは未解消。
- M2 以降（Runtime/外部 Evidence、現在状態、LLM 候補抽出、必要時推論、Broker 接続）は計画どおり未実装。
