# Personal World Model M2A 完了記録

作成日: 2026-09-20。状態: M2A 実装完了。カード別の詳細と期待値は [m2-progress.md](m2-progress.md)、計画は [M2計画](../../docs/saaa-personal-world-model-m2-plan.md)、固定契約は [M2実行契約](../../docs/saaa-personal-world-model-m2-execution-contract.md) を参照する。

## 1. 結論

M2A（現在状態付き WorldFrame を安全な参照境界で取得・再検証する内部API）を実装した。永続 WorldSliceV2 を変更せず、`WorldFrame` で包み、会議・coding の現在状態を正本から都度読む。通常会話・Context Broker・IPC からは未接続であり、M3 の前提部品である。

M2A では次を保証する。

- 認可: 信頼済み `AccessRequest`、running run、記録 Scope と現在 epoch、Project→対象の直接 link を同一 DB snapshot で検査する。不許可は要求全体を拒否し、所有者 ID を探す前に止める。
- 現在性: 期限（TTL <= 1,000 ms）と再検証の両方で判定し、期限前でも ledger revision / epoch / policy / scope link / Source / owner digest の変化を拒否する。
- 省略: 容量・予算・owner 不整合は対象単位または graph 単位で省略し、根拠や条件を部分的に削らない。
- 非所有: Runtime 状態の複製保存、Goal の新規採用、Project と会議・Task の自動紐付け、外部 Evidence の永続化を行わない。
- 非公開: 会議本文・capture token・workspace path・coding payload/result 本文を World へ返さない。Tool 実行・外部通信・LLM 抽出も行わない。

## 2. 検証コマンド結果（G1〜G6）

| ゲート | コマンド | 結果 |
| --- | --- | --- |
| G1 core | `bun run check:personal-state` | passed（fmt / clippy -D warnings / test） |
| G2 World | `cargo test --locked --manifest-path src-tauri/Cargo.toml --lib memory::personal_state::world` | 121 passed / 0 failed / 2 ignored |
| G2 M2A | `... --lib m2_` | 67 passed / 0 failed / 1 ignored |
| G3 owner | `... --lib meeting::` | 15 passed / 0 failed |
| G3 owner | `... --lib coding::` | 7 passed / 0 failed / 3 ignored |
| G4 scope・復元 | `... --lib runtime::context` | 13 passed / 0 failed |
| G4 scope・復元 | `... --lib persistence::` | 82 passed / 0 failed / 1 ignored |
| G5 size | `bun run size:check` | passed（`module-size ok (655 files)`） |
| G5 文書 | `bunx --bun spec-html check ./spec/docs --warnings-as-errors` | passed（errors=0, warnings=0） |
| G6 最終 | `bun run check:local` | passed（exit 0） |
| G6 Rust | `bun run test:rust-packages` | passed（exit 0） |
| G6 core | `cargo test --locked --manifest-path crates/personal-state-core/Cargo.toml` | 100 passed / 0 failed（lib51 + continuity17 + world_contract2 + world_m2_frame6 + world_traversal9 + world_v2_validation15） |

filter 0件を成功扱いしていない。perf 専用試験は `#[ignore]` とし通常 suite へ混ぜていない。

## 3. 性能ゲート（M2-27）

`cargo test --locked --manifest-path src-tauri/Cargo.toml --lib m2_27_ -- --ignored --nocapture`。同一端末・debug build・warm-up 5・30 sample・nearest-rank p95（昇順29番目）。fixture は Project 投影100件・ledger 2,000件・Runtime 8参照。構築は5秒以内。正常取得を assert してから測定し Err を成功値にしていない。

| 測定 | p95 | max | 条件 | 判定 |
| --- | --- | --- | --- | --- |
| Runtime のみ | 0.979 ms | 1.003 ms | p95 <= 20 ms | 合格 |
| Frame | 10.288 ms | 11.430 ms | p95 <= 150 ms、max <= 500 ms | 合格 |
| 再検証 | 10.648 ms | 10.704 ms | p95 <= 150 ms | 合格 |

値は M2A の開発用通過基準であり音声対話の最終 SLO ではない。基準は結果に合わせて緩めていない。

## 4. 受入シナリオ対応（M2-A〜M2-J）

| ID | 対応試験 | 結果 |
| --- | --- | --- |
| M2-A | `m2_22_active_meeting_yields_running_and_active_project`、`m2_07_world_snapshot_omits_token_entries_and_error` | meeting の現在状態のみ返し、本文・token なし |
| M2-B | `m2_22_pause_makes_the_old_frame_changed`、`m2_22_terminal_meeting_has_no_active_focus`、`m2_08_discarded_row_is_runtime_unavailable` | 旧 Frame は Changed、終端は active Focus なし |
| M2-C | `m2_23_revision_unchanged_state_change_is_detected` | revision 据置の cancel_requested を検出し cancelled と断定しない |
| M2-D | `m2_23_settled_does_not_claim_success` | terminal を返し Goal 達成・成功を表現しない |
| M2-E | `m2_21_same_name_in_another_project_is_denied`、`m2_06_unregistered_target_is_denied_without_leaking`、`m2_06_removed_direct_link_is_denied_without_epoch_change`、`m2_06_revoked_scope_is_denied` | 名前・状態を返さず ScopeDenied |
| M2-F | `m2_19_clock_rollback_is_expired`、`m2_19_ledger_revision_change_invalidates_the_old_frame`、`m2_19_source_forget_invalidates_the_old_frame`、`m2_19_source_content_change_makes_the_old_frame_changed`、`m2_19_scope_link_removal_makes_the_old_frame_scope_denied` | 期限前でも無効化 |
| M2-G | `m2_24_pending_projection_omits_graph_but_keeps_runtime`、`m2_24_projection_capacity_omits_graph_but_keeps_runtime`、`m2_13_*` | graph の省略理由を返し、許可 Runtime は参照可能 |
| M2-H | `m2_20_current_contract_is_transient_only_with_five_reasons`、`m2_20_valid_recall_is_still_transient_only`、`m2_20_unknown_sourceref_is_rejected_by_the_existing_parser` | transient_only、永続 Evidence 0件、hash を独立根拠にしない |
| M2-I | `m2_26_impossible_budget_is_rejected`、`m2_03_nine_distinct_refs_are_a_limit`、`m2_04_validity_boundaries`、`m2_19_clock_rollback_is_expired` | BudgetTooSmall / Limit / Expired を固定 |
| M2-J | `m2_22_restart_active_row_without_live_is_not_running`、`m2_23_outcome_unknown_is_unknown_not_terminal`、`m2_16_instance_id_is_stable_per_service_and_unique_across_services` | owner 状態を返し、前 process の Frame は使用不可 |

すべて合成 DB・固定時刻・fake owner で再現している。`runtime_frame_snapshot_tests.rs` は実 file DB を `SqliteReaders::open` で読む統合試験である。LLM・ASR・pi subprocess・外部サービスは起動していない。

## 5. ビルド・ゲートを成立させるために行った範囲外の最小修正

作業中に並行して進んだ D4 (external MCP) と旧 llang 文書に、M2A とは独立した既存のゲート不通過があった。ユーザー指示に従い、M2A の挙動を変えない最小修正で解消し、G5 / G6 をすべて通過させた。

1. `src-tauri/src/quality_eval.rs`: D4 で `AppState` に追加された `tool_selection` フィールドが harness 初期化子に無く、`quality-eval-harness` ビルドが失敗していた。`test_state.rs` と同じ `tool_selection::build_service(... direct())` を追加した。
2. `src-tauri/tests/sqlite_architecture.rs`: 全ファイルが `#![cfg(test)]` のテスト専用モジュール（M2A の `runtime_test_support.rs`）を production として走査し、`SqliteWriter::open` を誤検出していた。`tests.rs` と inline test module を除外する既存規則に合わせ、`#![cfg(test)]` のファイルも除外するよう修正した。
3. `spec/docs/saaa-llang-dynamic-capability-m2b-transport-reference.md`: 参考資料の先頭に追加された H1 と、保存した旧計画の H1 が2つあり MD001 だった。旧計画の見出しを H2 へ下げた。
4. `src-tauri/src/tool_selection/mcp/tests.rs`: D4 test の `MockServer::start(state)` を `state.clone()` に修正（test-only の所有権エラー）。
5. `scripts/module-size-baseline.json`: M2A の新規ファイルと、M2A で行数を増やした `coding/mod.rs` / `memory/personal_state/world/mod.rs` を登録した。

これらは M2A の設計・契約・返却値・認可・期限・予算に影響しない。

## 5.1 補足

- 通常 suite では M2-27 の `m2_27_frame_and_revalidation_gates` を ignored とする（重い fixture を通常試験へ混ぜないため）。認定値は §3 に記録した。
- 実 DB を経由しない fake owner による会議統合は `runtime_meeting_tests.rs` で、実 `meeting_sessions` 行と owner snapshot を組み合わせて確認した。ASR を起動する live 経路は M2A の対象外である。

## 6. 読取の副作用0・非接続

- `m2_17_read_path_writes_nothing`、`m2_25_reader_connections_reject_writes`、`m2_28_frame_reads_do_not_mutate_schema_or_world` で、Frame 取得・再検証・Evidence 能力判定が World assertion / transition / patch / generation / Source を増やさないことを確認した。
- `WorldFrameService` は `turns.rs`・`conversation_context.rs`・Context Broker・IPC・HTTP・MCP のいずれからも呼ばれていない。既存の通常会話入力は不変。
- 新規 DB 表・payload version・World Kind・公開 API・migration を追加していない（`m2_28` で表一覧と Schema version を確認）。

## 7. M2B / M3 への引渡し条件

M2A の Frame は内部診断用の一時 snapshot であり、生成中の保証ではない。通常会話への投入は未実装である。

- M2B: 外部側が安定 resource ID、immutable revision または内容 digest、権限と Scope、版を指定した再取得、deleted / unavailable / changed の区別を提供できることを前提にする。接続先の名前だけで実装済みとみなさない。新しい Source resolver を追加する場合は `store::load` / commit / Reader / 依存索引 / tombstone/journal / バックアップ復元 / generation 再検証を一つの設計として具体化し、`SourceRole` だけを変える実装を禁止する。Runtime については Frame の一時 snapshot と監査可能な履歴イベントのどちらを根拠に保存するかを先に決める。
- M3: 既存 Broker の Candidate と GenerationHandle へ接続する。Frame の TTL だけでは生成中の失効を保証できないため、dispatch 時と出力時の依存再検証、選択・省略、非命令表示、World 無効時の継続を必要条件とする。M2A の `revalidate_frame` はその部品であり、M3 の完成を意味しない。
- 外部 Evidence: `assess_contextstill_v1` は現行 memory-recall-v1 を `transient_only` と固定する。`persistent_eligible` は将来の予約値であり、M2A の production 実装からは返さない。

## 8. 完了判定

| 条件 | 状態 |
| --- | --- |
| 全カード M2-00〜M2-29 | 完了 |
| M2-A〜M2-J | 対応試験あり・合格 |
| 読取の副作用0 | 確認 |
| 認可・Source 失効・再起動・予算の試験 | あり |
| 性能ゲート | 合格（§3） |
| Scope 登録が必要な運用境界の説明 | [M2実行契約 R2](../../docs/saaa-personal-world-model-m2-execution-contract.md) と本記録 §4・§6 |
| 返却 Frame が内部診断用で通常会話未投入 | 本記録 §6・§7 |
