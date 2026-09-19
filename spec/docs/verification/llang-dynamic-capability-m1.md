# SAAA × L-Lang 動的拡張 M1 検証結果

実施日: 2026-09-19

上位文書: [詳細実装手順・契約・試験仕様](../saaa-llang-dynamic-capability-implementation-guide.md)、[M0 検証結果](./llang-dynamic-capability-m0.md)

## 判定

M1: PASS（制約は「未完了・制約」に列挙）

固定runtimeとcandidateを分離し、package v2（`predicate-i32-v1`、boolean入力のみ）の取込み・検証・有効化・呼び出し・停止・復元をSQLite catalogで管理できるようにした。M0-R=PASSを前提に、T10〜T15を実施した。

今回は実装していない（仕様どおり）: MCPサーバー起動、会話ツールへの公開、UI、LLM生成、enum/string/null/undefined入力の公開、任意effect、配布アプリ対応、inspectionの利用（M3）。

## 実装した構成

`src-tauri/src/generated_capabilities/` を新設した。

| ファイル | 責務 |
| --- | --- |
| `mod.rs` | 内部APIのexport |
| `contracts.rs` | v2 wire型、`deny_unknown_fields`、canonical hash（`package_hash` / `contract_hash` / `inventory_hash`） |
| `errors.rs` | 安定エラーコード18種と`CapabilityError` |
| `limits.rs` | 初期上限（64 KiB、1 MiB/64 KiB、15/30/15秒、内側1〜10秒、6 MiB、256ケース、60秒、2 KiB、1〜8 field） |
| `host/mod.rs`、`host/wire.rs`、`host/runtime_bundle.rs` | 信頼runtimeの検証、schema検査、起動組立て |
| `host/process.rs` | 既存PoCのprocess管理を移動（二重実装しない） |
| `package_store.rs` | staging・flat path/size検査・byte copy・inventory・rename |
| `schema.rs`、`repository.rs` | migrationと短いDB操作 |
| `verification.rs` | acceptance台帳の解決、L-Lang verifyとSAAA独立acceptanceの実行 |
| `guards.rs` | 入力検査、checkの終端記録、有効化の前提条件（現行runtime・完全なpayload） |
| `lifecycle.rs`、`service.rs`、`service/test_support.rs` | admission、invoke、有効版切替、停止、終了処理（`test_support.rs` は `#[cfg(test)]` の試験専用フック） |
| `recovery.rs` | 起動時のinterrupted化と整合性検査 |

主な設計判断:

- package manifest `version: 2` のみを新Runtimeで受付。旧PoCのv1は回帰試験として残す。host protocolは `llang-host-v1`、profileは `predicate-i32-v1`。
- 入力は1〜8個の必須boolean fieldのみ。nullable/undefinable/optionalはfalse、valuesは空。`undefinedFields` は空配列固定。それ以外のcontractは `unsupported-contract`。
- `package_hash` は固定版L-Langの `contentHash(parsed manifest)`（キー順を正規化したcanonical JSONのSHA-256）。Rust側でも同じ方式で算出し、fixtureの固定値と一致することを `t01_package_hash_matches_the_fixed_vector` で確認する。`inventory_hash`・`contract_hash`・`runtime_digest`・`acceptance_hash` は用途を分けた。
- stateは `candidate / validated / active / suspended / retired`。`active` はpartial UNIQUE indexで能力ごとに最大一つ。現在版pointerとactive行は単一transactionで更新する。
- サービスは短時間のadmission mutexとHost processのSemaphore(1)を持つ。ロック順は `admission → writer`。Host待機中はどちらのロックも保持しない。実行枠が無ければ `busy` で、無制限queueを作らない。
- invokeは「入力検査 → admission下でactive版・epoch再検査 → calls.runningをcommit → ロック解放 → 整合性再確認 → spawn → 終了記録」。開始commitに失敗したらspawnしない。終了記録に失敗したら成功を返さない。
- verifyは「acceptance hash照合 → 実行枠取得 → check.runningをcommit → Host verify → 独立acceptance → report保存 → state/inventory再確認して一transaction確定」。公開 `invoke()` はactiveを要求するため使わず、verification専用のHost実行関数を通す。二重検証は `busy`。
- 取込みは「staging → manifestと参照5ファイルだけをbyte copy → hash照合 → 信頼Hostでinspect → inventory生成 → 同一filesystem内でrename → 一transactionで能力・candidate・import完了」。同package_hash再取込みは同revisionを返し、acceptanceが異なれば `conflict`。
- 起動時の回復はDBとfileの確認だけでWasmを実行しない。runtime設定 `SAAA_LLANG_RUNTIME_CONFIG` が未設定または不正なら機能disabledで通常起動を継続する。
- 信頼runtimeはサービス構築時に一度検査するだけでなく、**spawn直前の毎回** `TrustedRuntime::revalidate` で検証し直す。検査したentrypointと実行するentrypointを同一にし、構築後に差し替えたruntimeを実行しない（H05）。
- 有効化は、保存された `runtime_digest` と現在のHost digest、保存された `inventory_hash` と管理copyの実hashを**transaction内で**照合する。別runtimeで再構築したサービスや、検証後に改変したpayloadではactiveにできない（L05）。
- invokeも受付transactionで現行runtime digestを再確認し、runtimeを変更した場合は再検証を要求する（L05）。
- invokeの実行登録はadmission待ちの `.await` より後、detached taskへ渡す直前に行い、admission下でも `ensure_accepting` を再確認する。途中で呼出元futureがabortされても実行登録・実行枠が残らない（S04）。
- 取込み時にacceptance台帳の `capabilityId` とcandidateの `metadata.id` を照合し、同じ入力契約を持つ別能力の期待値を流用できないようにする（P06）。
- importのHost実行も共通の実行枠（Semaphore(1)）と取消登録を使う。実行枠が埋まっていれば `busy` で、importだけが同時実行数を迂回しない（I07）。
- 終了処理 `shutdown` は終了状態を保持して新規受付を拒否し、所有する実行へ取消を伝え、実行枠が返るまで（上限3秒）待つ。呼び出し後の新規invoke/importは `unavailable`（S01）。
- 検証結果の確定は、revision状態の変更とcheckの終端記録を分離する。検証中にrevisionが停止・置換された場合、revisionは停止状態のまま保ち、checkは必ず終端状態（`failed`）へ移す（V05）。
- 正当な非flatなpackageは `unsupported-package-layout` として明示的に拒否する。candidate内のJS・runtime directory・起動コマンドは一切実行しない。
- `generated_capabilities` は `cfg(test)` に閉じず通常buildに含める。M2以降の入口が接続するまで呼び出し元がRust内部とtest harnessに限られるため、モジュールをcrate公開面（`pub mod`）に置き、M1の内部APIを通常buildの到達可能なAPIとして明示した。`#[allow(dead_code)]` で未使用を隠していない。
- 管理操作の入口を持たないM1でもクリーンなbuild gateを保つため、AppStateに保持した `Arc<CapabilityService>` から起動時 `recovery::reconcile_startup` と終了時 `shutdown` を呼ぶ。Tauri command、MCP、会話ツールは追加していない。

## 試験一覧（ID → test名 → 結果）

実kit（bundle済みruntime + 実candidate A/B）で実行した。mock host（script化したBun）を使うのはH03とH04のみである。

| ID | test名 | 結果 |
| --- | --- | --- |
| W01 | `generated_capabilities::tests::w01_real_v2_responses_parse_strictly` | pass |
| W02 | `w02_protocol_id_and_hash_correlation_is_enforced` | pass（error応答のpackageHash不一致も拒否） |
| W03 | `w03_string_boolean_is_not_a_success_value` | pass |
| W04 | `w04_outer_ok_does_not_hide_inner_failures` | pass |
| W05 | `w05_unknown_fields_versions_and_empty_cases_are_rejected` | pass |
| H01 | `h01_both_candidates_run_on_one_runtime_digest` | pass（実kit） |
| H02 | `h02_tampered_runtime_and_missing_bun_fail_before_spawn` | pass |
| H03 | `h03_hanging_host_is_killed_reaped_and_replaced` | pass（mock host） |
| H04 | `h04_child_environment_is_cleared` | pass（mock host） |
| P01 | `p01_reimporting_the_same_package_reuses_the_revision` | pass |
| P02 | `p02_managed_copy_ignores_later_external_changes` | pass |
| P03 | `p03_unsafe_layouts_are_refused_before_publication` | pass |
| P04 | `p04_tampering_with_the_managed_copy_is_detected` | pass |
| P05 | `p05_conflicting_existing_package_directory_is_never_overwritten` | pass |
| D01 | `d01_schema_initialization_is_idempotent` | pass |
| D02 | `d02_migration_preserves_existing_conversation_data` | pass |
| D03 | `d03_foreign_keys_reject_cross_capability_pointers_and_double_active` | pass |
| V01 | `v01_a_valid_candidate_becomes_validated_with_recorded_hashes` | pass |
| V02 | `v02_mismatched_acceptance_never_validates` | pass |
| V03 | `v03_double_verify_and_suspended_revisions_are_refused` | pass（競合注入は未実装、下記） |
| V04 | `v04_cancellation_aborts_verification_without_validating` | pass（期限そのものは未実測、下記） |
| L01 | `l01_activation_requires_a_passed_check_for_the_current_runtime` | pass |
| L02 | `l02_switching_to_a_passing_candidate_keeps_epoch_and_states` | pass |
| L03 | `l03_two_activations_at_the_same_epoch_conflict` | pass |
| L04 | `l04_transaction_failure_rolls_back_pointers_states_and_epoch` | pass |
| I01 | `i01_truth_table_matches_and_invalid_inputs_never_spawn` | pass |
| I02 | `i02_stale_resolved_capability_is_not_forwarded` | pass |
| I03 | `i03_suspension_does_not_cancel_accepted_work` | pass（barrier同期は未実装、下記） |
| I04 | `i04_cancelled_calls_are_recorded_and_release_capacity` | pass（caller切断の回収は未再現、下記） |
| I05 | `i05_capacity_is_bounded_and_never_queues` | pass |
| I06 | `i06_database_write_failures_never_report_success` | pass |
| R01 | `r01_interrupted_rows_are_recovered_without_spawning` | pass |
| R02 | `r02_missing_or_changed_payload_stops_the_capability` | pass |
| R03 | `r03_packages_without_a_catalog_row_are_reported_not_published` | pass |
| R04 | `r04_restart_keeps_the_catalog_without_running_candidates` | pass |
| （T01） | `t01_package_hash_matches_the_fixed_vector` | pass |
| （M0-R） | `m0r_runtime_is_reusable_from_a_moved_directory_with_spaces` | pass |
| （フラグoff） | `feature_flag_off_refuses_management_and_execution` | pass |

レビュー指摘の修正に対する回帰試験（`tests/regression.rs` と `tests/runtime_change.rs`）:

| ID | test名 | 指摘 | 結果 |
| --- | --- | --- | --- |
| H05 | `h05_runtime_modified_after_construction_is_never_executed` | 構築後に差し替えたruntimeを実行できた | pass |
| L05 | `l05_promotion_requires_the_current_runtime_and_an_intact_payload` | 旧runtimeの合格記録と改変payloadで有効化・実行できた | pass |
| I07 | `i07_import_shares_the_single_execution_slot` | importが同時実行数の制限を迂回した | pass |
| S01 | `s01_shutdown_rejects_new_work_and_cancels_owned_executions` | shutdownが何も停止しなかった | pass |
| S02 | `s02_an_aborted_call_is_never_left_running` | 呼出元futureのabortでcallがrunningのまま残った | pass |
| S04 | `s04_an_abandoned_caller_does_not_leak_an_execution_registration` | admission待ちの呼出元をabortすると実行登録が残り、shutdownが完了しなかった | pass |
| P06 | `p06_acceptance_for_another_capability_is_refused` | 別能力のacceptanceを同じ契約のcandidateへ結び付けられた | pass |
| V05 | `v05_a_revision_stopped_during_verification_leaves_no_running_check` | 検証中の停止でcheckがrunningのまま残った | pass |

H05とV05は、修正を一時的に戻すと失敗し、修正を入れると成功することを実測で確認した（H05は改変CLIが `status: ok` / `value: false` を返すこと、V05は `finalization runs` で失敗することをそれぞれ確認）。S04（invokeの登録をadmission後へ移す修正を戻すと失敗）とP06（acceptanceのcapabilityId照合を外すと失敗）も同じ方法で再現を確認した。

`generated_capabilities` の試験は49件、すべてpass（うち `errors` / `limits` の単体試験2件）。

## 実行コマンドと結果

```sh
cargo test --manifest-path src-tauri/Cargo.toml wasm_host_poc --no-fail-fast
# 8 passed、0 failed（既存PoCの回帰）

cargo test --manifest-path src-tauri/Cargo.toml generated_capabilities -- --list
# 49 tests（fixtureとruntimeを使う試験47件と errors / limits の単体試験2件）

cargo test --manifest-path src-tauri/Cargo.toml --lib generated_capabilities --no-fail-fast
# 49 passed、0 failed

cargo test --manifest-path src-tauri/Cargo.toml --no-fail-fast
# lib 603 passed / 0 failed / 13 ignored、統合試験 3 + 1 + 1 passed

cargo check --manifest-path src-tauri/Cargo.toml --lib
# error / warning なし

cargo fmt --check --manifest-path src-tauri/Cargo.toml
# 成功

cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
# 成功（新モジュールのみが対象。他モジュールの既存警告は0件）

bun run size:check
# module-size ok (577 files)
```

`bun run check:local` の結果: **失敗（フロントエンド試験のみ、作業前からの既存失敗）**。

- `format:check`、`lint`、`size:check`、`build:frontend`、`quality:check`、`ipc:check`、`typecheck`、`cargo fmt --check`、`cargo clippy -D warnings` は通過した。
- 最後の `test:frontend` が 313 pass / 14 fail / 12 errors で失敗した。失敗は `tests/*.tsx` などのフロントエンド試験で、`SyntaxError: Export named 'act' not found in module 'react/index.js'` を含む。
- 今回の変更はRustモジュール、fixture、ドキュメント、および `src-tauri/Cargo.toml` のみで、`src/` と `tests/` のフロントエンドコードを含まない。
- clean HEAD（`1b595ffd10c3c77adb19c8d3f9f47c536aa3fe54`）のworktreeで `bun test tests` を実行した結果も同じ 313 pass / 14 fail / 12 errors だった。作業前baselineを採っていないため、この比較を既存性の根拠とする。
- `test:frontend` の失敗で `bun run test` が停止するため、`check:local` 内ではRust全試験は走らない。Rust側は上記のとおり別途実行して確認した。

## 実kit検証とmock検証の区別

- 実kit: W01〜W05（実host応答、実candidate）、H01、H02、H05、P01〜P05、D01〜D03、V01〜V05、L01〜L05、I01〜I07、S01、R01〜R04、M0-R。信頼runtimeを起動してcandidate A/Bを実行している。H05は実runtimeをコピーし、構築後にentrypointだけを差し替える。L05は実runtimeのコピーを整合的に変更して別digestのbundleを作る。
- mock host: H03（ハングするBunプロセス、出力超過）、H04（環境変数消去）。信頼runtimeの代わりにscript化したBunを `execute_with_command`（テスト専用）で起動する。runtime本体の検証はH02とH05が担う。
- Bunが無い環境では実kit試験を実行できない。mockだけの合格をM1完了としない。

## 未完了・制約

| 項目 | 内容 |
| --- | --- |
| I03 競合注入 | 受付前の停止拒否と、非現行版の停止が現行版の実行を止めないことは確認したが、受付直後の停止をbarrierで同期する競合再現は未実装 |
| I04 caller切断 | 取消をspawn前に伝える経路でcancelled記録と枠解放を確認した。呼出元futureのabortでcallがrunningのまま残らないこと（S02）と、admission待ちでのabandonが実行登録・実行枠を残さないこと（S04）を確認した。子プロセスの回収は `kill_on_drop` に依存し、呼出元drop直後のreapは直接観測していない |
| V03 verify中suspend | barrier注入は未実装。状態がsuspendedのrevisionを検証しないことと、合格がstateをlift しないことを確認した |
| V04 全体期限 | 60秒のacceptance期限はコード上で検査する（`verification::run` が残時間とdeadlineを確認）が、期限超過そのものは実測していない。取消経路で「合格扱いしない」ことを確認した |
| H03 出力超過 | mock hostでのstdout上限確認。実kitで上限を超える応答は生成していない |
| inspection | M0-IはPASSだが未コミットsnapshot依存。SAAAはM1でinspectionを利用しない（M3） |
| 管理操作の入口 | M1はRust内部APIとtest harnessのみ。Tauri command、MCP、会話ツール、UIを追加していない。productionのacceptance台帳ディレクトリは `<data>/generated-capabilities/acceptance` を指し、取込み入口は無い |
| サンドボックス | 子プロセス以外の任意process treeを生成できるruntimeへ広げていない。OSレベルのメモリ隔離や、同一OSユーザーによる管理領域への能動的な書換えに対する完全隔離はM1の保証外 |
| runtime再検証の窓 | 実行のたびにspawn直前でbundleを再検証するが、検証とexecの間の短い窓は残る。bundleを不変なsnapshotとしてexecする仕組みはM1の範囲外 |
| baseline | `bun run check:local` の作業前baselineは未取得。新モジュール外の失敗が出た場合は既存問題として区別する |

## 次に実装可能な段階

- M2-A（会話ツール）とM2-B（MCP接続口）。`CapabilityService` の `resolve_active` / `invoke` を共通入口として使う。
- M3-A（更新・復帰・検査）。`contract_hash` 完全一致を互換条件とし、inspection出力を版ごとの保存領域へ置く。
- M4-A/M4-B（生成の決定的接続、実モデル実証）。M1のimport/verify/activate経路をそのまま使う。

M1で追加した依存: `jsonschema` をdev-dependencyから通常dependencyへ移動（versionと設定は維持）。それ以外の新規依存は無い。
