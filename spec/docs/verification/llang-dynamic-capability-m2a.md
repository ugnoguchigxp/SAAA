# SAAA × L-Lang M2A 検証結果 — 会話ツール接続

実施日: 2026-09-20

上位文書: [M2A 実装計画](../saaa-llang-dynamic-capability-m2a-plan.md)、[M1 検証結果](./llang-dynamic-capability-m1.md)

## 判定

M2A: PASS（制約は「未完了・制約」に列挙）

検証済みactive能力のうち、管理者が明示的に公開設定へ載せたものだけを、Chat Completions経路の会話でLLMのツールとして提示し、既存 `CapabilityService` で実行してboolean結果を同じ会話へ戻せるようにした。MCP、UI、LLM生成、management UI、TypeScript inspection、world clock、ABI拡張は今回未実装である。

## 実際にできること / 未対応

できること:

- `SAAA_GENERATED_TOOLS_CONFIG` のallowlistにあるactive revisionを `gc_<revision UUID 32桁>` として会話ツールへ提示する。
- 提示したoffer snapshot内のrevisionだけを実行し、offer後の更新・停止・payload改変・runtime不一致を拒否する。
- boolean入力の検証、16 KiB上限、内部call ID発行、`origin=conversation` の記録、安全な結果JSONを共通adapterで行う。
- 会話取消・timeout・shutdownをhost `Cancellation` へ伝え、実行枠とDB終端の所有権を維持する。

未対応:

- MCP listener（M2B）。`tools::execute` は `origin` と取消を引数に取るため、MCPは同じ関数を `origin="mcp"` で呼ぶだけでよい。
- Chat Completions以外のprovider。`available_agent_tools` を呼ぶのは `providers/chat_completions` のみで、Dynamic LAN経路には定義を追加していない。見かけ上の対応を足していない。
- 管理UI、LLM生成、inspection（M3/M4）。
- 生成ツールの進捗音声。`voice_progress::supports` は `gc_` を対象にしない。

## G0: 開始状態とM1入口検査

- 開始時HEAD: `b3c46065b23573700fa249e11eb17b2c805b1f65`（M1 + caller abort取消修正）。M2A実装は未コミット差分として開始した。
- 開始時の `git status --short`、HEAD、追跡diff、新規ファイルのSHA-256はrepository外 `/tmp/saaa-m2a-review-20260920-003534/` に保存した（`START_STATE.txt`、`tracked.diff`、`MANIFEST.sha256`）。秘密情報・target・node_modulesは複製していない。
- M1入口検査として `cargo test --lib generated_capabilities` / `wasm_host_poc` を実行し、開始時点で全件passを確認した。

| ID | 再現条件 | 対応するM1試験 |
| --- | --- | --- |
| G01 | 構築後に信頼runtime実ファイルを変更 | `h05_runtime_modified_after_construction_is_never_executed` |
| G02 | runtime digestの異なる正規設定へ切替 | `l05_promotion_requires_the_current_runtime_and_an_intact_payload` |
| G03 | verify後に管理下packageを変更してactivate | `p04_tampering_with_the_managed_copy_is_detected` |
| G04 | 実行枠を占有して別import/inspect | `i07_import_shares_the_single_execution_slot` |
| G05 | invoke開始後にshutdown | `s01_shutdown_rejects_new_work_and_cancels_owned_executions` |
| G06 | verify完了前にsuspend | `v05_a_revision_stopped_during_verification_leaves_no_running_check` |
| G07 | 実行futureの所有者をabort | `s02_abort_cancels_started_process_and_releases_capacity` |

## 実装した構成

| ファイル | 責務 |
| --- | --- |
| `generated_capabilities/publication.rs` | `SAAA_GENERATED_TOOLS_CONFIG` のparseと上限、`GeneratedToolSnapshot`（descriptor配列と `tool_name → ResolvedCapability`）、名前・schema・説明生成、32 KiB検査 |
| `generated_capabilities/tools.rs` | 共通実行adapter（snapshot lookup、入力検証、内部UUID、`service.invoke`、安全な結果JSON）、OpenAI定義adapter、`AgentToolOffer`、公開snapshotの一括解決 |
| `generated_capabilities/service.rs` | `resolve_publication`（allowlistを一つのcatalog読取で解決）、`resolve_active` のrepository委譲 |
| `generated_capabilities/repository.rs` | `resolve_active`（active revisionの解決。unknownはConflict、inactiveはNone） |
| `providers/stream/dispatch.rs` | `available_agent_tools` が `AgentToolOffer`（定義＋snapshot）を返す。`gc_` をadapterへ振り分け、recallへ流さない |
| `providers/chat_completions/mod.rs` | リクエストごとにofferを保持し、batch全体のoffered検証を維持 |
| `providers/chat_completions/voice_progress.rs` | snapshotと取消をadapterへ伝播 |
| `app_state.rs`、`lib.rs`、`quality_eval.rs`、`test_state.rs` | 公開設定をAppStateへ保持、起動時に一度だけ読込、診断をstderrへ |

設計判断:

- 公開設定は起動時に一度だけ読む。変更には再起動が必要。未設定・invalidは生成ツールだけを無効にし、SAAAの起動と既存会話機能を妨げない。
- 設定JSONは `formatVersion/enabled/capabilityIds` のみを受け付け、未知キー、version不一致、重複ID、空ID、9件以上、64 KiB超を明示的に無効化する。先頭8件への黙った切詰めはしない。
- ツール名はcandidate metadataではなくrevision UUIDから作る。不正UUIDを短縮hashで救済しない。
- 入力schemaは検証済みcontractから作り、object・booleanのみ・全required・`additionalProperties=false` にする。candidateの自由文説明はpromptへ入れず、説明はホスト管理の定型文＋field名にする。
- offer snapshotは1回のproviderリクエストとそのtool callバッチの間だけ保持する。実行時はsnapshotのResolvedCapabilityをそのまま使い、tool名から現在revisionを再解決しない。`service.invoke` がepoch・state・hash・runtime digestを再検査する。
- 内部call IDは毎回ホストでUUIDを発行し、providerの `tool_call_id` は会話対応付けにだけ使う。`origin` は `conversation`。
- 定義配列はcompact JSONで32 KiB以下を必須とし、超過時は当該リクエストの生成ツールを全件公開しない（既存ツールは保持）。
- 取消は `RunCancellation` をhost `Cancellation` へ橋渡しし、呼出元futureのdrop時は `CancelOnDrop` でhostへ取消を伝える。実行中callのDB終端と実行枠はdetached task（M1）が所有する。
- 検証範囲はChat Completions経路のみ。他providerへ見かけの定義を足していない。

## 試験一覧

### A1 publication（`generated_capabilities/tests/publication.rs`）

| ID | test名 | 結果 |
| --- | --- | --- |
| P01 | `p01_no_config_disabled_or_empty_list_publishes_nothing` | pass |
| P02 | `p02_malformed_configs_disable_publication_without_failing_startup` | pass |
| P03 | `p03_and_p06_only_the_allowlisted_active_revision_is_resolved` | pass |
| P04 | `p04_names_come_from_the_revision_and_schema_is_the_strict_boolean_subset` | pass |
| P05 | `p05_definition_limit_rejects_the_whole_offer_without_truncation`（provider非依存配列の境界。adapter wrapper分の再検査はコードにあるが専用の境界テストは未追加） | pass |
| P06 | `p03_and_p06_only_the_allowlisted_active_revision_is_resolved`（同一読取内でunknown/inactiveをskip） | pass |

### A2 adapter（`generated_capabilities/tests/adapter.rs`、実kit）

| ID | test名 | 結果 |
| --- | --- | --- |
| E01 | `e01_true_and_false_are_both_successful` | pass |
| E02 | `e02_invalid_inputs_never_start_a_host_process` | pass |
| E03 | `e03_an_unoffered_gc_name_is_refused` | pass |
| E04 | `e04_provider_call_ids_never_become_the_durable_call_id` | pass |
| E05 | `e05_an_offer_after_activation_is_rejected_not_forwarded` | pass |
| E06 | `e06_suspension_and_tampering_are_refused_without_leaking_internals` | pass |
| X01 | `x01_a_pre_cancelled_run_never_starts_a_host_process` | pass |

### A4 取消（`generated_capabilities/tests/adapter_abort.rs`、実kit・実ハングhost）

| ID | test名 | 結果 |
| --- | --- | --- |
| X01 | `x01_a_pre_cancelled_run_never_starts_a_host_process`（`adapter.rs`） | pass |
| X01b | `x01b_in_flight_run_cancellation_reports_cancelled_and_settles` | pass |
| X02 | `x02_aborting_the_adapter_future_cancels_and_settles` | pass |
| X03 | `i05_capacity_is_bounded_and_never_queues`（M1） | pass |
| X04 | `s01_shutdown_...` / `i06_database_write_failures_never_report_success`（M1） | pass |

X01b/X02は、実際に起動してブロックするhost CLI（`tests/hanging.rs`）を使い、起動後取消とadapter future abortが、callを `cancelled` で終端させ、実行登録と実行枠を解放し、子プロセスをreapすることを確認する。M1の `s02_abort_...` は `service.invoke` 直接のabortを見るため、adapter境界の取消はここで補う。

### A3-A5 Chat Completions（`providers/chat_completions/generated_tools_tests.rs`、実HTTP server + 実kit）

| ID | test名 | 結果 |
| --- | --- | --- |
| C01 | `c01_generated_definition_is_offered_and_its_result_returns_to_the_conversation` | pass |
| C02 | `c02_a_mixed_batch_with_an_unoffered_name_is_refused_before_any_call` | pass |
| C03 | `c03_generated_tools_need_persistence_and_respect_the_call_budget`（output_persistenceなしと12 call上限。`tools=false`/JsonProbe分岐は未試験） | pass |

C01は実際のHTTP request bodyに生成定義が入ること、次のrequestの `tool_call_id` とboolean結果JSONが対応することを検証する。実行は実bundle runtime + 実candidate Aで行う。C02はoffered+unknownの混合バッチが1件目のcallも実行せず `Protocol` で拒否されることを検証する。C03はoutput_persistenceなし・12 call到達時に生成ツールが提示されないことを検証する。

### 既存経路の回帰

- C04相当: `cargo test --lib providers` 76 passed / 2 ignored。既存recall/coding/UI/voiceの試験を含む。disabled時は生成ツールが出ないことをP01/C03で確認。
- C05相当: provider失敗後の自動再試行制御は既存の `chunk`/`mark_started` 経路を変更していない。`cargo test --lib providers` の既存retry試験がpass。
- X03相当: `i05_capacity_is_bounded_and_never_queues`。adapterは `Busy` を安全なコードへ写像。
- X04相当: `s01_shutdown_...`、`i06_database_write_failures_never_report_success`。timeout時も `host_cancel.cancel()` 後にdetached taskの終端を待つ。

## 自己レビュー（R1）と修正

修正前snapshotは `/tmp/saaa-m2a-review-20260920-003534/`（`MANIFEST.sha256` にSHA-256）、修正後snapshotは `/tmp/saaa-m2a-review-after-20260920-003852/`。初回実装と以下を区別する。workspaceにはM2Aと無関係のpersonal world model変更が併存するため、snapshotはM2A対象ファイルのみを複製した。

| # | 重要度 | 内容 | 修正 |
| --- | --- | --- | --- |
| F1 | 中 | 32 KiB上限をprovider非依存の定義配列で検査しており、OpenAIの `{"type":"function","function":...}` wrapper分だけ超過し得た | `tools::append_generated` で実際のprovider定義配列のバイト数も検査し、超過時はsnapshotごと非公開にした |
| F2 | 低 | 64 KiB超の設定ファイルを `fs::read` で全読みしてから拒否していた | `fs::metadata().len()` で読込前に拒否し、read後にも再確認する |
| F3 | 低 | P03（allowlist外activeの非公開）が構造的で専用assertが無かった | `p03_and_p06_...` にallowlist外activeの非公開assertを追加 |
| F4 | 低 | 新規module追加で `dispatch.rs` 等がmodule-size ratchetを超過 | 責務を `publication.rs`/`tools.rs`/`test_state.rs` へ分割し、baselineを上げずに登録（`bun run size:register`） |
| F5 | 低 | 未使用の `tools::unavailable_content` 参照とclippyの `cloned_ref_to_slice_refs` | 参照箇所を整理し `std::slice::from_ref` を使用 |

| F6 | 中 | X02（adapter future abort）の専用試験が無く、RunCancellation→host取消の起動後経路も未確認だった | `tests/hanging.rs` に実ハングhost helperを共通化し、`tests/adapter_abort.rs` にX01b/X02を追加。`tests/invoke_abort.rs` も同helperを使うよう整理 |

コード読直しで確認した経路: 設定 → 起動時読込 → `available_agent_tools` → offer作成 → HTTP body → stream parser → dispatch → `tools::execute` → `service.invoke` → host → DB終端 → tool応答。反例として、offer後activate（E05）、offer後suspend（E06）、payload改変（E06）、起動前取消（X01）、不正入力（E02）、未offer名（E03/C02）を実行した。

## 実行コマンドと結果

```sh
cargo test --locked --manifest-path src-tauri/Cargo.toml --lib generated_capabilities
# 63 passed / 0 failed

cargo test --locked --manifest-path src-tauri/Cargo.toml --lib providers
# 76 passed / 0 failed / 2 ignored

cargo test --locked --manifest-path src-tauri/Cargo.toml --lib wasm_host_poc
# 8 passed / 0 failed

cargo test --locked --manifest-path src-tauri/Cargo.toml --lib
# 620 passed / 0 failed / 13 ignored

cargo test --locked --manifest-path src-tauri/Cargo.toml
# lib 620 passed、統合試験 3 + 1 + 1 passed

cargo test --locked --manifest-path src-tauri/Cargo.toml --features quality-eval-harness quality_eval::tests
# 2 passed / 0 failed

cargo fmt --check --manifest-path src-tauri/Cargo.toml
# 成功

cargo clippy --locked --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
# 成功

bun run size:check
# module-size ok (586 files)
```

`bun run check` 全体はフロントエンド試験の作業前からの失敗（`SyntaxError: Export named 'act' not found`）で停止する。M1検証時と同じ既存失敗であり、今回の変更はRust側のみである。`bun run test` のRust部分は上記で個別に確認した。

## 公開設定の例

`SAAA_GENERATED_TOOLS_CONFIG` に絶対パスを設定する。

```json
{
  "formatVersion": 1,
  "enabled": true,
  "capabilityIds": ["<generated_capabilities.id のcatalog capability ID>"]
}
```

management/import/verify/activateの入口はM1のRust内部APIとtest harnessのみ。例（test harness）:

```sh
cargo test --locked --manifest-path src-tauri/Cargo.toml --lib generated_capabilities
# TestEnv::start(true) -> import_a() -> verify(ACCEPTANCE_A) -> activate_revision() が
# fixture A を active にする。C01はそのserviceを共有するAppStateで実HTTP経路を実行する。
```

## 未完了・制約

| 項目 | 内容 |
| --- | --- |
| X02 adapter future abort | adapter futureのabortを専用に再現し、`generated_capabilities/tests/adapter_abort.rs::x02_...` で回収まで確認した。子プロセスのreapはBunのsignal-0で観測する |
| P06 競合注入 | 複数IDを単一 `lifecycle::read` で解決するためoffer内の混合snapshotは起きない。更新との実競合はbarrier注入ではなく、offer後の更新をE05（stale-revision）で拒否することを確認した |
| C05 自動再試行 | provider失敗後の再試行制御は既存試験で維持。生成ツール固有の再試行試験は追加していない |
| P05 adapter wrapper境界 | 32 KiBはprovider非依存配列で境界試験する。実際のOpenAI wrapper分は `tools::append_generated` が再検査するが、wrapperで超過する境界の専用テストは追加していない（コード読直しで確認） |
| C03 tools=false/JsonProbe | 生成ツールの非公開は `chat_completions/mod.rs` の `mode != JsonProbe && options.tools` で担保する。C03はoutput_persistenceなしと12 call上限のみを実行し、tools=false/JsonProbe分岐は専用試験していない |
| MCP | 未実装（M2B）。共通adapterのAPIは用意済み |
| provider範囲 | Chat Completionsのみ。Dynamic LANは未対応で、対応済みに見える定義を追加していない |
| 実kit/mock | C01-C03、E01-E06、X01は実bundle runtime + 実candidate。mock hostはM1のH03/H04のみ |
| 入力上限 | 16 KiBはarguments文字列のbyte長。JSON object検証はhost起動前 |
| サンドボックス | M1と同じ。子プロセス以外の任意process treeへは広げていない |

## M2Bへの引継ぎ

- `GeneratedToolSnapshot` はprovider非依存の `descriptors()`（`tool_name`/`description`/`input_schema`/`resolved`）と `resolve(tool_name)` を持つ。MCPの `tools/list` はdescriptorをそのまま使える。
- `tools::execute(service, snapshot, call, origin, timeout, run_cancellation)` は会話固有の分岐を持たない。MCPは `origin="mcp"` で呼ぶ。
- `tools::openai_tool_definitions` はChat Completions専用adapter。MCPは使わない。
- MCP listener、認証token、`127.0.0.1` bind、tools/list・tools/call対応表は未実装。M2Bで別moduleとして追加し、同じ `Arc<CapabilityService>` と公開設定を使う。
