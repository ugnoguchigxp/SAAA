# Personal World Model M2A 進捗記録

作成日: 2026-09-20。本書はカード単位の実装・試験記録である。全体集計は [m2-results.md](m2-results.md)、計画は [M2計画](../../docs/saaa-personal-world-model-m2-plan.md)、固定契約は [M2実行契約](../../docs/saaa-personal-world-model-m2-execution-contract.md) を参照する。

着手時 HEAD は `f5a1069`。作業中に並行する D4 (external MCP) の WIP コミットが同じ作業ツリーへ入り、HEAD は `ae07e46` / `f273798` / `ec5fb85` へ進んだ。M2A の対象ファイル（core/world, meeting, coding, memory/personal_state/world, persistence スキーマ以外）は D4 と重複しない。M2A は新規 DB 表・migration・公開 API を追加しない。

## M2-00 記録（baseline）

| コマンド | f5a1069 時点の結果 |
| --- | --- |
| `cargo test --locked --manifest-path crates/personal-state-core/Cargo.toml` | 81 passed（lib38 + continuity17 + world_contract2 + world_traversal9 + world_v2_validation15） |
| `... --lib memory::personal_state::world` | 51 passed / 0 failed / 1 ignored |
| `... --lib meeting::` | 15 passed |
| `... --lib coding::` | 7 passed / 3 ignored |
| `... --lib runtime::context` | 13 passed |
| `... --lib persistence::` | 82 passed / 1 ignored |

計画作成時の詳細は [m2-planning-baseline.md](m2-planning-baseline.md)。全 `check:local` と全 Rust package 試験は f5a1069 では未実施だった。既存の性能記録は warm-up なし・5 sample・1,000件構築途中であり、M2-01 で方式を補った。

## M2-01 既存性能試験の方式修正

変更: `src-tauri/src/memory/personal_state/world/v2_tests.rs` の `d41_v2_performance`。

- `requested` と `created` を分離して表示し、`interrupted || created == requested` を assert。
- `load` / `rebuild` / `query` を `.expect(...)` し、Err を時間だけで成功扱いしない。
- warm-up 5、30 sample、nearest-rank p95（昇順29番目）へ統一。
- `m2_01_nearest_rank_p95_method`（非 ignored）で 30 sample の p95 が 29 であることを固定。

結果: `m2_01_nearest_rank_p95_method` passed。既存 `d41_v2_performance` は ignored のまま（重い fixture を通常試験へ混ぜない）。M2-27 の認定測定は別カードで実施。

## M2-02 core wire 型

変更: `crates/personal-state-core/src/world/runtime_frame.rs`（新規）、`world/mod.rs`（登録）。

- `RuntimeKind` / `RuntimeRef` / `RuntimePhase` / `MeetingLivePhase` / `MeetingOwnerState` / `CodingOwnerState` / `RuntimeOwnerState` / `RuntimeStateView` / `RuntimeFocus` / `FrameNotice` / `WorldFrame`。
- `RuntimeOwnerState` は `{"kind":"meeting_session","state":"paused"}`。
- すべて `deny_unknown_fields`。`WorldSliceV2`・v2 payload・EntityKindV2 は不変。

試験: `m2_02_frame_roundtrips_and_rejects_unknown_fields`、`m2_02_owner_state_wire_shape_is_explicit`、`world_m2_frame::m2_02_unknown_runtime_kind_is_rejected`。すべて passed。

## M2-03 正規化・上限

変更: `runtime_frame.rs` の `normalize_runtime_refs` / `normalize_ttl` / `effective_max_bytes` / `validate_frame_identifier`。

試験: `m2_03_nine_distinct_refs_are_a_limit`（重複排除後9件は `Limit`、8件は可）、`m2_03_ttl_zero_is_invalid_and_over_limit_is_capped`（0はInvalidInput、1001は1000、lower bound は上げない）、`m2_03_max_bytes_lower_bound_is_not_raised`（max_bytes=1は1のまま）。passed。

## M2-04 stamp canonical 化・digest・期限

変更: `content_digest`（captured_at / expires_at / graph.as_of_ms を除く）、`is_within_validity`（`captured <= now < expires`）、`FrameStamp` / `compare_stamp`。

試験: `m2_04_content_digest_ignores_observation_time_only`、`m2_04_validity_boundaries`、`world_m2_frame::m2_04_stamp_change_classification`、`m2_18_owner_digest_change_is_detected_even_with_same_revision`。passed。

## M2-05 / M2-06 scope 認可

変更: `src-tauri/src/memory/personal_state/world/runtime_scope.rs`（新規）`authorize_frame_request`。`scope::load` のみ使用し `resolve` / 登録 SQL を呼ばない。Project は run の focus/parent、対象は focus/current/parent、active + epoch 一致、Project→対象の直接 link を要求。

試験（`runtime_scope_tests.rs`）:
- `m2_05_authorized_meeting_request_succeeds`
- `m2_05_misclassified_access_is_denied`（Internal は拒否）
- `m2_05_wrong_principal_is_denied`
- `m2_05_stale_policy_is_denied`
- `m2_05_finished_run_is_denied`
- `m2_06_unregistered_target_is_denied_without_leaking`
- `m2_06_removed_direct_link_is_denied_without_epoch_change`
- `m2_06_revoked_scope_is_denied`

すべて passed。

## M2-07 会議 owner snapshot

変更: `src-tauri/src/meeting/types.rs` の `WorldMeetingSnapshot`、`meeting/mod.rs` の `MeetingRuntime::world_snapshot` と test-only `set_test_world_state`。

試験: `m2_07_world_snapshot_omits_token_entries_and_error`（token/本文/error を World へ出さない、owner state 不変）、`m2_07_default_meeting_reader_returns_idle`。passed。

## M2-08 会議 DB 参照

変更: `src-tauri/src/memory/personal_state/world/runtime_meeting.rs`（新規）。読む列は id/status/started_at/ended_at/saved_at のみ。不正時刻は `frame-owner-corrupt`。

試験: `m2_08_missing_row_is_runtime_unavailable`、`m2_08_discarded_row_is_runtime_unavailable`、`m2_08_malformed_time_is_owner_corrupt`。passed。

## M2-09 会議マッピング

変更: `runtime_frame.rs` の `map_meeting`（R3 表）。digest は `['meeting_session',id,status,started_at,ended_at,saved_at,live_id,live_state]`。終端行は別会議/None の live を null 正規化。

試験: `m2_09_meeting_mapping_covers_r3_table`（active+Active→running、active+Paused→unstable、paused+Paused→paused、paused+Stopping→stopping、terminal、discarded/missing→unavailable、無関係会議で digest 不変）。passed。

## M2-10 coding owner snapshot

変更: `src-tauri/src/coding/world_snapshot.rs`（新規）`read_world_snapshot`、`coding/mod.rs` 登録。run 件数を33で打ち切り、32超は `runtime_capacity_omitted`。`repository::authorize` を再利用。result の `complete` は boolean のときだけ返す。

試験: `m2_10_snapshot_reads_minimal_state`、`m2_10_non_boolean_complete_is_null`、`m2_10_thirty_two_runs_are_accepted_and_thirty_three_omitted`。passed。

## M2-11 coding の Source 検証

変更: `runtime_coding.rs`（新規）。initial/current/過去runの source について会話・role・available・tombstone・Project scope ref を検査。失敗は `runtime_unavailable`、本文は返さない。

試験: `m2_11_tombstoned_source_is_unavailable`、`m2_11_unavailable_source_is_unavailable`、`m2_11_unmapped_source_is_unavailable`。passed。

## M2-12 coding phase / Focus

変更: `runtime_frame.rs` の `map_coding` と `focus_for`。running+accepted のみ running、cancel_requested は stopping、settled は terminal、outcome_unknown は unknown。Goal完了 edge は0。

試験: `m2_12_coding_mapping_and_focus`、`m2_23_revision_unchanged_state_change_is_detected`、`m2_23_settled_does_not_claim_success`、`m2_23_outcome_unknown_is_unknown_not_terminal`、`m2_23_unknown_state_is_omitted_not_remapped`。passed。

## M2-13 容量 preflight

変更: `runtime_capacity.rs`（新規）`check_frame_capacity`。Project 投影100、全 ledger 2000。`store::load` 前に実行し、超過時は後続表を読まない。

試験: `m2_13_projection_100_is_allowed_and_101_is_omitted`、`m2_13_ledger_2000_is_allowed_and_2001_is_omitted`（超過時 graph None・既存 assertion 0件削除）。passed。

## M2-14 予算組立

変更: `runtime_frame.rs` の `assemble_frame`。Runtime 単位は view + Focus を丸ごと採否、Runtime 枠 2048 byte、graph 枠 6144 byte、Frame 8192 byte。収まらない単位は省略し `runtime_budget_omitted`。

試験: `m2_14_runtime_units_are_bounded_by_bytes_and_count`、`m2_14_one_byte_budget_fails`、`m2_14_focus_never_dangles`、`world_m2_frame::m2_14_assembly_drops_graph_before_stripping_runtime_evidence`、`m2_14_graph_whole_omission_sets_truncated`。passed。

## M2-15 graph 取得

変更: `runtime_graph.rs`（新規）。認可済みの外側値だけで `ActivateInputV2` を一度呼ぶ。観測配列と temporary attention は空。空 seed は Query を呼ばず空 envelope を返す。stale/pending は graph None + notice。

試験: `m2_24_graph_and_runtime_are_returned_together`、`m2_24_pending_projection_omits_graph_but_keeps_runtime`、`m2_24_projection_capacity_omits_graph_but_keeps_runtime`、`m2_15_empty_seed_does_not_fabricate_a_node`。passed。

## M2-16 サービス型

変更: `runtime_frame.rs`（adapter, 新規）`WorldFrameService` / `PreparedWorldFrame` / `OwnedFrameRequest` / `MeetingReader` / `RuntimeMeetingReader`。`instance_id` は service 構築時に一度生成。内部型は Serialize しない。DB へ保存しない。

試験: `m2_16_instance_id_is_stable_per_service_and_unique_across_services`（別 service の frame は Expired）。passed。

## M2-17 prepare 順序

変更: `WorldFrameService::build`。事前 Scope → live-before → DB snapshot → live-after → 整形。会議変化はその単位のみ省略（`runtime_unstable`）。自動 retry 0回。

試験: `m2_17_live_change_between_reads_omits_only_that_unit`（world_snapshot 呼出し2回）、`m2_17_read_path_writes_nothing`（total_changes 不変）。passed。

## M2-18 stamp 差分分類

変更: `compare_stamp`。owner digest 差は Changed、instance 差は Expired、期限境界を正確に判定。

試験: `world_m2_frame::m2_18_owner_digest_change_is_detected_even_with_same_revision`、`m2_04_stamp_change_classification`。passed。

## M2-19 再検証

変更: `WorldFrameService::revalidate_frame`。instance 一致 → 期間 → 同要求の再読取 → stamp 比較。旧 Frame を書き換えない。

試験: `m2_19_clock_rollback_is_expired`（999→expired, 1999→current, 2000→expired）、`m2_19_scope_link_removal_makes_the_old_frame_scope_denied`、`m2_19_source_content_change_makes_the_old_frame_changed`、`m2_19_ledger_revision_change_invalidates_the_old_frame`、`m2_19_source_forget_invalidates_the_old_frame`。passed。

## M2-20 外部 Evidence 適格性

変更: `evidence_eligibility.rs`（新規）`assess_contextstill_v1`。現行 memory-recall-v1 は常に `transient_only` と5理由。未知契約は `frame-unsupported-evidence-contract`。

試験: `m2_20_current_contract_is_transient_only_with_five_reasons`、`m2_20_unknown_contract_is_rejected`、`m2_20_valid_recall_is_still_transient_only`、`m2_20_unknown_sourceref_is_rejected_by_the_existing_parser`。passed。

## M2-21 Scope 負例

試験（`runtime_scope_tests.rs`）: `m2_21_one_disallowed_reference_denies_the_whole_request`、`m2_21_same_name_in_another_project_is_denied`、`m2_21_low_classification_never_leaks_runtime_content`、`m2_21_invalid_reference_id_is_invalid_input`。passed。

## M2-22 会議統合

試験（`runtime_meeting_tests.rs`）: `m2_22_active_meeting_yields_running_and_active_project`、`m2_22_pause_makes_the_old_frame_changed`、`m2_22_unstable_live_state_omits_the_unit`、`m2_22_terminal_meeting_has_no_active_focus`、`m2_22_restart_active_row_without_live_is_not_running`。ASR 起動0。passed。

## M2-23 coding 統合

試験（`runtime_coding_tests.rs`）: `m2_23_revision_unchanged_state_change_is_detected`、`m2_23_settled_does_not_claim_success`、`m2_23_outcome_unknown_is_unknown_not_terminal`、`m2_23_unknown_state_is_omitted_not_remapped`。pi process 起動0。passed。

## M2-24 World + Runtime 統合

試験: `m2_24_graph_and_runtime_are_returned_together`、`m2_24_pending_projection_omits_graph_but_keeps_runtime`、`m2_24_projection_capacity_omits_graph_but_keeps_runtime`。passed。

## M2-25 snapshot 整合

変更: `runtime_test_support.rs` の `Fixture::file` / `readers_open`。一時 file DB を `SqliteReaders::open`（製品 read transaction）で読む。

試験（`runtime_frame_snapshot_tests.rs`）: `m2_25_persistent_readers_report_current_then_changed`、`m2_25_policy_and_epoch_changes_are_rejected`、`m2_25_reader_connections_reject_writes`。passed。

## M2-26 予算境界

試験: `m2_26_budget_is_never_exceeded_and_notices_are_bounded`（max_bytes 以下、notice16以内、Node30以内）、`m2_26_impossible_budget_is_rejected`（max_bytes=1は `frame-budget-too-small`）、`m2_26_long_japanese_content_preserves_evidence`。passed。

## M2-27 性能

変更: `runtime_frame_perf_tests.rs`（新規、ignored）。五要素100件・ledger2000件・Runtime8参照、warm-up5・30sample・nearest-rank p95。

測定結果（debug build、同一端末）:

| 測定 | p95 | max | 合格条件 |
| --- | --- | --- | --- |
| Runtime のみ | 0.979 ms | 1.003 ms | <= 20 ms |
| Frame | 10.288 ms | 11.430 ms | p95 <= 150 ms、max <= 500 ms |
| 再検証 | 10.648 ms | 10.704 ms | <= 150 ms |

構築は 5 秒以内。`ledger=2000`。passed。

## M2-28 境界

試験（`runtime_frame_boundary_tests.rs`）: `m2_28_frame_reads_do_not_mutate_schema_or_world`（表・assertion・transition・patch・generation・source 件数不変）、`m2_28_no_context_generation_or_broker_row_is_created`。passed。既存 continuity / scope 回帰は G1〜G4 で確認。

## M2-29 記録

[m2-results.md](m2-results.md) を作成。

## 実装上の補足

- `runtime_test_support.rs` はテスト専用 fixture。合成 DB・固定時刻・fake meeting owner のみを使い、実会議・Provider・pi subprocess・外部サービスを起動しない。
- M2A の入口は `WorldFrameService` のみで、`turns.rs` / `conversation_context.rs` / Context Broker / IPC からは呼ばれない。通常会話の入力は不変。
- 並行 D4 作業と旧 llang 文書に起因するゲート不通過は、M2A の挙動を変えない最小修正で解消し、G5 / G6 を通過させた。詳細は [m2-results.md](m2-results.md) §5。

## コードレビューでの改善（実装漏れ是正を含む）

実装後に契約 R0〜R10 と全カードを再点検し、以下を修正・強化した。

| # | 改善 | 内容 | 追加・更新試験 |
| --- | --- | --- | --- |
| 1 | 会議終端 digest の正規化 | 終端行で live が別会議/None のとき `live_session_id` を null に正規化（従来は対象 id を誤って入れていた） | `m2_09_terminal_meeting_normalizes_unrelated_live_identity` |
| 2 | `FrameNotice.reference` の wire 固定 | `skip_serializing_if` を外し、`null` を含む固定形にした | `m2_02_frame_roundtrips...` |
| 3 | 予算削減順 | notice 超過時は「graph 全体 → Runtime 単位（kind/id 降順）」の順に落とす（R7） | `m2_14_*`、`m2_26_*` |
| 4 | seed 正規化 | 既存規則で dedup し、正規化後 5 件は `frame-limit`（従来は 4 件へ無言切り捨て） | `m2_15_five_distinct_seeds_are_a_limit`、`m2_15_duplicate_seeds_are_deduplicated_not_truncated` |
| 5 | graph のエラー対応 | `world-limit`→`frame-limit`、graph 予算不足は Frame 失敗ではなく graph 省略 | `m2_26_small_budget_with_graph_omits_graph_not_frame` |
| 6 | coding source 検査 | job 自身の initial source を必ず含め、role は message 側で判定（R4） | `m2_11_unmapped_initial_source_is_unavailable` |
| 7 | run_id 検証 | `validate_identifier` で run_id を検証（R1） | `m2_05_*`（不正 id は ScopeDenied） |
| 8 | request fingerprint | `PreparedWorldFrame` に request 全体の fingerprint を保持し、再検証時に照合（R6） | `m2_16_*`、`m2_19_*` |
| 9 | 再検証の軽量事前検査 | ledger/epoch/policy/scope の変化を `store::load` 前に検出（R6） | `m2_19_ledger_revision_change...`、`m2_25_scope_epoch_change_is_rejected` |
| 10 | 容量検査の省略 | graph 要求がないときは容量 preflight を呼ばない | `m2_27`（Runtime のみ） |
| 11 | 死んだ型フィールドの削除 | `AuthorizedFrame`/`AuthorizedTarget` の未使用フィールドと未使用 helper を削除 | — |
| 12 | 漏洩・副作用の追加確認 | Frame JSON に workspace/payload/本文が含まれないこと、Evidence 判定が永続化しないことを追加確認 | `m2_10_frame_json_has_no_coding_content_or_paths`、`m2_28_evidence_assessment_persists_nothing` |

再点検の結果、計画 §4 の追加対象・除外対象、契約 R0〜R10 の各項目に対する実装漏れは残っていない。`cargo clippy -D warnings`、`cargo fmt --check`、`size:check`、`spec-html check`、`check:local`、`test:rust-packages` はすべて通過する。
