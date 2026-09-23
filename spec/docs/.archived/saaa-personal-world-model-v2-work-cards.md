# Personal World Model v2 詳細作業カード

改訂日: 2026-09-20。状態: 実装計画。実装未実施。

[全体計画](saaa-personal-world-model-initial-plan.md)のW2番号は到達点のまとまりであり、そのまま一括実装する指示ではない。実装担当へ渡す単位は本書のD00〜D42、計43枚とする。[実装契約](saaa-personal-world-model-v2-execution-contract.md)の指定C節と組み合わせて使う。

## 共通の実行手順

1. 対象カード、指定C節、直前カードの結果を読む。新規の型・関数はC0〜C10の名前と責務を使う。
2. 最小fixtureと期待値を先にテストへ書く。既存試験で同じ条件を確認できる場合は重複追加せず対応を記録する。
3. 許可ファイルの対象関数だけを変更する。新規DTOの登録・import等の機械的変更は可。ファイル移動・周辺の整理はしない。
4. 対象試験と指定suiteを実行し、件数と結果を記録する。0 testsは不合格。赤い試験を残して後続へ進めない。
5. v2-progress.mdへ実際の関数・試験名、結果、未解決事項、次カードを記録する。変更が不要なら確認結果で完了してよい。

全カードはD番号順で、直前カードの完了が前提。D05後が既存基盤ゲート、D33後が保存・照会接続ゲート、D40後が意味・統合ゲート。別レイヤーの変更が必要なら、失敗する最小テストと不足契約を残しカードを分割してから進む。仮実装のalways true、todo!、黙って未知値を既存型へ変換するfallbackを残して完了しない。

coreは `crates/personal-state-core/src/world/`、adapterは `src-tauri/src/memory/personal_state/world/`。core試験は `crates/personal-state-core/tests/world_v2_*.rs`、adapter試験は `world/v2_*_tests.rs` に責務別に分割してmod登録する。既存tests.rsの大きなファイルへすべて追記しない。

## 試験コマンド

カードのテスト関数名は `dNN_説明`。以下のNNを対象番号に置換して使う。filterはテスト名を固定するための指示であり、現在存在する試験名ではない。

```sh
# coreの対象試験
cargo test --locked --manifest-path crates/personal-state-core/Cargo.toml dNN_
# coreカードのsuite
bun run check:personal-state

# adapterの対象試験
cargo test --locked --manifest-path src-tauri/Cargo.toml --lib dNN_
# adapterカードのsuite
cargo test --locked --manifest-path src-tauri/Cargo.toml memory::personal_state
```

restoreは上記対象試験に加え全体計画K4、boundaryはK2＋K3。perfは固定性能fixtureを明示実行し通常unit suiteへ10,000件を混ぜない。docsはbaseline記録、finalはK1〜K6。機械的なadapter import変更を含むcoreカードでは、追加で `cargo check --locked --manifest-path src-tauri/Cargo.toml` を実行する。未使用API段階で既存lintが失敗する場合、allowを全体へ追加せず対象モジュールの既存規約に従う。

## 中間段階の接続規則

D15までのcore新APIは純粋試験で確認する。新Adapterの検証・再構築・照会は、専用の合成snapshotを使って個別に検証し、既存Writerから呼ぶのはD33の接続カードだけとする。それまでは既存v1製品経路を保つ。D33で正規Writerからの統合試験へ切り替える。途中で未完成の型やqueryを製品経路へ配線して、後続カードまでコンパイル不能・試験失敗を持ち越さない。新規APIを追加するだけのカードもテストから実際に呼び、todo!や偽の成功結果で代用しない。

## カード一覧

各行のC節が入力契約、操作が許可変更、期待結果が出力契約である。変更対象に書かれていないレイヤーは対象外。失敗時は下記の共通分岐に従う。

| ID（元の到達点） | 許可ファイル・関数の所在 | 入力契約 | 実施する一責務 | 具体的な期待結果 | 検証lane |
| --- | --- | --- | --- | --- | --- |
| D00（W2-00） | baseline | 全体§2・C0 | git差分、DB版21、現存関数、既存K1〜K4とR1〜R10試験の対応を記録 | 既存失敗と未実施を区別。実装修正を混ぜない | docs |
| D01（W2-01） | adapter/query.rs・tests.rs | C3・C7 | R1の認可試験を再実行し不足ケースだけ追加 | other principal、purpose違い、classification不足、revoked Projectで名称・alias・Focus漏洩0 | adapter |
| D02（W2-01） | adapter/query.rs・tests.rs | C3・C7 | R2の依存有効性をSource・Objectiveまで確認し必要なら補強 | now=200で[0,200)のEntityと依存Relation/Focusが消える。revision変更なし | adapter |
| D03（W2-02） | core/traversal.rs・core/relevance.rs | C0・旧R4/R9/R10 | 既存純粋関数の境界試験を再確認。v2追加はしない | A→BでAからreverseは空、trimにdangling参照なし、10分岐後の到達を保持 | core |
| D04（W2-02） | adapter/query.rs・tests.rs | C0・旧R3/R5/R6/R7/R8 | 現行Readerの残る取得・整形問題だけ修正 | 500件でseed経路あり、不成立prefix保持、無関係Focus Gapなし、nodes=2以内、bytes=1エラー | adapter |
| D05（W2-03） | src-tauri/src/memory/personal_state/tests.rs | C5・C11 | real_db_backup_restore_merges_current_journal_and_missing_journal_blocks_openを参考にWorld入りfixtureを一つ追加 | 古いDB＋current journalで削除済みWorld名・alias・edge復活0。journal欠落拒否 | restore |
| D06（W2-04） | core/model_v2.rs（新規） | C1 | v2 wire struct・enum・構造検証のみ実装 | JSON roundtrip、version=3拒否、score1001拒否、goal参照必須。v1ファイル未変更 | core |
| D07（W2-04） | core/slice_v2.rs（新規） | C8 | 返却DTOだけを定義。選択・探索・整形処理は実装しない | empty envelopeのJSON往復と全field型、既存WorldSlice不変 | core |
| D08（W2-04） | core/versioned.rs（新規） | C1 | decode_versionedと共通読取ビューを追加 | 旧JSONはv1のまま、新JSONだけv2。未知field/tag拒否。旧scoreはnull | core |
| D09（W2-04） | core/identity_v2.rs（新規） | C2 | 固定配列によるキー生成と論理同一性関数を追加 | 相関端点反転は同値、増減反転は別、score変更は同値、v1/v2相当関係は同一性一致 | core |
| D10（W2-05） | core/validation_v2.rs（新規） | C3 | Goal参照とEntity/Focusの純粋検証を実装 | Active Objective o1だけ受理。別kindはInvalidReference、別ProjectはScopeDenied、依存不足拒否 | core |
| D11（W2-06） | core/validation_v2.rs | C3・全体§5.3 | Relation端点と追加属性のマトリクス検証を追加 | 全12型の正例/負例。serves_goalのmetric方向欠落、相関のeffect_input、causes→metricを拒否 | core |
| D12（W2-06） | core/validation_v2.rs | C1・C3 | 根拠・epistemic・score・mechanismの検証を追加 | manual800＋根拠あり受理、根拠なし拒否、supported保存拒否、method偽装拒否 | core |
| D13（W2-06） | core/validation_v2.rs・core/validation.rsのWorldError | C2・C3 | validate_versioned_patchへ検証順序と版横断重複検査を統合 | 同じ意味のv1/v2二重ActiveはDuplicateIdentity、明示Supersedeで通過。容量置換差引き | core |
| D14（W2-07） | core/conditions_v2.rs（新規） | C4 | 条件の三値評価だけを実装 | r1/a成立、r1/b不成立、r1/a+b不明、r2/aはr1に使わない、空条件不明 | core |
| D15（W2-07） | core/conditions_v2.rs | C4 | availability評価を追加 | B availableのみ→available、相反→unknown、観測なし→unknown、Nodeの存在は判定に使わない | core |
| D16（W2-08） | adapter/validation.rs | C3・C5 | validate_commit_v2とversioned loaderを追加。既存commitへの配線は統合カードまで保留 | validate_commit_v2への不正v2入力拒否、non-World patch不変。ReaderやDDLには変更しない | adapter |
| D17（W2-08） | src-tauri/src/memory/personal_state/store.rs | C5 | 適用済みpatchのfingerprint再送確認を意味検証前へ分離 | 置換後再送false、v2内容hash差替え拒否、同ID異内容Conflict、忘却後再送で復活なし、新patchのCAS/fence維持 | adapter |
| D18（W2-09） | adapter/schema.sql・personal_state/schema.rs・src-tauri/src/persistence/schema.rs | C6 | projection_version列と最新＋1のmigrationを追加 | 旧表default1、再migration列重複なし、recover前に列あり。新しいWorld表は0個 | restore |
| D19（W2-09） | adapter/projection.rs | C6 | rebuild_v2を追加して明示呼出しで検証。既存rebuildはまだv1 | 専用snapshot fixtureのv1/v2混在で同じ値、meta=2、Objective撤回でGoal依存が消える、fault rollback | adapter |
| D20（W2-10） | adapter/query.rs | C6 | 旧activateにprojection_version検査だけ追加 | meta1の旧fixtureは従来結果、meta2はprojection_stale。未知kindを旧Conceptへ変換しない | adapter |
| D21（W2-10） | adapter/query_v2.rs（新規） | C7 | ActivateInputV2とload_query_context_v2を実装。activate_v2本体は統合時に追加 | scope不正エラー、meta不一致notice、seed曖昧は候補扱い、DB書込み0 | adapter |
| D22（W2-10） | adapter/observations_v2.rs（新規） | C4・C7 | Source/Project/時刻を検査する観測Adapterを実装 | 有効観測だけcore入力へ、別Project拒否、旧版除外＋notice、30件超Limit | adapter |
| D23（W2-10） | adapter/query_v2.rs | C6・C7 | Entity・Goalの依存閉包を読み取り時に再検証 | 時刻だけ進めたGoal/Source失効を反映し、失効名をSliceへ出さない | adapter |
| D24（W2-11） | core/traversal_v2.rs（新規） | C7・C8 | v2 frontier・方向・visited・条件展開の純粋状態機械を実装 | forward/reverse分離、cycle停止、不成立prefix保持、unknownは説明候補、path枠は最終結果だけ | core |
| D25（W2-11） | core/traversal_v2.rs | C8 | evaluate_pathとeffect_summaryを追加 | +−+ /900,800,700 /共通基準→decrease,567。基準違い→unknown,null。対立はsummaryのみmixed | core |
| D26（W2-12） | core/traversal_v2.rs | C7・C8 | Goal関連経路の終点判定を追加 | 技術→2指標→Goal←Projectが4 hopで到達。has_goal逆向きは因果へ入らない | core |
| D27（W2-12） | core/relevance_v2.rs（新規） | C4・C8 | 相関・依存の専用結果への変換を追加 | 相関対称、dependenciesは必要先とavailabilityを返す。作用edge生成0件 | core |
| D28（W2-13） | core/relevance_v2.rs | C9 | Gap六種の判定・固定文・key生成を追加 | null/500は低評価でない、499は候補、未知seedに架空IDなし、無関係Focusとの直積なし | core |
| D29（W2-13） | core/relevance_v2.rs | C9 | Focus順位とrequest内attentionを追加 | current_work→explicit→temporary、同順位距離とID、失効Objectiveの自動降格なし | core |
| D30（W2-14） | core/slice_v2.rs | C8 | 先に定義したWorldSliceV2へ説明単位のunion・参照閉包・byte選択を実装 | nodes2/bytes小で単位ごと省略、1 byteエラー、残した条件/根拠欠落0、全参照先存在 | core |
| D31（W2-14） | adapter/query_v2.rs | C7 | BFS frontierの隣接SQL取得とfetchカウンタを追加 | 二段目もfrontier取得。LIMIT=残数、fetch<=500、残数0のSQL0件、LIMIT+1なし | adapter |
| D32（W2-14） | adapter/query_v2.rs | C7〜C9 | activate_v2を完成。専用snapshotで純粋探索・Focus・Gap・Slice・flagsを接続 | 全16 flag組合せで無効種類なし、全体paths<=10、観測依存含むbyte<=要求、read-only | adapter |
| D33（W2-14） | adapter/validation.rs・adapter/projection.rs・store.rsと統合試験 | C5・C6・C7 | 検証済みvalidate_commit_v2/rebuild_v2を既存Writerへ配線し、旧integration試験をactivate_v2へ移す | 正規Writerでv1/v2混在を保存・照会。旧payload不変、同patch再送、rollback、meta2、旧Readerはstale | adapter |
| D34（W2-15） | core/outcome_v2.rs（新規） | C10 | compare_outcomeの純粋比較・反証候補を実装 | 同条件の逆方向800→640、799→639、null維持、別条件/unknown/unchangedで減衰なし | core |
| D35（W2-15） | core/validation_v2.rs | C3・C10 | OutcomeUpdate再計算とcounterevidence保存の検証を追加 | 任意640の直接保存拒否、正しい旧版/観測/差分だけ受理、旧scoreと合わない値拒否 | core |
| D36（W2-16） | adapter/outcome_v2.rs（新規） | C10 | prepare_outcome_patchと固定operation ID・prepared envelopeを実装 | 旧Supersede→新Assert/Activate/Dispute。新depends_onに旧版なし。入力Source union保持 | adapter |
| D37（W2-16） | adapter/outcome_v2.rs・adapter側試験のみ | C5・C10 | Writer経由の再送・CAS・消去試験を追加し担当内の不具合を修正 | 同prepared再送640のままrevision不変、二つ目旧版StalePatch、反証Source忘却で新評価失効 | adapter |
| D38（W2-17） | adapterのv2統合試験 | C11・全体§9 A/B | Writer fixtureからGoal関連と相関を照会 | Aは関連4/因果2 hop、Goal撤回反映。Bは相関とGapのみ、因果追加0件 | adapter |
| D39（W2-17） | adapterのv2統合試験 | C11・全体§9 C/D/E/F | 多段・依存・未知・Outcomeを一連で検証 | 567、availability三状態、未知Gap、640再送no-op。DBに派生direct edgeなし | adapter |
| D40（W2-18） | src-tauri/src/memory/personal_stateの既存境界試験 | C0・全体§4 | 通常抽出・Personal State投影・公開snapshotからWorldが除外されるか確認 | 五要素追加後も旧Contextへ混入0件。Provider/Broker接続を追加しない | boundary |
| D41（W2-19） | adapterのv2性能fixture | 全体§10 | 長い日本語・履歴・五要素を含む0/100/1000/10000件を測定 | 構築と操作を分離、5秒中断を記録、未測定容量を認定しない | perf |
| D42（W2-20） | spec/evidence/world-model/v2-results.md | 全体§10 | 全試験・サイズ・文書・K6を実行し結果対応表を作成 | D00〜41の証拠、残課題、M2未接続、実施/未実施を明示 | final |

## 失敗時の分岐と担当者の裁量

| 状況 | 次の行動 |
| --- | --- |
| コンパイル・対象試験失敗 | 対象カード内で修正し、その試験と関連suiteを再実行 |
| 基準時点の別機能・別文書の失敗 | baselineとの差を残す。対象外コードを直さず今回の合否と分ける |
| 認可・忘却・CAS・互換性の失敗 | ゲート不通過。後続の機能追加を止め、最小再現を維持して修正 |
| 契約にない選択が必要 | 選択肢・推奨案・影響するC節を記録し、そのカードだけ設計へ戻す。独自に仕様を補わない |
| ファイル・関数が現行で移動済み | 同じ責務の現存先を探して対応表を更新。意味が変わるなら設計へ戻す |
| payload/Source/byte上限に収まらない | LimitまたはBudgetTooSmall。根拠削除や上限引上げで通さない |

private helper名、同じ契約内の関数分割、テストの補助関数名は担当者が決めてよい。型の意味、wire名、列、ハッシュ入力、評価式、処理順、エラー、上限、合格条件は変更しない。カードはcommit単位を強制しないが、独立に検証可能な状態で終える。

## 一枚だけ渡す依頼例

```text
World Model v2のD14だけを実装してください。
読むものは詳細カードのD14、実装契約C4、D13までの結果です。
core/conditions_v2.rsの条件評価とcore単体テストだけを変更してください。
r1/a成立、r1/b不成立、r1/a+b不明、r2/aはr1に影響なし、空条件不明をassertしてください。
対象試験d14_とcheck:personal-stateを実行し、件数と結果をv2-progress.mdへ残してください。
availability、SQL、LLM、後続カードは実装しないでください。
```
