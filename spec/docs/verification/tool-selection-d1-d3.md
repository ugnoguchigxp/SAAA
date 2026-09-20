# ツール選択 D0〜D3 実装・検証報告

作成日: 2026-09-20 / 対象: [D0〜D3詳細実装手順](../saaa-tool-selection-d0-d3-implementation-guide.md)
上位: [M2B v2](../saaa-llang-dynamic-capability-m2b-plan.md)

## 1. 完了状況の要約

| 段階 | 状態 | 根拠 |
| --- | --- | --- |
| D0 | 完了 | 開始snapshot、既存/M2A回帰、契約固定、評価dataset、manifest/lock |
| D1 | 完了 | 全table/FTS/migration、1,500件ingest、3入口、参照TTL/上限 |
| D2 | 完了 | 実モデル配備（E5-small＋bge-reranker-v2-m3）、live laneでRecall@30=1.00 / Hit@5=1.00（held-out含む） |
| D3 | 完了 | 条件付き訂正記憶の決定的fixture G01〜G20、会話provider経由の構造化抽出（mock HTTPで実検証）、冪等/撤回/rollback/削除連鎖 |
| D4/D5 | 対象外 | MCP transport は今回未実装 |
| D6 | 対象外 | 順位学習 |

実装したもの: SQLite台帳とmigration、catalog/grant、L-Lang adapter（実fixture invoke）、FTS trigram＋embedding＋RRF＋cross-encoder、条件付き訂正記憶、opaque参照、gateway 3入口、Python worker/prepare_models、評価dataset/CLI、turn hookとprovider接続、会話provider経由のtoolなし構造化抽出。

## 2. 実装ファイル

新規 `src-tauri/src/tool_selection/`:

| ファイル | 責務 |
| --- | --- |
| `contracts.rs` | config、RequestContext、error code、semantic label、limits |
| `schema.rs` | `tool_selection_*` migration、FTS5 trigram、索引、FK |
| `repository.rs` | SQL、epoch、decision/candidate/invocation/feedback/rule |
| `catalog.rs` | 登録、usage page、grant、`is_authorized` |
| `ranking.rs` | cosine、RRF、base正規化、補正clamp、stable topo sort（golden test） |
| `rules.rs` | scope/条件一致、soft補正、forbid、pairwise |
| `retrieval.rs` | query/document prefix、安全なFTS MATCH生成、vector順位 |
| `feedback.rs` | 抽出出力の厳格検証、冪等signature、保存transaction |
| `references.rs` | run所有opaque UUID、10分TTL、64件/run |
| `gateway.rs` | `tools_search/describe/invoke` schema・envelope・dispatch |
| `service.rs` | search/describe/invoke/begin_turn、epoch再検査、L-Lang bridge、batch embedding |
| `inference.rs` / `worker.rs` | ML境界、JSONL framing、per-model load deadline、終了/kill |
| `extraction.rs` / `provider_extraction.rs` | 抽出境界、会話provider解決＋toolなし構造化request |
| `backends/mod.rs` / `backends/llang.rs` | backend境界、既存`CapabilityService::invoke`への変換 |
| `tests/mod.rs` | G01〜G20＋gateway＋provider抽出＋E01 |

その他: `providers/openai_compatible/structured.rs`（toolなし一回request）、`crates/larm-session/src/http_api.rs`（`temperature: Option<f32>`追加）、`examples/evaluate_tool_selection.rs`、`scripts/tool-selection/{worker.py,prepare_models.py,evaluate.py,generate_fixtures.py,requirements.lock}`、`src-tauri/tests/fixtures/tool-selection/`。

接続: `runtime/turns.rs`（message永続化後・最初のprovider request前に`begin_turn`）、`providers/stream/agent_dispatch.rs`（3入口の定義追加とdispatch）、`AppState`/`test_state.rs`、`persistence/schema.rs`（version 21→22）。

## 3. 検証コマンドと結果

```sh
cargo test --locked --manifest-path src-tauri/Cargo.toml --lib tool_selection        # 73 passed
cargo test --locked --manifest-path src-tauri/Cargo.toml --lib generated_capabilities # 67 passed
cargo test --locked --manifest-path src-tauri/Cargo.toml --lib providers             # 79 passed, 2 ignored
cargo test --locked --manifest-path src-tauri/Cargo.toml --lib                       # 753 passed, 0 failed, 14 ignored
cargo clippy --locked --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings  # clean
cargo fmt --check --manifest-path src-tauri/Cargo.toml                               # clean
bun run size:check                                                                   # module-size ok (622 files)
cargo test --locked --manifest-path crates/larm-session/Cargo.toml                   # 22 passed
```

### 実モデルlane（D2）

モデルは `prepare_models.py` で取得し、ローカルfilesのみをload（remote code実行なし）:

| モデル | ローカルpath | manifest hash |
| --- | --- | --- |
| intfloat/multilingual-e5-small | `~/.cache/saaa-tool-selection/models/multilingual-e5-small` | `7bec5bd9d05be2d114fdc30b1d2fe97cb75e41d5c16aefcbaccf2239fb687bad` |
| BAAI/bge-reranker-v2-m3 | `~/.cache/saaa-tool-selection/models/bge-reranker-v2-m3` | `0796f1af886ba785bbeba9a9c37fd628df7be82203785c00b42c566a2c624714` |

package versions: sentence-transformers 5.4.0 / transformers 5.5.3 / torch 2.11.0 / huggingface-hub 1.10.1。dimension 384。専用venv（`~/.cache/saaa-tool-selection/venv`、`--system-site-packages`）。no-match閾値はvalidation splitで選定: validation適格の最小top logit `3.2200`、validation no_matchの最大top logit `-5.5812` の中点 **-1.1806**（誤受入0/20、適格150/150受理）。held-outでは再調整していない。

live lane実行（`catalog.jsonl` 1,500件、`index_embeddings` 1,500件、170 scenario）:

```sh
cargo run --locked --manifest-path src-tauri/Cargo.toml --example evaluate_tool_selection -- \
  --catalog src-tauri/tests/fixtures/tool-selection/catalog.jsonl \
  --scenarios src-tauri/tests/fixtures/tool-selection/scenarios.jsonl \
  --output /tmp/tool-selection-live-verified.json \
  --manifest ~/.cache/saaa-tool-selection/manifest.json \
  --python ~/.cache/saaa-tool-selection/venv/bin/python
python3 scripts/tool-selection/evaluate.py --results /tmp/tool-selection-live-verified.json \
  --split src-tauri/tests/fixtures/tool-selection/split.json --output /tmp/tool-selection-live-verified-metrics.json
```

結果（`lane=live`、degraded 0）:

| 指標 | all | development | validation | held-out |
| --- | --- | --- | --- | --- |
| Recall@30 | 1.00 | 1.00 | 1.00 | 1.00 |
| Hit@5 | 1.00 | 1.00 | 1.00 | 1.00 |

no_match: 20件中20件がstatus=no_match、誤受入0（0%）、degraded 0。実行時間 5m48s（MPS）。mock laneの旧結果（0.80/0.69）はhash doubleの配線確認であり精度根拠ではない。

コードレビューでE5 prefixの二重適用を修正した後、同じlive laneを再計測し **Recall@30=1.00 / Hit@5=1.00 / no_match誤受入0% / degraded 0**（4m33s）を再確認した（`/tmp/tool-selection-live-prefixfix-metrics.json`）。

## 4. G fixture と実テスト名

| ID | テスト |
| --- | --- |
| G01 | `tool_selection::tests::g01_basic_order_is_reranker_order` |
| G02 | `tool_selection::tests::g02_project_correction_applies_to_the_next_search` |
| G03 | `tool_selection::tests::g03_other_object_type_keeps_the_base_order` |
| G04 | `tool_selection::tests::g04_other_project_and_principal_are_not_corrected` |
| G05 | `tool_selection::tests::g05_unknown_object_does_not_receive_the_correction` |
| G06 | `tool_selection::tests::g06_repeated_message_is_idempotent` |
| G07 | `tool_selection::tests::g07_arguments_correction_creates_no_tool_choice_rule` |
| G08 | `tool_selection::tests::g08_ambiguous_correction_creates_no_rule` |
| G09 | `tool_selection::tests::g09_successful_invocation_does_not_create_positive_feedback` |
| G10 | `tool_selection::tests::g10_once_without_task_is_conversation_scoped` |
| G11 | `tool_selection::tests::g11_revoke_removes_the_active_rule_and_bumps_epoch` |
| G12 | `tool_selection::tests::g12_unauthorized_tool_never_appears_even_with_a_preference` |
| G13 | `tool_selection::tests::g13_rule_commit_during_inference_triggers_one_research` |
| G14 | `tool_selection::tests::g14_correction_after_describe_makes_invoke_change_selection` |
| G15 | `tool_selection::tests::g15_unknown_decision_id_is_rejected` |
| G16 | `tool_selection::tests::g16_invalid_byte_boundary_is_rejected` |
| G17 | `tool_selection::tests::g17_storage_failure_rolls_back_and_keeps_the_epoch` |
| G18 | `tool_selection::tests::g18_deleting_the_source_message_removes_derived_rules` |
| G19 | `tool_selection::tests::g19_old_execution_ref_is_stale_after_a_revision_change` |
| G20 | `tool_selection::tests::g20_unavailable_reranker_degrades_without_faking_confidence` ＋ `tool_selection::worker::tests::timeout_kills_the_worker_and_the_next_request_is_isolated`（実プロセス） |

追加の実検証:
- provider抽出: `tool_selection::tests::provider_extraction_uses_the_configured_conversation_provider`（mock HTTPがtoolなし構造化responseを返す）、`provider_extraction_absent_provider_degrades`。
- L-Lang実invoke: `tool_selection::tests::e01_real_llang_invoke_true_and_false_through_selection_backend`（実`CapabilityService`＋fixture A、true/false両方、backend call 2回）。
- gateway: `gateway_search_describe_invoke_round_trip`、`gateway_rejects_unknown_arguments_without_leaking_details`。
- worker framing: `tool_selection::worker::tests::*`（NaN/Inf、id/hash/dimension、rerank順序）。

## 5. 未実施・制約（正確な区分）

1. **live外部providerでの抽出**: 抽出transportは実装し、mock HTTP providerで実検証済み。この環境に設定済みの外部会話provider/credentialがないため、実providerへの構造化requestは未実施。dynamic LAN/harness providerは抽出対象外（`provider()`が`None`を返しdegraded）。
2. **閾値の一般化**: no_match閾値 `-1.1806` は本datasetのvalidation splitで選定した値であり、他datasetへの一般化を保証しない。
3. **性能目標**: p50/p95の詳細計測は未実施（live 170 scenarioが5m48s、hardware MPS）。warm検索・rerank p95≤1秒の目標は未計測。
4. **D4/D5/D6**: MCP transport、順位学習は対象外。

## 6. 反例レビュー

- (1) 無関係な訂正の適用: `scope_matches`はprincipal一致を全scopeで必須化、`condition_matches`はoperation/object必須、phase/input/source指定時一致必須。G03/G04/G05で順位不変。
- (2) scope不明のuser全体保存: scope必須。project/task IDなしのpersistent projectはconversationへ縮小し`scope_shrunk`を返す。user scopeは明示時のみ。
- (3) SQL直書きだけの完成: gateway dispatch経由の3入口＋会話provider抽出transportをmock HTTPで実検証。抽出器は`ConversationProviderExtractor`が設定済みproviderを解決。
- (4) rerankerがmockのまま: live laneで実bge-rerankerによるRecall@30/Hit@5=1.00。lane=live/mockを出力ヘッダで区別。
- (5) 検索中訂正が古い参照に負ける: 推論前epoch snapshot＋保存transaction内再照合＋最大1回再検索。G13でrule_epoch変更→再検索2回。
- (6) API成功からpositive生成: G09でsucceeded後もsatisfaction=unknown。`explicit_positive`のみが満足度を設定。
- (7) describe後schema変更で最新版へ転送: executionRefはrevision/schema_hash/epochを保持、`current_revision`が再検査。G14=selection-changed、G19=stale-reference、backend起動0。
- (8) 別作業の変更混入: 開始snapshotを保存し、world-modelファイルは変更していない。

## 7. Snapshot

- 開始時: `/tmp/saaa-tool-selection-snapshot-20260920T072517`（HEAD `b3c46065`）
- 初回実装（R1レビュー対象）: `/tmp/saaa-tool-selection-r1-20260920T075911`
- 最終（コードレビュー修正・prefix修正後のlive検証）: `/tmp/saaa-tool-selection-final-20260920T113108`（live metrics・model manifest含む）

## 8. 自己レビューで見つけて修正したもの

初回実装のレビュー:
- 評価datasetのscale toolがcurated purposeを逐語コピーしgoldが識別不能 → 無関係なインフラ語彙へ変更。
- FTSクエリがintent全体を単一引用句にし長文で無一致 → トークン単位に引用エスケープしてOR結合。
- workerの`loaded`フラグがembedding/reranker共通で初回rerankがload中にtimeout → モデル別deadline。
- embedding indexが1リクエスト1,500文書でJSONL 2 MiB行上限超過 → 64件ずつbatch化。
- `recent_decisions`が存在しない`tool_id`列参照 → revision joinへ修正。
- 却下feedbackの不正`decision_id`をFKに使いrollback → 却下は`decision_id=NULL`。
- project ruleのprincipal不一致が`apply_rules`単体で漏れる → `StoredRule.principal_id`を必須化。
- avoid/prefer矛盾の二重加算 → condition単位でnarrower scope/newer ruleを優先。
- 会話provider抽出でrouting source誤値（"explicit"）→ 正規値"provider"。

第2次レビュー（追加修正）:
- **E5/Reranker prefixの二重適用**: Rust側が`query:`/`passage:`を付け、workerも付与していた。workerのみが付与するようRustはraw textを渡し、rerankerもraw query/documentを受けるように修正。live laneは依然 Recall@30=1.00 / Hit@5=1.00。
- **dedupeがscope/conditionフィルタ前**: 一致しない狭いscopeルールが一致する広いscopeルールを隠す → フィルタ後にdedupe。
- **discoveryで生成toolと3入口を併存**: discovery modeでは`gc_`生成toolをofferせず3入口に置換（両方提示しない）。
- **worker再起動後のload deadline**: kill時にloadedフラグを戻し、再起動後の初回はload deadlineを使う。
- **抽出promptの不正JSON**: 16 KiB超過時にJSONを途中切断していた → 古いdecisionから落として常にvalid JSON。全1,500 tool名を提示せず直近選択tool名のみ。
- **describeの実行参照**: contractが大きすぎる場合もexecutionRefを発行していた → 発行しない。
- **撤回の過剰適用**: 解決不能なtool名の撤回が同scopeの全ruleをrevoke → 未解決ならambiguous。
- **embedding index欠落**: vector branch空をOk扱い → Degradedとして報告。
- **未使用コード削除**: 未使用のrepository/reference/ranking/retrieval/catalog関数とClosureExtractor/MapEmbedding等を削除。
- **未使用のgrant API**: `catalog::grant_user`/`grant_project`をテストで検証。
- **gateway型検証**: `section`/`cursor`の非文字列を黙って既定値にせずinvalid。
- **gateway決定のmessage紐付け**: retry idが無い場合はrunの`input_message_id`を参照。
- **起動時のrunning invocation回収**: crashで残ったrunningをinterruptedへ終端。
