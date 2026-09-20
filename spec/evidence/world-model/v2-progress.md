# Personal World Model v2 進捗記録

計画: [initial-plan](saaa-personal-world-model-initial-plan.md) / 契約: [execution-contract](saaa-personal-world-model-v2-execution-contract.md) / カード: [work-cards](saaa-personal-world-model-v2-work-cards.md) / 結果: [v2-results](v2-results.md)

## D00 ベースライン

- HEAD: `95eb761`（feat: add personal world model and tool selection foundations）。
- DB版: 着手時 `22` → D18 で `23`。
- 旧 M1 の core/adapter 実装を維持し、新規 v2 層を追加した。計画外の `tool_selection` 差分は本作業では触れていない。
- 既存 K1〜K4 は D00 時点で成功（`v2-results.md` のゲート表）。

## カード状態（全43枚）

| カード | status | 主な変更 | 検証 |
| --- | --- | --- | --- |
| D00 | done | 本記録・baseline | docs |
| D01 | done | 変更なし。R1 既存試験を再実行 | r01 |
| D02 | done | 変更なし。R2 既存試験を再実行 | r02 |
| D03 | done | 変更なし。core 境界試験を再実行 | r04/r09/r10 |
| D04 | done | 変更なし。R3/R5〜R8 既存試験 | r03/r05〜r08 |
| D05 | done | `personal_state/tests.rs` に World 入り復元 fixture | `world_projection_does_not_resurrect_a_deleted_source_on_restore` |
| D06 | done | `core/world/model_v2.rs` | d06_ ×3 |
| D07 | done | `core/world/slice_v2.rs`（DTO） | d07_ |
| D08 | done | `core/world/versioned.rs` | d08_ ×4 |
| D09 | done | `core/world/identity_v2.rs` | d09_ ×4（golden hash） |
| D10 | done | `core/world/validation_v2.rs` | d10_ ×2 |
| D11 | done | 同（端点マトリクス） | d11_ ×4 |
| D12 | done | 同（根拠・評価・mechanism） | d12_ |
| D13 | done | 同（検証順序・版横断重複・容量） | d13_ ×2 |
| D14 | done | `core/world/conditions_v2.rs` | d14_ |
| D15 | done | 同（availability） | d15_ |
| D16 | done | adapter `validation_v2.rs`（versioned loader + validate_commit_v2） | d21/d36/d37 経由 |
| D17 | done | `store.rs` の再送判定を意味検証前に分離 | d37, t11 |
| D18 | done | `projection_version` 列 + DB版 23 | t08 |
| D19 | done | adapter `projection_v2.rs`（v1/v2 混在 rebuild） | d21/d38/d39 |
| D20 | done | 旧 `activate` の version 検査、未知 kind fallback 廃止 | d20_ |
| D21 | done | adapter `query_v2.rs`（ActivateInputV2/load_query_context_v2） | d21_ ×3 |
| D22 | done | adapter `observations_v2.rs` | d39_ |
| D23 | done | 読取時の Goal/Objective 再検証 | d38_goal_retraction |
| D24 | done | `core/world/traversal_v2.rs` | d24_ |
| D25 | done | evaluate_path / effect_summary（567） | d25_ ×3 |
| D26 | done | Goal 4 hop 到達 | d26_ |
| D27 | done | `core/world/relevance_v2.rs`（相関・依存変換） | d28_ 内 |
| D28 | done | Gap 六種・key・固定文 | d28_ ×3 |
| D29 | done | Focus 順位・request attention | d29_ |
| D30 | done | `slice_v2.rs` の unit union・参照閉包・byte 選択 | d30_ ×5 |
| D31 | done | adapter BFS frontier 取得と fetch/scan 計数 | d31_ |
| D32 | done | `activate_v2` 完成・flags・read-only | d32_ ×2, d38, d39 |
| D33 | done | Writer 配線（validate_commit_v2 + rebuild_v2） | d21/d36/d37/d38/d39 |
| D34 | done | `core/world/outcome_v2.rs`（反証・640） | d34_ ×4 |
| D35 | done | OutcomeUpdate 再計算検証 | d37_ |
| D36 | done | adapter `outcome_v2.rs` prepare_outcome_patch | d36_ |
| D37 | done | outcome 再送/CAS | d37_ |
| D38 | done | §9 A/B 統合 | d38_ ×2 |
| D39 | done | §9 C/D/E/F 統合 | d39_, d36_, d37_ |
| D40 | done | boundary 回帰（snapshot 除外） | `world_assertions_stay_out_of_the_public_snapshot` |
| D41 | done | v2 性能 fixture（ignored） | `d41_v2_performance` |
| D42 | done | 結果表・ゲート | `v2-results.md` |

## 変更ファイル

core 新規: `core/world/{model_v2,slice_v2,versioned,identity_v2,validation_v2,conditions_v2,traversal_v2,relevance_v2,outcome_v2}.rs`
core 変更: `core/world/mod.rs`（登録）、`core/world/validation.rs`（`WorldError::DuplicateIdentity`）、`core/reducer.rs`（World の版横断 Supersede と `patch_fingerprint` 公開）、`core/world/traversal_v2.rs`（strength/evidence、maximal_only_v2）
core テスト: `core/tests/world_v2_validation.rs`

adapter 新規: `world/{validation_v2,projection_v2,query_v2,observations_v2,outcome_v2,v2_tests}.rs`
adapter 変更: `world/{mod,validation,projection,query}.rs`、`world/{schema.sql,test_support,tests}.rs`、`personal_state/{schema,store,tests}.rs`、`persistence/schema.rs`

証跡: `spec/evidence/world-model/v2-progress.md` / `v2-results.md`、`scripts/module-size-baseline.json`（新規6 module と `schema.rs`/`world/mod.rs` のレビュー済み baseline 更新）

## コードレビューと改善（レビュー後）

初回実装をレビューし、以下の実装漏れ・不具合を修正した。

| 指摘 | 修正 |
| --- | --- |
| `WorldSliceV2.focus` が常に空 | `SliceUnitV2` に `focus` を追加し、到達 Focus と seed Focus を unit 化。d38_focus / d20 で確認 |
| seed のみで edge がない照会が空 | seed 自身を説明単位として必ず返す（深さ0） |
| 明示質問の未知 seed で Gap が出ない | 早期 return を分岐し `missing_knowledge` を返す。d28_unknown_explicit_seed |
| `activate_v2` が v1-only 投影を stale 扱い | `projection_version` 1/2 を受理。d21_activate_v2_reads_a_v1_only_projection |
| 逆方向因果の合成順が未反転 | `path_view` で宣言順に反転評価。d25_reverse_causal_search |
| `goals=false` でも Goal Focus が残る | Focus 候補を Goal 除外。d32_goals_flag / d32_all_flag_combinations |
| `disabled:*` notice なし | flag 無効時に `disabled:<kind>` を付与 |
| seed の正規化前 4 件上限 | 正規化後 dedup してから上限判定 |
| 観測の relation id 未検証 | WorldRelation かつ access 許可のみ受理。d22 | 
| `store::commit` が非トランザクション | `is_autocommit` 時に transaction を開始。fault 注入で全 rollback を確認。d19_failed_commit |
| BFS が relevance 深さ+1 を余分に取得 | 展開ラウンドを `0..max_depth` に修正 |
| 読取時 Goal/Focus の Objective スコープ未再検証 | access.task_request を再検査 |
| v1 continuity の Supersede と World 版横断 | reducer の Supersede を World kind かつ version prefix 差のみ許可。v1→v2 置換を d13_v1_relation で end-to-end 確認 |
| 関係 12 型の網羅なし | 正例 13 / 負例 6 を追加 |
| flags 16 組合せ未検証 | d32_all_flag_combinations |
| 関係の正例/負例が一部のみ | 12型正例13・負例6を追加 |
| optional field 省略の decode | d06_missing_optional_fields |
| effect summary の対立 | d25_opposing_directions |
| dependency 変換 | d27_dependency_reports_required_entity_and_availability |
| 低評価境界 | d28_low_confidence_boundary |
| 楽観的 store::commit の原子性 | transaction 化 + d19_failed_commit |

## 未解決事項

1. **v1→v2 Supersede**: 契約 C2 と reducer の `semantic_key` 一致要求が両立しないため、`reducer.rs` の Supersede 検査を「World kind 同士なら論理同一性は versioned validator に委ねる」へ変更した。v1 continuity の挙動は不変。
2. **全 lib の flake**: `cargo test --lib` 全体で `tests::codex_app_server_contract_covers_start_stream_resume_and_cancel` が一度失敗したが、単体実行と再実行では成功。最終の全 lib は **734 passed / 0 failed / 14 ignored**。World と無関係な既存 flake。
3. **docs チェック**: `spec-html check` は `saaa-llang-dynamic-capability-m2b-transport-reference.md` の MD001（別作業の新規文書）のみ失敗。本作業の文書は通過。
4. **`check:local` 全体**: `format:check` / `lint` / `cargo fmt --check` / `clippy -D warnings` / K1〜K5 は成功。`check:local` の `check`（frontend build + 全 test）は未通し。
5. **M2 以降**: `activate_v2` は内部専用で Context Broker へ未接続。Runtime/外部 Evidence、`runtime_ref`、現在状態、LLM 候補抽出、必要時推論は未実装（計画どおり M2/M3）。
6. **性能**: 10,000 件は build が 5 秒を超えるため未認定。100 件までは安定。`v2-results.md` 参照。
