# Personal World Model M3A 進捗

作成日: 2026-09-20。並行差分は巻き戻さない。0件実行を合格にしない。

## M3-00 baseline

| 項目 | 値 |
| --- | --- |
| HEAD | `c0eda5ea6c67e7471ef4a011fc673fa41a45d2e1` |
| M2報告 | [m2-results.md](m2-results.md)。Broker 未接続 |
| `cargo check --locked --manifest-path src-tauri/Cargo.toml --lib` | passed |
| 計画§3の旧コンパイル停止 | 今回は再現せず。当時の失敗と混同しない |
| dirty | Role Routing 文書、Tool Selection D5、執事循環計画。巻き戻していない |

## カード記録

| ID | 試験 | 件数 | 結果 |
| --- | --- | --- | --- |
| M3-00 | 文書 | 試験未追加 | baseline 記録 |
| M3-01 | `m3_01_request_types_are_not_serde_and_fields_stay_private` | 1 | passed |
| M3-02 | `m3_02_*` 3件 | 3 | passed。ExactName拒否、5 seed/9 ref Limit、重複seed正規化、空要求 empty_request、カンマ Project は ambiguous、未知 Project は scope_denied |
| M3-03 | `m3_03_wrapper_preserves_frame_fields` | 1 | passed |
| M3-04 | `m3_04_utf8_limits_omit_the_whole_candidate` | 1 | passed |
| M3-05 | `m3_05_one_frame_becomes_one_untrusted_candidate` | 1 | passed。May/Untrusted/Base/utility0 |
| M3-06 | `m3_06_prepare_and_revalidate_use_one_service` | 1 | passed。別serviceは Expired |
| M3-07 | `m3_07_ttl_*` `m3_07_notice_*` | 2 | passed。1999 Current / 2000 Expired / 999 Expired。実file DB。notice のみは empty_frame |
| M3-08 | `m3_08_summary_has_no_payload_and_input_stays_put` | 1 | passed |
| M3-09 | `m3_09_baseline_matches_broker_and_skips_prepare_on_error` | 1 | passed。baseline_error 時 prepare 0 |
| M3-10 | `m3_10_partial_scope_is_denied_without_second_compose_success` | 1 | passed |
| M3-11 | `m3_11_same_utility_does_not_displace_existing` | 1 | passed |
| M3-12 | `m3_12_expired_clock_and_other_run_do_not_retry` | 1 | passed |
| M3-13 | `m3_13_shadow_kind_is_rejected_before_record` | 1 | passed。planned 維持、input件数不変 |
| M3-14 | `m3_14_real_frame_paths_select_without_fake_candidates` | 1 | passed。graph は now=entity valid_from |
| M3-15 | `m3_15_source_and_link_changes_drop_stale_candidates` | 1 | passed。旧 Frame の revalidate が Current でない。revision 据置の coding 状態変更も検出 |
| M3-16 | `m3_16_json_escapes_quotes_and_fake_delimiters` | 1 | passed |
| M3-17 | `m3_17_yellow_health_survives_and_instruction_stays_one` | 1 | passed。40 byte 上限は Budget、world 不採用 |
| M3-18 | `m3_18_shadow_does_not_write_or_dispatch` | 1 | passed。generation 増分0。turns / conversation_controller / chat_completions / agent_session 非配線 |
| M3-19 | `m3_19_shadow_path_stays_within_dev_gates` ignored | 1 | passed。extra p95 0.447 ms、full p95 29.358 ms、max 34.968 ms、prepared=35 selected=35 ledger=2000 |
| M3-20 | 下表ゲート | — | `check:local` passed。clippy / size / `m3_` / personal-state / world suite / spec-html / `test:rust-packages` |
| M3-21 | [m3-results.md](m3-results.md) | — | 記録 |

`cargo test --locked --manifest-path src-tauri/Cargo.toml --lib m3_`: 21 passed / 0 failed / 1 ignored。

`cargo test --locked --manifest-path src-tauri/Cargo.toml --lib runtime::context`: 34 passed / 0 failed / 1 ignored。

配置: `src-tauri/src/runtime/context/world/`（source / render / shadow）。通常 turns へ未配線。`WorldFrameService` は変更せず組み合わせ。

size: 新規9ファイルを baseline 登録。既存 `generation_inputs.rs` / `mod.rs` は 10% ratchet 内。閾値は緩めていない。
