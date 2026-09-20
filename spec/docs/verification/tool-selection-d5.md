# ツール選択 D4残件解消・D5 MCP公開 実装・検証報告

作成日: 2026-09-20 / 対象: [D5実装計画](../saaa-tool-selection-d5-mcp-server-plan.md) / 上位: [M2B全体計画](../saaa-llang-dynamic-capability-m2b-plan.md) / 既存: [D4検証報告](./tool-selection-d4.md)

## 1. 完了状況の要約

| カード | 状態 | 根拠 |
| --- | --- | --- |
| P00 開始snapshot・基準コマンド | 完了 | `spec/evidence/tool-selection-d5/start-*`、§4 |
| P01 管理taskによるinvoke所有 | 完了 | `invocation.rs`、`tests::p01_caller_abort_does_not_leave_the_invocation_running`、`mcp_server::tests::h07_http_disconnect_does_not_cancel_the_managed_call` |
| P02 endpoint binding付き訂正ルール | 完了 | schema v25、`p02_remote_rule_is_bound_and_survives_only_the_learned_endpoint`、`p02_backfill_marks_pre_version_remote_rules_unconfirmed` |
| P03 D4未達の追加試験 | 一部完了 | 同名別source曖昧・送信後取消・endpoint変更・結果容量はD4で確認済み。L-Lang+2MCP同居のD5経由振分け、100call満足度集計、403/redirect実測は未追加 |
| P04 実E5/BGE評価・1500/10000件測定 | 未完了 | 本実行環境にモデル配備なし。mock輸送試験を実ML評価とは呼ばない |
| S01 設定・token・bind検証 | 完了 | `mcp_server/config.rs` 表駆動試験 |
| S02 型付きID・envelope・error mapping | 完了 | `mcp_server/protocol.rs` fixture |
| S03 session・TTL・scope・conversation | 完了 | `mcp_server/sessions.rs`、`context.rs` |
| S04 scenario-only API・request-local検索 | 完了 | `service::extract_scenario_only` / `search_with_scenario`、H11試験 |
| S05 管理task・ID予約・取消・監督 | 完了 | `mcp_server/calls.rs`、H06/H07 |
| S06 POST/GET/DELETE・認証・3入口dispatch | 完了 | `mcp_server/router.rs`、実HTTP試験 |
| S07 lib起動/終了配線・self接続拒否 | 完了 | `start_from_environment` / `shutdown_slot`、`review_self_endpoint_source_is_refused` |
| S08 H01〜H14・D4回帰 | 一部完了 | 9件のH試験を実装（§7）。H04/H08/H12の一部は未実装 |
| S09 独立MCPクライアントsmoke・性能測定 | 未完了 | 手書きHTTP試験のみ。独立クライアント・p50/p95測定は未実施 |
| R1 初回snapshot固定・差分レビュー | 完了 | §8 |
| R2 修正・再検証・報告 | 完了 | §8、§4 |

D5のサーバー本体・P01/P02は完了。H04/H08/H12の一部、P03の一部、P04、S09は未達として残す。D4/D5全体の完了は宣言しない。

## 2. snapshot と作業環境の事故

| 時点 | 内容 |
| --- | --- |
| 開始 | HEAD `848c380`、DB schema version 24、`tool_selection` 130 / `generated_capabilities` 67 / `providers` 79(2 ignored)、fmt/clippy/size green |
| 事故 | 作業中、同一workspaceの別タスクが `git stash -u` で未コミット変更（D5実装・未追跡fileを含む）を退避し、mainをM2Aコミットへ前進させた。D5 WIPは `stash@{0}`（message `other-agents-wip-d5-m3`）へ移動した |
| 復旧 | 他者のstashをpopせず、stashのbaseが当時のHEADと一致することを確認した上で、D5関連pathのみ `git checkout stash@{0} --` / `stash@{0}^3 --` で復元した。他タスクのM3・role-routing文書は取り込まない |
| 最終 | HEAD `be23064`。`0b4bbe5`（整形）→ `f9c7f07`（D5追加試験）→ `be23064`（writer/終了処理のlib外集約）を追加commit。`bun run check` green |

復旧後は作業をcommit単位で保全し、再度のstashで失われないようにした。

## 3. 実装

### 3.1 P01: 呼出元abortに耐える実行所有

- `RunCancellation` を `Arc` 内部状態に変更し `Clone` 可能にした（`app_state.rs`）。これにより管理taskが caller と独立して取消を観測できる。
- 新規 `tool_selection/invocation.rs`。`service::invoke` は事前検査・invocation行作成後に `ManagedInvocation` を `tokio::spawn` し、oneshotで応答を待つ。
- 管理taskが backend 実行・結果検証・大結果保存・`finish_invocation` を所有する。backend panicは `catch_unwind` で `interrupted` に写像し、DB終端を飛ばさない。
- 呼出元futureがdropしても管理taskは終端まで動く。明示取消（共有 `RunCancellation`）だけがremote停止の起点。

### 3.2 P02: 訂正ルールのendpoint binding

- schema v25で `tool_selection_rule_source_bindings(rule_id, tool_id, endpoint_hash)` を追加。`length(endpoint_hash) <= 64`。
- `backfill_rule_source_bindings` は version < 25 のupgrade時のみ実行し、既存remote対象ruleへ空hash（未確認）bindingを作る。現在endpointを推測で割り当てない。
- `feedback::insert_soft_rule` は対象toolが `mcp_http` のときだけ、その時点の `tool_selection_mcp_sources.endpoint_hash` をbindingとして保存する。L-Lang ruleはbindingを持たない。
- `active_rules` は binding を持ち、かつ空hashまたは現在endpoint不一致のruleを不適用にする。別source・L-Langのruleは影響を受けない。
- `McpManager::register_configured_sources` はendpoint変更を検出すると該当sourceのbindingを空へ戻し、rule_epochを進める。元URLへ戻しても未確認のまま自動復活しない。

### 3.3 D5サーバー（`mcp_server/`）

| file | 責務 |
| --- | --- |
| `config.rs` | `SAAA_TOOL_GATEWAY_MCP_CONFIG` のparse、絶対path・port・projectId検証、tokenFileのowner-only権限・改行1つ・base64url 32byte検証 |
| `protocol.rs` | 型付きrequest ID（int/string別）、batch/null/小数拒否、envelope、error response |
| `sessions.rs` | `Initializing/Ready/Closing`、session上限16、idle TTL 30分、初期化10秒、call 4/session・16/全体、ID履歴4096 |
| `context.rs` | session用conversation行の冪等作成、project存在確認、`RequestContext` 構築 |
| `calls.rs` | 3入口のMCP定義（gateway schema再利用）、`isError` 判定、`CallPermit` によるslot解放 |
| `router.rs` | axum POST/GET/DELETE、Bearer定数時間比較、Host/Origin/Content-Type/Accept検査、body 64KiB・10秒、JSON-RPC dispatch |
| `mod.rs` | `start` / `start_from_environment` / `ServerHandle::shutdown` / `shutdown_slot`、self endpoint記録 |

- wire契約: `2025-06-18`、`initialize`（sessionヘッダーなし）、`notifications/initialized`、`ping`、`tools/list`（3定義固定・cursorなし）、`tools/call`、`notifications/cancelled`、GET 405、DELETE 204。JSONのみ（SSEは返さない）。
- `tools_search` は `service::extract_scenario_only` でscenarioだけ抽出し、訂正候補を保存しない。`search_with_scenario` でrequest-localに検索する。
- self参照: D4 sourceのURLが自listener（`localhost`/`127.0.0.1`/`[::1]`の同一port/path）を指す場合、`sync_source` と `check_locked` が `source-self-reference` で拒否する。

### 3.4 writer集約

`sqlite_architecture` gate（productionで `SqliteWriter::open(` は `lib.rs` のみ）に合わせ、`tool_selection::open_service` / `open_mock_service` は `crate::open_database_writer` 経由にした。lib.rsのhard budget 800行を超えないよう、MCP listener起動・終了は `mcp_server` 側の関数へ移した。

## 4. 検証コマンドと結果（最終HEAD）

```sh
cargo test --manifest-path src-tauri/Cargo.toml --lib tool_selection        # 160 passed
cargo test --manifest-path src-tauri/Cargo.toml --lib generated_capabilities # 67 passed
cargo test --manifest-path src-tauri/Cargo.toml --lib providers             # 79 passed, 2 ignored
cargo fmt --check --manifest-path src-tauri/Cargo.toml                      # clean
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings # clean
bun run size:check                                                          # module-size ok (665 files)
bun run check                                                               # green（cargo test / sqlite_architecture 含む）
```

開始時実測（今回転記）: `tool_selection` 130 / `generated_capabilities` 67 / `providers` 79(2 ignored)、fmt/clippy/size green。D4報告の過去件数は転記していない。

追加された主なD5受入試験（すべて実loopback HTTPまたは実SQLite、backendは決定的fixture）:

- `tool_selection::tests::p01_caller_abort_does_not_leave_the_invocation_running`
- `tool_selection::mcp::tests::p02_remote_rule_is_bound_and_survives_only_the_learned_endpoint`
- `tool_selection::mcp::tests::p02_backfill_marks_pre_version_remote_rules_unconfirmed`
- `tool_selection::mcp::tests::h10_large_mcp_result_pages_over_the_published_wire`
- `tool_selection::mcp::tests::review_self_endpoint_source_is_refused`
- `tool_selection::mcp_server::tests::h01` 〜 `h14`（下記）

## 5. migration

- `DATABASE_SCHEMA_VERSION` 24→25。
- `tool_selection_rule_source_bindings` を `mcp::schema::migrate`（`CREATE TABLE IF NOT EXISTS`）で追加。pre-transaction相当のbackfillは `initialize_database` 内の本transactionで `previous_version < 25` のときだけ実行。
- 既存remote ruleのbindingは空hashの未確認として作られ、`active_rules` から除外される。L-Lang ruleは変更なし。
- D4のv23→v24 sources再構築経路は維持。`foreign_key_check` はD4試験で確認済み。
- World Model等の他migration番号は上書きしていない。

## 6. 設定・公開境界

- 未設定または `enabled=false` はlistenerなし。不正設定・token読込失敗・bind失敗は公開サーバーだけを無効化し、会話経路とD4接続は継続する（`start_from_environment` は診断コードをstderrに残して `None`）。
- 全methodでBearer必須。定数時間比較。Originあり403、Hostは実listenerのみ、CORSなし。401ではsession/台帳へ触れない。
- tokenは設定structへ保存せず、ログ・DB・応答へ出さない。
- principalはローカルprofile、projectは設定値のみ。clientInfo/body/tool引数では上書きできない。

## 7. 受入試験の対応

| ID | 状態 | 備考 |
| --- | --- | --- |
| H01 | 完了 | 1500 tools登録後 `tools/list` は厳密に3件、32KiB以内、ledger混入なし |
| H02 | 完了 | 無認証/不一致token/Origin/Host不一致を全methodで拒否、decision 0 |
| H03 | 完了 | session欠落400/未知404/初期化前-32002/version不一致400/batch/不正ID/415/406/413 |
| H04 | 未完了 | 同名L-Lang+2MCPのD5経由振分け試験は未追加。source修飾解決はD4 A05で確認済み |
| H05 | 完了 | 別sessionへのcandidateRef転用は `not-authorized`、同sessionは成功 |
| H06 | 完了 | 整数1/文字列"1"は別request、完了済み同ID再利用は-32600、下位再実行0 |
| H07 | 完了 | 実HTTP切断後もbackend実行が継続しDBが `succeeded` で終端 |
| H08 | 一部 | 送信前/送信後取消はD4で確認済み。DELETE/deadline/panic/shutdownの各起点をD5で網羅する試験は未実施 |
| H09 | 完了 | session 16件で17件目を-32000、backend call 0 |
| H10 | 完了 | 120KiB MCP結果をwire経由で全page復元一致、各応答40KiB以内 |
| H11 | 完了 | 訂正らしきintentでrule/feedback増加0、並列intentのdecision scenarioが混入しない |
| H12 | 一部 | endpoint変更でbinding空・`active_rules` から除外・L-Lang不変を確認。project/operation別の実検索順位差は未追加 |
| H13 | 一部 | エラーにtoken/path/SQLを露出しない。100call混在集計は未実施 |
| H14 | 完了 | 再起動後の旧sessionは404、参照も無効 |

ネットワーク試験はloopbackの実HTTPのみを使用し、外部有料サービスを前提にしていない。

## 8. 自己レビュー（R1）と修正（R2）

初回snapshotに対し「認証→session→context→scenario→search→revision参照→認可→実行→終端」と「HTTP切断→管理task継続→DB終端」をファイル横断で追跡した。

1. **呼出元abortでのrunning残留**（指摘/再現: 旧 `invoke` は `backend.invoke` を直接await）→ 管理task所有へ変更。caller abort後もDBが `succeeded` で終端することをbarrier試験で確認。
2. **外部intentの訂正記憶**（再現: MCP検索文字列を `begin_turn` へ渡すとruleが作られる）→ `extract_scenario_only` + `search_with_scenario` を追加し、MCP経路は訂正を保存しない。rule/feedback増加0を確認。
3. **URL変更後の訂正誤適用**（再現: source_id+toolNameは不変）→ endpoint bindingを追加し、変更時はbindingを空へ。元URL復帰でも未確認を維持。
4. **H10 wire肥大**（指摘: 大結果のpage応答が40KiBを超えうる）→ 実HTTPで各page応答を計測し40KiB以内を確認。
5. **self参照による循環呼出し**（指摘: D4 sourceが自公開endpointを指せる）→ `set_self_endpoint` と `normalize_loopback` で拒否。`localhost` 表記も拒否。
6. **lib.rs hard budget超過**（指摘: D5起動配線で808行）→ listener起動/終了とwriter factoryをlib外へ集約し800行以下へ。
7. **sqlite_architecture違反**（指摘: `tool_selection/mod.rs` の `SqliteWriter::open(`）→ `lib.rs` の `open_database_writer` 経由に統一。gate自体は変更していない。
8. **fmt差分**→ `cargo fmt` 適用。clippy `-D warnings` clean。

## 9. 未達・制限（完了扱いにしない）

- **P03**: L-Lang+2MCP同居のD5経由振分け、100call満足度集計、403/redirectの実測、profile 32MiB `result-storage-limit` の実通信は未追加。
- **P04 / 実ML**: E5/BGE配備がないため実モデル評価・1500/10000件 p50/p95・memory・prompt bytes測定は未実施。mock輸送試験を実ML評価とは呼ばない。
- **H04**: 同名toolのD5経由の実行先振り分け試験は未追加（D4 A05とsource修飾解決で一部確認）。
- **H08**: DELETE/deadline/task panic/app shutdownの6起点をD5で網羅していない。送信前・送信後取消はD4で確認済み。
- **H12**: project/operation別の検索順位差、別operation不適用のD5経由試験は未追加。binding機構と除外は確認済み。
- **H13**: 100call混在の満足度集計は未実施。
- **S09**: 独立MCPクライアント（例: 公式SDK）でのsmoke、性能測定、version固定の再現コマンドは未実施。
- **redirect**: D4時点でredirect追跡なし。403はD4で確認、D5の公開境界ではOrigin/Host/authを確認。
- **GET SSE / server initiated request / OAuth / stdio / 管理UI**: 計画どおり対象外。

## 10. 変更ファイル（主要）

新規: `src-tauri/src/tool_selection/invocation.rs`、`src-tauri/src/tool_selection/mcp_server/{mod,config,protocol,sessions,context,calls,router,tests}.rs`、`spec/evidence/tool-selection-d5/start-*`。

変更: `tool_selection/{mod,service,gateway,repository,feedback}.rs`、`tool_selection/mcp/{schema,manager,tests}.rs`、`tool_selection/tests/mod.rs`、`persistence/schema.rs`（version 25）、`app_state.rs`、`lib.rs`、`quality_eval.rs`、`test_state.rs`。

## 11. 再現コマンド

```sh
cargo test --manifest-path src-tauri/Cargo.toml --lib tool_selection
cargo test --manifest-path src-tauri/Cargo.toml --lib tool_selection::mcp_server
cargo test --manifest-path src-tauri/Cargo.toml --lib tool_selection::mcp::tests
cargo test --manifest-path src-tauri/Cargo.toml --test sqlite_architecture
cargo fmt --check --manifest-path src-tauri/Cargo.toml
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
bun run size:check
bun run check
```

D5サーバーの起動再現（実アプリ）:

```sh
SAAA_TOOL_GATEWAY_MCP_CONFIG=/absolute/path/mcp-server.json <saaa>
# mcp-server.json: {"formatVersion":1,"enabled":true,"port":43127,"tokenFile":"/abs/token","projectId":null}
# token file: base64url of 32+ random bytes, mode 0600
```
