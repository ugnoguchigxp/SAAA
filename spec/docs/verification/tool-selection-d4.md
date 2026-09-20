# ツール選択 D4 外部MCP接続・台帳同期 実装・検証報告

作成日: 2026-09-20 / 対象: [D4実装計画](../saaa-tool-selection-d4-mcp-implementation-plan.md)
上位: [M2B全体計画](../saaa-llang-dynamic-capability-m2b-plan.md) / 既存: [D0〜D3検証報告](./tool-selection-d1-d3.md)

## 1. 完了状況の要約

| カード | 状態 | 根拠 |
| --- | --- | --- |
| T00 開始snapshot | 完了 | HEAD `f5a1069`、tool_selection 81 / generated_capabilities 67 / providers 79(2 ignored)、fmt/clippy/size green |
| T01 設定parse・URL・secret参照・grant宣言 | 完了 | 表駆動試験（重複ID・未知field・不正URL・missing token・loopback HTTP） |
| T02 schema migration（v23→v24） | 完了 | 旧CHECKの実DB fixtureからupgrade、既存sources/catalog保存、`foreign_key_check`空、再open |
| T03 安定ID・descriptor正規化 | 完了 | 同名別source別ID、key順不変、A→B→A、schema/endpoint変更、annotations管理metadata |
| T04 JSON-RPC/HTTP/SSE接続 | 完了 | 実HTTPテストサーバーでJSON/SSE両応答、init順、ID照合、session有無/404再初期化、GET 405 |
| T05 全page取得・検証・atomic publish | 完了 | 途中失敗で旧snapshot/epoch不変、no-opでepoch不変、変更batch+1、消失/再出現、cursor循環/重複名 |
| T06 manager・poll・通知・grant差分・停止 | 完了 | import単独でgrantなし、設定由来grantの保留→後続sync適用、source削除でmanaged grantのみ撤回、手動grant保持 |
| T07 BackendRouter・McpBackend・invoke | 完了 | 実HTTP backendを既存gateway経由で実行、endpoint変更/未知kindを送信前拒否、unknown/abort/permit |
| T08 大結果の継続取得 | 完了 | 100KiB日本語結果の全page復元一致、16KiB envelope、scope/TTL/別run拒否、1MiB超のsize-limit |
| T09 eligible検索・embedding差分・freshness | 完了 | stale source候補0・HTTP call 0、embedding欠損でdegraded（BM25縮退） |
| T10 feedback・provider_extraction・候補response | 一部完了 | 同名別sourceの名前only訂正はambiguous（rule増加0）。実会話provider経由の自然訂正は未実施（下記制限） |
| T11 tests・実モデル評価・実HTTP負荷 | 一部完了 | 実HTTP fixture 41件を含むtool_selection 123件。実E5/BGE評価・1500/10000件負荷測定は未実施 |
| R1 初回snapshot固定・全差分レビュー | 完了 | §7 |
| R2 修正・最終回帰・報告 | 完了 | §5, §7 |

D5（SAAA自身のMCP公開）とD6（順位学習）は未実施。L-Lang process管理・実行取消・World Model処理は変更していない。

## 2. snapshot

| 時点 | 内容 |
| --- | --- |
| 開始 | HEAD `f5a1069`、DB schema version 23、作業ツリーに M2 計画の未追跡docsのみ |
| 初回実装 | 外部MCP一式を `tool_selection/mcp/` に新設。作業中に同一workspaceの別タスク（World Model M2）が本作業を `git stash` で退避したため、git worktree `/Users/y.noguchi/Code/SAAA-d4`（branch `d4-work`）で隔離して継続し、最終的にmainへfast-forwardで統合 |
| 最終 | branch `d4-work`。`src-tauri/src/tool_selection/{mcp,backends,resolve,gateway_schemas}.rs`、`persistence/schema.rs`、`lib.rs`、`scripts/module-size-baseline.json` |

## 3. 実装ファイル

新規 `src-tauri/src/tool_selection/`:

| ファイル | 責務 |
| --- | --- |
| `mcp/mod.rs` | protocol version・上限の固定、module公開 |
| `mcp/config.rs` | `mcpSourcesPath` の別JSON parse、URL/ID/token参照/grant宣言の検証、`endpoint_hash` |
| `mcp/schema.rs` | D4 table（mcp_sources / mcp_managed_grants / mcp_results）、v24 sources CHECK再構築 |
| `mcp/descriptors.rs` | canonical JSON、`mcpt_`/`mcpr_` 安定ID、descriptor正規化、usage page分割 |
| `mcp/transport.rs` | JSON-RPC 2.0 / HTTP / SSE、SSE decoder、session header、GET stream、unsupported server request拒否 |
| `mcp/session.rs` | `Disabled→Initializing→Ready→Reconnecting/Unavailable→Closing`、single-flight initialize、profile/source同時実行上限、shutdown |
| `mcp/sync.rs` | 全page取得・上限検証・1 transaction publish、epoch/消失/再出現 |
| `mcp/manager.rs` | 設定reload、sync single-flight、grant差分、embedding欠損backfill、通知poll、status、shutdown |
| `mcp/repository.rs` | MCP tableのSQL、freshness、managed grant差分、結果保存量 |
| `mcp/results.rs` | 大結果の保存・8KiB page分割・ACL/TTL再検査 |
| `mcp/service_support.rs` | source表示、大結果finalize、継続取得、admission error写像 |
| `mcp/wiring.rs` | BackendRouter/McpBackend/McpManagerの組立 |
| `mcp/tests.rs` | 実HTTPテストサーバーを含むD4受入試験 |
| `backends/router.rs` | DB由来binding kindでllang/mcp_httpを振り分け |
| `backends/mcp.rs` | McpBackend（binding検証、`isError`、RPC/unknown写像） |
| `resolve.rs` | 同名別sourceを曖昧として扱うtool_id解決 |
| `gateway_schemas.rs` | 3入口のOpenAI定義（変更なし・移設） |

既存変更: `tool_selection/contracts.rs`（`mcpSourcesPath`）、`repository.rs`（freshness条件・`tool_ids_by_name`・`source_kind`）、`service.rs`（manager/preflight/大結果/結果page）、`gateway.rs`（describe `oneOf`・結果分岐・sourceId/sourceLabel）、`schema.rs`（D4 schemaへ委譲）、`persistence/schema.rs`（version 24・pre-transaction再構築呼出）、`lib.rs`（manager起動/停止）。

## 4. migration結果

- `DATABASE_SCHEMA_VERSION` を 23→24。`CREATE TABLE IF NOT EXISTS` の変更だけにせず、旧 `tool_selection_sources` のCHECKに `mcp_http` が無い場合のみ、foreign_keys OFFのpre-transactionで親tableを再構築し、既存行と参照先catalog行を保存する。
- `migrate_sources_kind` 後に `PRAGMA foreign_key_check` が空であることを実DB fixture（T02）で確認。同じDBを再openして version 24 を確認。
- D4追加tableは `tool_selection_mcp_sources` / `tool_selection_mcp_managed_grants` / `tool_selection_mcp_results`。token/session id/HTTP header/生エラー本文/URLは保存しない（`endpoint_hash` のみ）。

## 5. 検証コマンドと結果（worktree `d4-work`）

```sh
cargo test --manifest-path src-tauri/Cargo.toml --lib tool_selection      # 123 passed
cargo test --manifest-path src-tauri/Cargo.toml --lib generated_capabilities # 67 passed
cargo test --manifest-path src-tauri/Cargo.toml --lib providers            # 79 passed, 2 ignored
cargo fmt --check --manifest-path src-tauri/Cargo.toml                     # clean
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings # clean
bun run size:check                                                        # module-size ok (639 files)
```

新規受入試験（`tool_selection::mcp::tests`、実HTTPテストサーバー使用）:

- T01 config表駆動、T02 v23→v24、T03 ID/descriptor、T04 JSON+SSE/404再初期化/405、T05 失敗時不変・epoch・消失再出現・cursor循環・重複名・空page、T06 grant保留/撤回、T07 実HTTP invoke/isError/disconnect unknown/cancel前call0/endpoint変更拒否/reconcile unknown、T08 100KiB page復元/scope/TTL/1MiB size-limit、T09 stale候補0・degraded、T10 同名別source ambiguous、A01 2source 1500tools、A06 progress混在、A15 401/unsupported server request。

A01 は 2 source 1500 tools を各100件pageで同期し、search最大8件・LLM定義3件・SystemContextに全説明なしを確認した。

### 実ML・負荷測定（未実施）

E5/BGE の配備（`~/.cache/saaa-tool-selection/venv` とmanifest）がこの実行環境に無いため、A13の実モデルheld-out評価、1500/10000件のp50/p95・memory・prompt bytes測定は**未実施**である。`tool_selection::mcp::tests` の輸送・同期試験は mock lane であり、実ML評価ではない。D0〜D3の live lane 記録（`tool-selection-d1-d3.md`）は維持されるが、D4で追加した外部descriptorに対する再評価は未完了。

## 6. 受入試験の対応

| ID | 状態 | 備考 |
| --- | --- | --- |
| A01 | 完了 | 2source 1500tools、3定義、検索≤8 |
| A02 | 完了 | page3相当の不正JSON・重複名・cursor循環で旧snapshot/epoch不変 |
| A03 | 完了 | 同内容no-op、末尾変更+1、A→B→A、消失→再出現（tool_id不変・履歴保持） |
| A04 | 完了 | 未認可/別project/別principal/stale参照で候補0・HTTP call 0 |
| A05 | 一部 | 同名別sourceの曖昧訂正でrule 0 は確認。L-Langと2MCPのsource付き実行の明示試験は未追加 |
| A06 | 完了 | JSON/SSE両方、progress通知を挟んでも対象IDの結果のみ |
| A07 | 完了 | 副作用後に切断でunknown、retryなし、chat call数1 |
| A08 | 一部 | 送信前取消call0。送信後取消/abortの終端・permit回収は実装済みだが専用fixtureは未追加 |
| A09 | 一部 | sync中generation変更検知、disable直後dispatch拒否、shutdown。barrierによる競合再現は未整備 |
| A10 | 完了 | 100KiB日本語のpage復元一致、別run/TTL/容量上限 |
| A11 | 未完了 | 実会話provider経由の自然訂正は未実施（D0〜D3のfixture抽出経路は維持） |
| A12 | 一部 | MCP isError/timeoutで満足度はunknownのまま。100call混在の集計は未実施 |
| A13 | 未完了 | 実E5/BGE未配備 |
| A14 | 一部 | v23→v24と再open、L-Lang既存試験は green。起動後L-Lang/訂正/World Modelの統合回帰はworktreeで実施 |
| A15 | 一部 | 401・missing env・SSE過大・unsupported server requestを確認。redirect追跡なしの実測と403は未追加 |

## 7. 自己レビュー（R1）と修正（R2）

初回実装snapshotに対し、通信用・永続化・service経路をファイル横断で追跡し、次の問題を検出して修正した。

1. **大結果pageの非整合**（再現: 日本語100KiBで `page_text` がchar境界panic／再結合不一致）→ page境界を先頭から走査する方式に変更し、`page_count_for` と `page_text` を同じ境界計算に統一。全page連結一致を試験化。
2. **session 404後の再初期化漏れ**（再現: `tools/list` の404でstateがReadyのまま）→ `list_page` でもSessionExpiredでReconnectingへ遷移。次操作で再initialize、進行中callは自動再送しないことを試験化。
3. **mockサーバーのcursor扱い誤り**（再現: 2回目syncが空pageを返しepochが動く）→ cursor→page対応をstateで保持し、no-op再同期のepoch不変を確認。
4. **`sources.kind` rebuildのFK**（再現: defer_foreign_keysではcommit時FK違反）→ pre-transactionでforeign_keys OFFにして再構築。`foreign_key_check` 空を実DBで確認。
5. **再起動reconcileの外部call識別**（指摘: 単にinterruptedでremote不明を区別しない）→ MCP revisionのrunningを `error_code='remote-outcome-unknown'` で終端。
6. **module-size ratchet超過**（指摘: service.rs等が上限超過）→ D4 schemaを `mcp/schema.rs`、tool解決を `resolve.rs`、backend組立を `mcp/wiring.rs`、service支援を `mcp/service_support.rs`、LLM定義を `gateway_schemas.rs` に分離。上限緩和ではなく既存baseline内に収めた。
7. **401の扱い**（指摘: unknown扱いで再試行余地に見える）→ 401/403は `remote-unauthorized` の非再試行失敗として区別。

## 8. 未達・制限（完了扱いにしない）

- **A13 / 実ML**: E5/BGE未配備のため実モデル評価・1500/10000件負荷測定は未実施。mock輸送試験を実ML評価とは呼ばない。
- **A11**: 実会話provider経由の自然言語訂正は本環境で未実施。訂正記憶のfixture経路（D0〜D3）は維持。
- **A08/A09/A12/A14/A15の一部**: 送信後取消・barrier競合・100call集計・redirect実測・403は未整備。実装は上記の通り。
- **project scope の結果ACL**: `mcp_results` は project_id を保存しないため、取得時の再検査は user grant または project grant の存在で判定する。会話run/scope・principal・TTLは厳密に検査する。
- **source URL変更時の訂正rule**: tool_id は source_id+toolName のため、URLだけ変更しても同一 tool_id となり既存ruleが自動適用されうる。計画の「管理上の再確認まで保留」は未実装。
- **`result-storage-limit` の end-to-end**: 実装・応答契約はあるが、profile 32MiB を実通信で満たす試験は未追加（`result-size-limit` は検証済み）。
- **D5/D6**: 未実施。

## 9. 変更ファイル（主要）

`src-tauri/src/tool_selection/mcp/*`（新規14）、`backends/mcp.rs`・`backends/router.rs`（新規）、`resolve.rs`・`gateway_schemas.rs`（新規）、`contracts.rs`・`repository.rs`・`service.rs`・`gateway.rs`・`schema.rs`・`feedback.rs`（変更）、`persistence/schema.rs`（version 24）、`lib.rs`（起動/停止）、`scripts/module-size-baseline.json`（新規file登録、既存値は不変）。
