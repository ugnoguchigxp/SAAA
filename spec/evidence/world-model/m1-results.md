# M1 結果・性能測定

## 合格した検証 (K1〜K5)

- K1 `bun run check:personal-state`: fmt / clippy -D warnings / core test すべて成功。
  core は 26 passed（既存 + world_contract 2 + world_traversal 7）。
- K2 `cargo test --locked --manifest-path src-tauri/Cargo.toml memory::personal_state`:
  48 passed / 0 failed（world adapter 24 を含む）。
- `cargo test --locked --manifest-path src-tauri/Cargo.toml` 全体: 653 passed / 0 failed。
- `bun run check:local`: 成功（format / lint / check 全体）。
- `bun run test:rust-packages`: 成功。
- K3 `cargo test ... runtime::context`: 13 passed / 0 failed。
- K4 `cargo test ... persistence::`: 82 passed / 1 ignored / 0 failed。
- K5 `bun run size:check`: `module-size ok (595 files)`。World 新規 module と
  `worker.rs` を baseline に登録し、ratchet 内。
- `bunx --bun spec-html check ./spec/docs --warnings-as-errors`: 変更範囲外の
  `spec/docs/saaa-llang-dynamic-capability-m2b-transport-reference.md`（別作業の新規文書、
  MD001 二重 H1）のみ失敗。M1 の文書は通過。M1 では未解消として報告し、無関係な修正は行わない。

## T01〜T22 の対応

| ID | テスト |
| --- | --- |
| T01 | `crates/personal-state-core/tests/world_contract.rs::t01_*` |
| T02 | 同 `t02_*` |
| T03 | `world/tests.rs::t03_*` |
| T04 | `world/tests.rs::t10_*`（端点不在の直接 commit 拒否） |
| T05 | `world/tests.rs::t05_*`（World 循環許容 / Assertion 依存循環拒否） |
| T06 | `world/tests.rs::t06_*`（二重 Active 拒否・置換・Dispute） |
| T07 | `world/tests.rs::t07_*`（Objective 撤回で Focus 失効） |
| T08 | `world/tests.rs::t08_*`（migration 再実行・表・索引・trigger） |
| T09 | `world/tests.rs::t09_*`（再構築の再現性・空投影） |
| T10 | `world/tests.rs::t10_*` |
| T11 | `world/tests.rs::t11_*`（同 patch no-op / 異内容 Conflict） |
| T12 | `world/tests.rs::t12_*`（忘却で投影消去） |
| T13 | `world/tests.rs::t13_*`（Source 編集で旧版を使わない） |
| T14 | `world/tests.rs::t14_scope_isolation_and_authorization` |
| T15 | `world/tests.rs::t15_*`（stale / pending_review） |
| T16 | `core/tests/world_traversal.rs::t16_*` と `world/tests.rs::t16_t17_*` |
| T17 | `core/tests/world_traversal.rs::t17_*` |
| T18 | `core/tests/world_traversal.rs::t18_t19_*` |
| T19 | 同 |
| T20 | `world/tests.rs::t20_*` |
| T21 | `core/tests/world_traversal.rs::t21_*` と `world/tests.rs::t21_*` |
| T22 | `world/tests.rs::t22_*` |

## 性能測定 (WM-14)

- 環境: 同一端末・同一 debug build、`cargo test ... -- --nocapture`。
- fixture: 合成 Entity を 8 件/patch で commit。warm-up 5、measure 30、単位 ms。
- 出力: `WORLD_PERF scale=... load=(median,p95,max) rebuild=... query=... commit=...`

| scale | load median/p95/max | rebuild | query | commit (5 件) |
| --- | --- | --- | --- | --- |
| 0 | 0.056 / 0.057 / 0.057 | 0.131 / 0.133 / 0.136 | 0.127 / 0.130 / 0.132 | 1.58 / 1.89 / 1.89 |
| 100 | 3.71 / 4.29 / 5.28 | 18.4 / 21.0 / 31.9 | 5.35 / 6.27 / 8.73 | 30.3 / 31.4 / 31.4 |
| 1,000 | 31.0 / 40.7 / 45.1 | 249.7 / 351.0 / 356.1 | 42.9 / 62.8 / 63.2 | 518.6 / 643.9 / 643.9 |

- 通常の test 実行は scale 0/100 のみ。1,000 は `SAAA_WORLD_PERF=1` で実行した。
- 10,000 項目: **中止**。理由: 8 件/patch 制約のため 1,250 patch が必要で、
  commit が既存件数に比例して増える（1,000 件で既に 1 commit ≈ 0.52 s）ため、
  fixture 構築だけで 5 秒/fixture の中断条件を大きく超える。load・rebuild も線形超で
  伸びるため、M1 では達成値を断定しない。製品規模の保持方針は次段階で決める。
- commit は 1 回の Writer transaction で load + 全投影再構築を含む。1,000 件の
  rebuild 250 ms と commit 519 ms は、M1 の診断用途（非同期・非対話）では許容範囲。
  対話遅延の合格値は定めない。

## 投影の外部キー

`personal_world_entities/relations/focus` の `assertion_id` は `personal_assertions(id)` を、
relation/focus の端点は `personal_world_entities(project_scope, entity_id)` を参照する。
再構築は Entity → Relation → Focus の 3 パスで行う。忘却時の消去は CASCADE に任せず、
専用 trigger が明示的に全投影を消す。

## レビュー R1〜R10 の反映

`m1-review-2026-09-20.md` の指摘はすべて製品コードへ反映し、回帰試験を追加した。

| 指摘 | 反映 | 回帰試験 |
| --- | --- | --- |
| R1 個別 Assertion の認可 | principal・classification・purpose・scope を `PERMITTED_ASSERTION` で照合 | `r01_*` |
| R2 時間経過の失効 | Entity/Focus/Objective の valid_until と erased を read 時に検査 | `r02_*` |
| R3 scan 予算枯渇 | edge 取得は scan を消費しない | `r03_*` |
| R4 causal reverse | forward と reverse を別々に隣接登録 | `r04_*`（core） |
| R5 条件不一致 | 最大経路選択の前に条件一致を検査 | `r05_*` |
| R6 Focus 直積 | `(focus, edge)` 対応で Gap 生成 | `r06_*` |
| R7 Node 上限 | Focus を含む Slice 全体で Node 上限を検査 | `r07_*` |
| R8 byte 予算 | `min` のみで上限側だけ制限、小予算はエラー | `r08_*` |
| R9 trim 後の参照欠落 | prune で Focus/Gap の参照整合性を保つ | `r09_*`（core） |
| R10 経路上限 | 返却上限は Focus 到達・最大経路選択後に適用 | `r10_*`（core） |

## 検証範囲の区別（レビュー指摘への回答）

- 確認済み: 型・正規化・キー、共通 commit 経由の保存、投影再構築、忘却、Scope 分離、
  stale/pending、関連/因果探索、Focus/Gap、byte 予算、基本 fixture の end-to-end。
- T13 は Source 編集（旧版を使わない）に加え、既存
  `real_db_backup_restore_merges_current_journal_and_missing_journal_blocks_open`
  が「現在 journal を使う復元」「journal 欠落時の拒否」「削除済みが復活しない」を担う。
  本記録では両者を T13 の確認範囲とする。
- 未確認: 10,000 項目の性能認定、live 推論、Context Broker 接続、会話からの自動抽出。
  これらは M2/M3 の範囲であり、M1 完了の根拠にしない。

## 未実施・未接続（M2 以降）

- 会話からの自動抽出、Runtime 状態取り込み、Context Broker への WorldSlice 供給、
  外部 Source Adapter、UI/IPC/HTTP/MCP 新 API、Gap の永続化・調査キュー。
- `world::query::activate` は M1 ではテストからのみ呼ぶ（Provider へ未接続）。
- 10,000 項目規模の実測、履歴増加時の保持方針。

## レビュー指摘 R1〜R10 の対応（2026-09-20）

`m1-review-2026-09-20.md` の10件を修正し、回帰試験を追加した。M1 の意味論・
上限値・既存 API は変えず、読み取り時の再検査と探索/整形の境界条件を直した。

| ID | 修正 | 回帰試験 |
| --- | --- | --- |
| R1 | 読み取り時に `personal_assertions` の AccessScope（principal / scope / task_request / purposes / classification / policy）を再検査。project scope は `context_scopes.state='active'` を確認。entity・alias・relation・focus の全経路に適用 | `world/tests.rs::r01_principal_classification_and_revoked_scope_are_denied` |
| R2 | entity の有効期間、relation 端点の有効性、Focus と `current_work` Objective の期限を読み取り時に検査 | `r02_time_only_expiry_hides_entities_and_current_work` |
| R3 | edge 取得で traversal の scan 予算を消費しない。seed 隣接を優先して取得し、取得と探索の予算を分離 | `r03_edge_fetch_does_not_consume_the_traversal_scan_budget` |
| R4 | Causal/Forward は from→to のみ、Causal/Reverse は to→from のみ。Related の双方向探索と分離 | `core/tests/world_traversal.rs::r04_causal_reverse_does_not_follow_forward_edges` |
| R5 | 条件一致を展開後・最大経路選択の前に適用。条件配列の順序も正規化 | `r05_condition_mismatch_does_not_erase_a_valid_short_path`, `t18_t19_*` |
| R6 | ResearchGap は (Focus, causal edge) の組から生成し、無関係な Focus との直積を作らない | `r06_unrelated_focus_does_not_join_another_focus_gap` |
| R7 | Focus 取得と Slice node 数を同じ予算で制限。経路は全 node が収まる場合のみ採用する | `r07_nodes_are_bounded_even_with_focus` |
| R8 | `max_bytes` は最大値のみ制限。256 byte 未満は `world-budget-too-small` を返し、要求を勝手に引き上げない | `r08_small_byte_budget_is_an_explicit_error` |
| R9 | trim の前後で参照閉包を再計算し、node を欠く Focus / Gap を返さない | `core/...::r09_trim_drops_focus_and_gaps_whose_node_is_absent` |
| R10 | 返却経路数の上限は Focus 到達・最大経路選択の後に適用し、中間 prefix で使い切らない | `core/...::r10_prefix_paths_do_not_consume_the_return_cap` |

再検証（2026-09-20）:

- `cargo test --locked --manifest-path crates/personal-state-core/Cargo.toml`: 26 passed
  （unit 17 + world_contract 2 + world_traversal 7）。
- `cargo test --locked --manifest-path src-tauri/Cargo.toml --lib personal_state::world`:
  24 passed / 0 failed。
- `cargo test --locked --manifest-path src-tauri/Cargo.toml --lib`: 648 passed / 0 failed /
  13 ignored（全 661 test）。
- `bun run check:personal-state`: 成功（fmt / clippy -D warnings / core test）。
- `cargo clippy --locked --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings`: 成功。
- `bun run size:check`: `module-size ok (595 files)`。

## 完了記録の訂正

- T13 の対応先は Source 編集の試験であり、計画にある「World を含む古い DB と current
  journal からの復元」とは別物である。T13 は Source 版選択の試験として限定して扱う。
- 既存 Continuity 試験は実行時に 17 件。以前の記録の「既存 0」は誤記であり、
  `crates/personal-state-core` の unit 17 件が該当する。
- `m1-progress.md` は WM-00〜15 を done としているが、上記 R1〜R10 の境界条件は
  本追記の試験で確認した。未接続・未実施（Provider 接続、10,000 件実測）は
  引き続き未完了として扱う。

## 追加レビュー（第2パス）

境界条件をさらに厳密化し、回帰試験を追加した（core 28 / adapter world 27）。

| 改善 | 内容 | テスト |
| --- | --- | --- |
| 端点の状態 | 既存 Entity は Active のみ参照可。Candidate/Disputed の端点を拒否 | adapter `r13_*` |
| 因果の始点 | Project を因果始点から除外（concept/metric のみ許可） | adapter `r11_*` |
| 容量判定 | 同 patch の Retract/Invalidate/Supersede を差し引いて 10,000 件を判定 | core `enforce_capacity` |
| 監査述語 | `LIKE 'world_%'` を完全一致 `IN (...)` へ変更 | adapter query |
| seed 解決 | 未解決 seed が 1 つでもあれば `unknown_seed`、4 件超は `world-limit` | adapter `r12_*` |
| Gap 順序 | reason → Focus区分 → 経路長 → relation assertion ID → key | core `r11_*` |
| byte 削減 | 経路・Gap を削り切った後は低順位 Focus も削る | core `r09_*` |
| Focus 打切り | Focus 取得件数の超過を `truncated:nodes` として返す | adapter `r07_*` |
| 探索上限 | `Limits::capped()` で depth3/node30/edge60/path10/scan500 を強制 | core `r12_*` |

未使用の `world_source_refs_limited` を削除し、上限は `MAX_WORLD_SOURCES` に一本化した。

### 併せて修正した別領域のレビュー指摘

- generated capability の期限切れ予算を invoke 前に拒否（`tools.rs`）。`Duration::ZERO`
  では durable call を作らない。回帰: `adapter_timeout::x04b_*`。
- generated tools 公開設定の相対パスを拒否（`publication.rs`）。回帰:
  `publication::p02b_*`。
- 音声ツール進捗の固定 60 秒を、会話ターンの実際の残り時間へ置換
  （`voice_progress.rs` / `chat_completions/mod.rs`）。
