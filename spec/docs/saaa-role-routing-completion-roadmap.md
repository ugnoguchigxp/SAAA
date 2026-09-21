# Role Routing 完了ロードマップ — Terra 実行用

作成日: 2026-09-21。状態: **計画のみ。実装完了を示す文書ではない。**

[作業カード](saaa-role-routing-work-cards.md) / [全体計画](saaa-role-routing-plan.md) / [実行契約](saaa-role-routing-execution-contract.md) / [受入仕様](saaa-role-routing-acceptance.md) / [学習契約](saaa-role-routing-learning-contract.md) / [検証証跡](../evidence/role-routing/results.md)

## 1. 目的と使い方

通常の会話 turn から、有限 recipe に従って author、reviewer、reviser、premium、tool specialist を実行し、更新・取消・遅延完了・ツール副作用・音声の競合まで安全に処理する。型、保存関数、単体テストだけでは完了としない。

この文書は RR-00〜39 の要求を置き換えず、**残作業の実行順と小さな実装単位を定める**。既存カードの合格条件、C/L 契約、A01〜A42、P1〜P5 を削除・緩和しない。監査時点の既存カードはすべて部分、完了は 0/40。以下の E/L 作業単位にも完了済みのものはない。

Terra は E00 から依存順に進める。1単位の実装ファイルは原則1〜3、登録・生成物を除いて5を超える場合は a/b 枝番に分割する。表の主要ファイルは責務の所在であり、一度に全部変更する指示ではない。既存 module が別ファイルに分割されていれば実際の境界を使う。新規予定ファイル名は設計案であり、同等の責務が既にあれば再利用する。

各単位は「入口接続→正常系→拒否・故障系→証跡」の順で閉じる。依存する安全条件が不合格なら、その条件を必要とする dispatch は有効化しない。live 許可待ちでも独立した offline 作業は続ける。

### 判定規則

| 状態 | 判定 |
| --- | --- |
| 未着手 | 対象の責務または通常 turn への接続がない |
| 部分 | 部品・限定経路はあるが、指定の実経路、故障・競合試験、証跡のいずれかが欠ける |
| 完了 | 単位の指定経路と試験を実行し、DB・dispatch・tool・speech の期待値を確認して証跡を保存した |

offline 単位は offline で完了可能。live を含む親 RR カードは、offline 子単位が完了しても live 未実施なら部分。0 tests、ignored、環境不足は pass ではない。過去の試験件数を今回の合格として再掲しない。

## 2. 監査結果と優先修正

監査基点は HEAD `7471ecadc1d64a5b3c1d4e6a898669be80a2cb8a` と当日の作業ツリー。関連 runtime、size baseline、他機能に既存 dirty 変更がある。着手時に再取得し、既存差分を上書き・取り込み・revert しない。

現在の実経路は「通常受付 transaction→queue/claim→policy snapshot→単一 actor の Respond→Provider または SDK sidecar→最終回答採用」。次の根拠は静的読解であり、今回の故障再現を意味しない。

| 根拠ファイル | 確認事項 | 対応 |
| --- | --- | --- |
| `runtime/conversation_inputs_roles.rs`、`role_routing/repository_turns.rs` | Respond と actor数1に限定。受付と dispatch がそれぞれ候補選択する | E07〜E10 |
| `role_routing/repository_turns.rs` | 採用 API は結果側 step/revision を受け取らず現在 revision を読む。draining も採用対象。採用・usage は ordinal 0 固定 | E02〜E04 |
| `role_routing/coordinator.rs` | Start が同 revision の planned step を一括 running にする | E03/E08 |
| `role_routing/schema.rs`、`recovery.rs` | active root 制約・検索が responding/draining に限定 | E03/E25 |
| `role_routing/repository_turns.rs`、`runtime/turns.rs` | 受付時に deadline を作り queued 中にも適用。claim 時開始という C5 と不一致 | E06 |
| `tool_selection/gateway.rs` | root から最新 step を推定。step 不在でも予約処理は許可。role 専用 caller の不明状態を拒否しきれていない | E12/E13 |
| 同上 | operation key は名前+引数 hash、link ID は root を含まない。settle は owner の意味を十分に判定せず DB エラーも呼出元へ返さない | E14 |
| `role_routing/tools.rs` | read-only が名前 prefix 判定 | E13 |
| `role_routing/review.rs`、`revision.rs`、`repository.rs` | モデルの verified 文字列を revision 許可へ使用。review target は最初の respond を検索 | E17/E18 |
| `role_routing/proposals.rs` | candidate_id が premium actor ID として扱われる。selection の recipe ID と意味が揃わない | E07/E20 |
| `scripts/role-routing/codex-isolation.ts` | 子 env allowlist に bridge token がない。固定 SDK の envOverride 経路では子へ継承されない構造 | E15 |
| `scripts/role-routing/codex-sidecar.ts` | outputSchema 後処理は JSON parse のみ。例外 message を protocol error code に使用 | E15/E17 |
| `role_routing/speech_queue.rs` | 単一 owner 部品はあるが revision/epoch と永続 speech lifecycle の実接続は不足 | E24 |

表の Rust path は `src-tauri/src/` 基準。review/revise/premium/specialist の通常 executor 接続は未着手。SDK の常時 new thread 自体は安全側だが、同 step tool roundtrip と実認証隔離の証明はまだない。

## 3. 固定する実装契約

### 3.1 状態・識別子・保存

1. SQLite と既存 SqliteWriter を正本とし、frontend や actor cache を別の正本にしない。
2. root の終端 status は `runtime_runs` に集約し、phase は進行段階に使う。既存 completed/cancelled/failed phase からの移行を E01/E03 で定義する。
3. 実行 envelope は rootId、inputRevision、stepId、attemptId、executionEpoch、policyId を持つ。結果・tool session・cancel 完了も同じ束縛を検証する。
4. inputRevision はユーザー条件変更に使う。review 後の文章修正は別 step/output version とし、条件 revision を勝手に進めない。
5. ordinal は root 全体で単調増加。同一操作再送では増やさない。step/output/decision/tool link の root・revision 対応を複合制約または同一 transaction の検査で保証する。
6. step lifecycle は planned→running→succeeded/failed/cancelled/interrupted。停止待ちは draining。旧結果も実際の終了状態を保存し、採否は別に記録する。
7. 同一 terminal ID と同一 digest の再送は元 receipt、異なる payload は conflict。更新件数0を成功として扱わない。
8. step output の有効性と最終回答への採用を分離する。review/draft は assistant message、root completed、TTS を発生させない。
9. 最終採用は barrier、現 revision、現 step/attempt、未確定 tool、scope を transaction で照合し、最終 message・root 終端・event・speech intent を一貫して保存する。
10. DB commit 後にのみ Effect を開始。commit 後 dispatch 前の crash を自動 replay しない。再起動は interrupted として明示再開を求める。

### 3.2 recipe と実行所有者

- 会話ごとに coordinator は1つ。推論 slot は root ごとに最大1、frontend/classifier は別枠最大1、tool は逐次。resourceGroup が同じ場合は競合を避け reasoner を優先する。
- recipe は既知テンプレートを有限計画へ compile。任意 graph、再帰、script、URL 実行を認めない。roles 配列を単純な for-loop で実行しない。
- compile 時に役割、用途別 capability、独立 reviewer、input/output 依存、到達可能な final、最大 steps を検査。全分岐は前進し、review/retry/escalation/specialist も残予算を消費する。
- candidateId、recipeId、actorId を区別。保存済み decision を dispatch の選択元にする。直前再検証は適格性を検査し、暗黙に別候補を選び直さない。
- author は draft、reviewer は評価、reviser は draft と issue 採否、premium は承諾済み推論、specialist は typed tool request を返す。adapter は root 完了権限を持たない。
- reviewer の actor ID は author と異なること。実 provider/model fingerprint も記録し、同一モデル別 alias を別モデル検証の証拠にしない。
- 上限は root の全実行に累積。step/root deadline、tool calls、review rounds、automatic switches、費用予約を各 dispatch で検査。失敗や inputRevision 更新で予算をリセットしない。

### 3.3 tool・security・プライバシー

- sidecar の MCP は既存 host-controlled 認証済み loopback gateway のみ。外部 MCP、native tools、任意ネットワークを暗黙に許可しない。隔離を証明できない adapter は unsupported。
- MCP session は root だけでなく step/revision/attempt/epoch に束縛し、古い session を新 step に付け替えない。token は子環境だけへ最小限伝播する。
- permit は reserve 時と invocation owner の開始引受時に検査。後者を開始の線形化点とする。その前の更新は実行0、その後の更新は owner の実結果を settle してから進む。
- tool の read-only/mutating は解決先 tool の信頼できるメタデータで決める。名前 prefix やモデル申告は使わない。unknown は read-only として許可しない。
- operationId と payloadDigest を分離。同一論理操作の再送は実行しない。同名同引数でも別の正当な read を永久に禁止しない。mutation の再実行には owner の確定状態と新たな意図が必要。
- invocation receipt/resultRef を既存 owner に紐付ける。remote outcome 不明は unknown、DB settle 失敗は成功でない。unknown のまま後続 mutation や自動 retry をしない。
- token・認証情報を JSONL/DB/ログへ保存しない。protocol エラーは固定コード。ログ、export、診断、追加 ledger へ会話本文や tool 引数を複製しない。
- 既存会話正本と実行用 IPC の本文利用は維持し、routing には参照・digest・構造化結果を置く。初期版の中間 draft はメモリ保持、restart は interrupted。本文永続化を追加して再開機能を作るのは今回の範囲外。
- World、memory scope、既存 tool ACL の意味を変えない。各 step で既存の失効・来歴検査を通す。

### 3.4 update・cancel・speech

- 入力受付 commit と同時に pending-input barrier を立てる。classifier 完了前の結果は保留し、発話しない。
- status/social は旧 step を維持して保留解除。amendment は条件追記と revision 更新、旧子の drain。unclear は確認待ちのまま barrier 維持。
- completed 後の追加条件/challenge は新 root。旧 root を running に戻さない。
- executionEpoch と conversation の speechEpoch を分ける。合成完了と再生開始の両方で speechId/epoch を検査する。遅い SpeechEnded が別音声の slot を解放しない。
- cancel commit 後は新 dispatch を禁止。停止要求だけで副作用取消済みとしない。TTS 停止未確認なら次音声を重ねない。音声失敗でも保存済み回答を消さない。

## 4. 依存フェーズ

| フェーズ | 作業 | 出口 |
| --- | --- | --- |
| P0 基準と契約 | E00〜E01 | dirty/契約差分と受入対応の固定 |
| P1 台帳と採用境界 | E02〜E06 | step 単位の採用と入力 barrier |
| P2 executor | E07〜E11 | 通常 turn から有限 step を逐次実行 |
| P3 tool と SDK | E12〜E16 | 同一 gateway・budget・session 隔離 |
| P4 review/revise | E17〜E19 | 独立評価と修正から final へ |
| P5 premium/specialist | E20〜E22 | 承諾と親子 tool 経路 |
| P6 会話・音声・競合 | E23〜E27 | 実経路の競合・再接続受入 |
| P7 学習と運用 | E28〜E34 | 増分・shadow・削除・UI |
| P8 全体 gate | E35〜E37、L01〜L04 | A01〜A42、P1〜P5、V1〜V6 の証跡 |

P1 の barrier は P6 まで先送りしない。P2 は fake adapter と既存単一 actor で閉じ、P3 完了前に multi-step の外部 tool を有効化しない。R3 を省略して全体完成と報告しない。

## 5. 実行単位

以下の test 名は追加・拡張する試験の仕様名。既存の同等試験を再利用する場合は対応表へ実名を記録する。Rust module path は `src-tauri/src/` 基準。新規 module は必要に応じて mod 登録する。

### P0 基準と契約

#### E00 基準と受入台帳 — RR-00/39

- 状態: 部分。依存: なし。
- 対象: `spec/evidence/role-routing/baseline.md`、`progress.md`、新規 `acceptance-matrix.md`。
- 実装: HEAD、dirty、schema、固定 SDK/CLI、既存 command の有無を秘密抜きで記録。A01〜A42、P1〜P5 に担当 E/L、試験名、lane、実行結果を置く。過去ログと今回実行を分離。
- 失敗時: ビルド阻害はファイル・command と再現条件を記録。他機能を無断修正しない。
- 最小検証: RR40件、A42件、P5件の欠落・重複0。
- 完了: 未検証セルを含む対応表が存在し、着手可能な offline 作業が判別できる。

#### E01 状態・データ契約の整合 — RR-01/02/03

- 状態: 部分。依存: E00。
- 対象: execution-contract、work-cards、`role_routing/contracts.rs`。
- 実装: §3 の型、終端正本、phase、candidate、revision、output、deadline を契約化。Clock/ID 注入、未知 field、UTF-8 bytes、finite 値検証を定義。追加識別子の migration 方針を確定。
- 失敗時: 既存契約と衝突する場合は根拠を文書化し、安全条件を維持する案を選ぶ。security 緩和で解決しない。
- 最小試験: `rr_01_unknown_field`、`rr_01_utf8_limit`、`rr_01_invalid_id`。
- 完了: 後続が識別子・状態の意味を推測せず実装でき、受入条件が弱まっていない。

### P1 台帳と採用境界

#### E02 現行採用 API の stale 拒否 — RR-12/16/17

- 状態: 部分。依存: E01。
- 対象: `role_routing/repository_turns.rs`、`runtime/conversation_turn.rs`、`runtime/conversation_role_codex.rs`。
- 実装: 結果側 step/revision/attempt の期待値を採用 API に渡す。draining、pending barrier、cancel、terminal を transaction で拒否。現段階では既存 ordinal 0 経路を保ったまま安全化。
- 失敗時: assistant message を同 transaction で rollback、TTS intent 0。古い成功結果を新 revision として記録しない。
- 最小試験: `rr_12_old_revision_result_rejected`、`rr_16_pending_input_blocks_real_acceptance`、`rr_12_db_failure_no_speech`。
- 完了: Provider/SDK 両呼出元と実保存経路で stale 採用0。pure reducer だけでは不可。

#### E03 step lifecycle と DB 制約 — RR-02/05

- 状態: 部分。依存: E02。
- 対象: schema、coordinator、新規 step repository、既存 migration 入口。
- 実装: 一意 active root/step、step 単位 claim、root 内単調 ordinal、参照整合性、terminal idempotency。新 phase を index/query/recovery に反映。出荷済み migration の書換えで済ませない。
- 失敗時: 不整合旧 row を成功へ補正しない。移行を rollback、dispatch 0。
- 最小試験: `rr_02_migrate_existing_partial_state`、`rr_02_one_active_reasoning_step`、`rr_05_duplicate_completion_once`、FK check。
- 完了: 同 revision の planned step が一括 running にならず、異 root/revision の参照を拒否。

#### E04 中間 output と最終採用の分離 — RR-09/12/22

- 状態: 未着手。依存: E03。
- 対象: step repository、repository_turns、Provider/SDK 完了境界。
- 実装: complete_step と finalize_root を分離。usage を実 step に保存。draft/review は root を完了させない。最終 message/event/speech intent の一回採用。
- 失敗時: output 保存失敗では次 step 開始0。別 payload の重複 terminal は conflict。
- 最小試験: `rr_12_intermediate_output_not_final`、`rr_22_usage_belongs_to_actual_step`、`rr_12_finalize_once`。
- 完了: 2 step 台帳で final 1件、中間発話0、ordinal 0 固定なし。

#### E05 入力 receipt と pending barrier — RR-04/16/17

- 状態: 部分。依存: E04。
- 対象: runtime/turns、routing input repository、coordinator。
- 実装: inputId/sourceId/payloadDigest の再送・conflict、active root 入力の保存と barrier を同一 transaction にする。classifier 結果の世代一致を検査。
- 失敗時: queue full は message 保存0。分類失敗は unclear として保留。複数 pending input を途中で取り落とさない。
- 最小試験: `rr_04_receipt_retry`、`rr_04_same_id_changed_payload`、`rr_16_multiple_pending_inputs`、A03/A13/A15。
- 完了: 通常入力 API で受付 commit 直後から採用が止まり、receipt を DB から復元可能。

#### E06 policy と queue 期限 — RR-03/18/22

- 状態: 部分。依存: E03/E05。
- 対象: repository_policy、recovery、runtime/turns、settings 保存境界。
- 実装: settings CAS、同 digest no-op/rollback の版規則、queued root の snapshot 固定。root deadline は claim 時開始、awaiting_user で延長しない。必要なら queue timeout を別概念にする。
- 失敗時: policy 不明は拒否、期限切れは説明可能な interrupted。最新設定へ黙って差替えない。
- 最小試験: `rr_03_policy_cas_conflict`、`rr_18_queued_policy_immutable`、`rr_22_deadline_starts_at_claim`。
- 完了: queue 待ちで推論予算を消費せず、dispatch 直前の権限撤回は別途効く。

### P2 executor

#### E07 有限 recipe compiler — RR-03/06/22

- 状態: 未着手。依存: E01/E06。
- 対象: contracts、selection、新規 recipe module。
- 実装: 用途付き step、依存 output、条件分岐を既知テンプレートから生成。candidate/recipe/actor ID を分離。review/revise の順と上限を検査。
- 失敗時: cycle、未知 primitive、独立 reviewer 不在、上限不足は理由付き除外。設定の任意コードを実行しない。
- 最小試験: `rr_06_recipe_invalid_dependency`、`rr_22_recipe_all_branches_bounded`、`rr_06_self_review_alias_rejected`。
- 完了: 全分岐の有限性と予算消費が機械検査でき、無効 recipe の dispatch 0。

#### E08 coordinator registry と非同期 driver — RR-05

- 状態: 未着手。依存: E04/E05/E07。
- 対象: coordinator、新規 driver/registry、AppState 登録。
- 実装: 会話単位 actor、mpsc 完了イベント、commit→Effect、step claim、終了時解放。IO中も input/cancel を処理し、transaction/同期Mutex を await 越しに保持しない。
- 失敗時: Effect 起動失敗を step failure に戻す。process restart は replay せず interrupted。
- 最小試験: `rr_05_commit_before_dispatch`、`rr_05_one_actor_per_conversation`、`rr_05_io_does_not_block_input`。
- 完了: fake 2 step が順に動き、途中 cancel が後続を止める。

#### E09 role context と scope — RR-07

- 状態: 部分。依存: E08。
- 対象: 既存 role projection、conversation_inputs、context generation 接続。
- 実装: root 初回入力+採用 amendments を順序付き Must として保持。毎 step の scope/epoch/source digest と generation 来歴を検査。role に必要な output だけ渡す。
- 失敗時: scope widening 拒否、必須失効は再取得1回、それでも失敗なら停止。空 projection で条件欠落を隠さない。
- 最小試験: `rr_07_amendment_present_once`、`rr_07_scope_no_widening`、`rr_07_revoked_source`、既存 context regression。
- 完了: 最新条件が欠けた envelope は dispatch 0、旧非routing fixture 不変。

#### E10 通常 turn と Provider/Sink 接続 — RR-09/12/14

- 状態: 部分。依存: E08/E09。
- 対象: conversation_turn、conversation_inputs_roles、新規 Provider adapter/Sink。
- 実装: enabled 時の通常 turn を executor へ接続。保存済み decision を読む。delta は内部 buffer、外部は本文なし activity、最終採用後だけ回答通知。disabled は旧経路。
- 失敗時: partial/timeout は step failure。routing 障害を理由に旧 turn を自動再実行しない。
- 最小試験: `rr_05_normal_turn_two_steps`、`rr_09_partial_stream_no_answer`、`rr_09_child_delta_not_spoken`、A01。
- 完了: 実受付/実 writer/通常入口から複数 step が動く。関数単体の呼出列だけでは不可。

#### E11 hard filter・予算・resourceGroup — RR-06/09/22

- 状態: 部分。依存: E10。
- 対象: selection、limits、新規 budget、driver。
- 実装: 実 host facts で capability/location/cloud/cost を検査。費用予約+実 usage、switch/review/step/root 上限を累積。resourceGroup 競合では reasoner 優先。
- 失敗時: 費用不明を0にしない。設定上限があり価格不明なら起動拒否。利用不可時は新 decision を記録し許可済み fallback のみ。
- 最小試験: `rr_22_cloud_revoked_before_dispatch`、`rr_22_loop_budget`、`rr_09_shared_resource_group`、A07/A25/A28。
- 完了: dispatch 前撤回と全予算超過の開始0、理由を DB から説明可能。

### P3 tool と SDK

#### E12 MCP session の実行束縛 — RR-11/21

- 状態: 部分。依存: E03/E08。
- 対象: mcp_server sessions/router/context/calls、gateway の routing 入口。
- 実装: root/step/revision/attempt/epoch を host が session に保持。root-only query から現在 step を推定する経路を routing 用には廃止。session 不明・失効は拒否。
- 失敗時: generic 非routing MCP の既存意味を変えない。旧 session は新 step に移送せず失効。
- 最小試験: `rr_21_old_session_cannot_use_new_step`、`rr_21_missing_step_denied`、既存 MCP regression。
- 完了: revision 切替直後に旧 sidecar が呼んでも新 step の tool link が増えない。

#### E13 共通 permit と tool budget — RR-10/21/22

- 状態: 部分。依存: E11/E12。
- 対象: routing tools/budget、gateway、既存 invocation owner の開始境界。
- 実装: offered tools、解決先副作用分類、role、scope、取消、実行束縛を二段検査。root/step tool budget を原子的に予約。read-only unknown は拒否。
- 失敗時: permit 不明・予算不足・DB failure は外部実行0。開始引受後の取消は owner settle 待ち。
- 最小試験: `rr_10_reviewer_resolved_mutation_denied`、`rr_21_tool_budget`、`rr_10_update_between_reserve_and_invoke`。
- 完了: Provider/SDK/specialist が同じ検査を使い、名前 prefix に依存しない。

#### E14 operation idempotency と settle — RR-11/17

- 状態: 部分。依存: E13。
- 対象: tool_ledger、gateway、owner receipt 接続。
- 実装: root を含む一意 ID、論理 operationId、canonical payload digest、owner invocation/resultRef 対応。確定結果と unknown を区別。settle failure を呼出元へ返す。
- 失敗時: caller detach でも owner を追跡。unknown は次 mutation/retry を遮断し unresolved と表示。取消要求だけでリンクを settled にしない。
- 最小試験: `rr_11_same_payload_different_roots`、`rr_11_duplicate_operation_once`、`rr_11_detached_owner_settles`、`rr_11_settle_failure_blocks_continuation`。
- 完了: A09/A17/A18 の実 gateway 経路で mutation 回数と DB 整合性を確認。

#### E15 sidecar protocol と隔離 env — RR-19/20/21

- 状態: 部分。依存: E12。
- 対象: codex-sidecar.ts、codex-isolation.ts、adapters/codex*.rs、sidecar tests。
- 実装: bridge token の SDK 子 env への最小伝播、固定 error code、frame byte/unknown field/terminal 検証。host 側で用途別 output schema を検証する境界を用意。許可 gateway 以外の設定を渡さない。
- 失敗時: protocol 違反は failed、子ツリー回収。例外本文や stderr を無加工保存しない。
- 最小試験: `rr_21_sdk_child_receives_token_without_recording`、`rr_19_wrong_step_terminal`、`rr_20_config_isolation`、秘密文字列の非記録 fixture。
- 完了: mock SDK constructor の引数だけでなく子プロセス環境まで検査。実隔離の合格は L01 に残す。

#### E16 SDK adapter の executor 接続 — RR-19/21

- 状態: 部分。依存: E10/E14/E15。
- 対象: conversation_role_codex の責務抽出、adapters/codex、driver。
- 実装: StepRequest→SDK→StepResult。SDK が直接最終 message を保存しない。新 step/revision/model/fingerprint は new thread、同 step の host MCP roundtrip は同じ束縛で処理。
- 失敗時: 認証/能力/容量エラーを分類し、許可された fallback だけ新 decision で開始。unknown tool を理由に別 actor で再実行しない。
- 最小試験: `rr_21_sol_tool_roundtrip`、`rr_21_changed_revision_new_thread`、`rr_21_changed_model_new_thread`、A29/A30 offline。
- 完了: 通常 turn→mock SDK→実 gateway→回答候補→最終採用の縦通し。個人 thread 操作0。

### P4 review/revise

#### E17 review schema と host verification — RR-24

- 状態: 部分。依存: E09/E15。
- 対象: review、repository の review 保存、新規 verification 型。
- 実装: target output/author step/evidence manifest を明示。issue 最大8、byte 上限、verdict/severity/claim/unresolved を検査。モデル verdict と host verification を別列・型にする。
- 失敗時: 虚偽ref、revoked、別 revision の無許可 evidence を拒否。実在refだけでは verified にしない。
- 最小試験: `rr_24_model_verified_does_not_authorize_revision`、`rr_24_review_target_is_exact_draft`、`rr_24_false_evidence`。
- 完了: 検証器版と証拠を持つ host 判定だけが revision 許可に使われる。

#### E18 review→revise executor — RR-24/25

- 状態: 未着手。依存: E11/E13/E16/E17。
- 対象: recipe 分岐、revision decision 保存、reviewer/reviser prompt。
- 実装: 独立 reviewer を read-only で実行。verified と仮説を分けて author へ渡す。decision 消費と revise step 作成を一 transaction にする。issue ごとの採否・根拠を残す。
- 失敗時: 不正reviewは修正許可にしない。根拠取得は上限内1回、改善不能は unresolved/clarify/proposal。根拠なし結論反転を採用しない。
- 最小試験: `rr_25_normal_turn_author_review_revise`、`rr_25_decision_consumed_once`、`rr_25_review_round_limit`。
- 完了: S→Q→S 順、最終回答1件、中間発話0。必須 review 失敗を検証成功と表示しない。

#### E19 feedback と challenge 新 root — RR-08/23

- 状態: 部分。依存: E05/E18。
- 対象: signals/classifier、feedback repository、通常入力 routing。
- 実装: target answer を同会話の一意回答へ解決。explicit positive/negative と challenge を分離し、challenge は新 root から旧回答を参照。
- 失敗時: 引用、否定、対象曖昧は clarify/reject。沈黙や「本当に？」を自動低評価にしない。
- 最小試験: `rr_23_challenge_starts_new_root`、`rr_23_wrong_conversation`、`rr_23_quoted_negative`、A21〜A24。
- 完了: completed root の再開0、通常入力から独立再考・reviewへ到達。

### P5 premium/specialist

#### E20 proposal 束縛と一度だけの消費 — RR-26

- 状態: 部分。依存: E07/E11/E18。
- 対象: proposals、schema、executor の premium 分岐。
- 実装: proposal を root/revision/policy/recipe candidate/actor-model fingerprint/送信先/価格版/期限に束縛。approved と consumed を区別し、消費と step 作成を同時 commit。
- 失敗時: stale/decline/期限切れは起動0。dispatch 直前に capability/cloud/cost/scope を再検査。対象変更は再提案。
- 最小試験: `rr_26_approval_consumed_once`、`rr_26_candidate_changed_requires_new_proposal`、`rr_26_approval_then_cloud_revoked`。
- 完了: 提案0回、有効承諾1回、二重承諾でも増加0。費用不明は unknown。

#### E21 proposal UI/IPC と reload — RR-26/27/28

- 状態: 未着手。依存: E20。
- 対象: routing IPC、command_registry、生成 bindings、roleRoutingApi/types、chat 提案UI。
- 実装: DB snapshot に提案と状態を追加。UI は proposal ID を送信、host が候補を復元。承諾/辞退/失効/利用不可を表示。単独「はい」を承諾にしない。
- 失敗時: reload/再送/切断で二重 dispatch しない。IPC error を承諾成功として表示しない。
- 最小試験: `rr_26_approval_reload_dispatch_once`、frontend stale proposal、A26〜A28。
- 完了: UI→IPC→writer→executor を通った offline 起動回数を確認。bindings 手編集0。

#### E22 specialist から親への復帰 — RR-38

- 状態: 未着手（wrapper は部分）。依存: E14/E16。
- 対象: tool_specialist、recipe/driver、specialist prompt。
- 実装: 親の目的と offered tools を渡し、typed request のみ受理。host 実行後は結果を親へ戻す。親より広い権限を与えず step/tool 予算を消費。
- 失敗時: 無効なら親の直接経路。失敗 fallback は未実行が確定した場合のみ。unknown は親の再実行も禁止。
- 最小試験: `rr_38_normal_turn_specialist_returns_to_parent`、`rr_38_swap_actor_preserves_permissions`、`rr_38_unknown_blocks_parent_retry`。
- 完了: A41、specialist 最終採用0、差替えで ACL/ledger/speech owner 不変。実 Needle3 評価を mock 合格に含めない。

### P6 会話・音声・競合

#### E23 構造化分類と frontend slot — RR-08/15/16

- 状態: 部分。依存: E05/E09/E11/E19。
- 対象: classifier、role prompt、frontend adapter、reaction fixtures。
- 実装: kind/evidence span/target/confidence/replyKey を検証。status/social で現 revision を保ち、amendment は条件追加、unclear は barrier 維持。共有 resourceGroup では定型受付へ。
- 失敗時: frontend failure は reasoner を落とさない。挨拶混じりの実質依頼を簡易完了しない。classifier に承諾/認可権限を与えない。
- 最小試験: `rr_08_mixed_greeting`、`rr_08_bad_target`、`rr_08_timeout_unclear`、A04〜A07/A14/A15。
- 完了: 通常入力→分類→barrier 解決が DB 状態と一致し、旧結果の保留/採用を説明可能。

#### E24 永続 speech owner と epoch — RR-13/14/17

- 状態: 部分。依存: E04/E05/E23。
- 対象: speech_queue、新規 speech repository/driver、既存 voice response 接続。
- 実装: speech intent/lifecycle、speechId/epoch、ack/progress/final 優先順位、author_verbatim を接続。barge-in、合成完了、再生直前、停止完了を検査。
- 失敗時: 停止未確認なら次再生0、音声失敗でも画面回答保持。completed root の旧音声も次入力で停止できる。
- 最小試験: `rr_13_late_synthesis_not_played`、`rr_29_old_speech_end_new_owner`、A11/A12。
- 完了: 同時再生<=1、古い epoch 再生0、開始/終了/取消を DB から追跡可能。

#### E25 queue・restart・replay と ASR receipt — RR-14/18/27

- 状態: 部分。依存: E06/E21/E24。
- 対象: recovery、IPC replay/snapshot、frontend store、ASR 受付接続。
- 実装: FIFO/queued cancel、startup interrupted、明示再開UX、snapshot+seq replay。ASR は receipt 時に queue を解放。同 sourceId の再送を重複受付しない。
- 失敗時: UI切断は cancel ではない。restart で tool/音声の自動再実行0。live/replay 重複を seq で除去。
- 最小試験: `rr_18_restart_no_replay`、`rr_14_subscribe_replay_race`、`rr_14_asr_duplicate_receipt`、A19/A20。
- 完了: 再接続で永続状態が復元され、frontend-only 状態が正本にならない。

#### E26 実経路 Barrier 競合マトリクス — RR-29

- 状態: 未着手（pure reducer fixture のみあり）。依存: E14/E18/E21/E24/E25。
- 対象: 新規 routing integration fixture。実 writer/通常入力/coordinator/gateway/voice driver を使用。
- 実装: 受付→分類保留→Provider完了、採用→新入力→旧TTS、reserve→update→invoke、dispatch→update→settle、cancelと完了の両順序、旧SpeechEnded、replay/live競合を個別 test 化。
- 失敗時: sleep・乱数で順序を偶然作らない。oneshot/Barrier で固定し、timeout は試験停止用だけに使う。
- 最小試験: `rr_29_provider_before_classifier`、`rr_29_tool_permit_update_before_invoke`、`rr_29_tool_dispatch_update_then_settle`、`rr_29_answer_commit_input_then_tts`、`rr_29_cancel_completion_both_orders`。
- 完了: root/revision/step/attempt/eventSeq/operationId/speechEpoch と外部呼出回数を assert。A13〜A20/A27/A30 を網羅。

#### E27 mock ASR→tool→TTS E2E — RR-15/29

- 状態: 未着手。依存: E22/E26。
- 対象: offline E2E runner、音声/UI fixture。
- 実装: mock ASR final→receipt→frontend→author→tool→必要なreview/revise→final→TTS→UI reload。TTS/tool/Provider failure も注入。
- 失敗時: 成功した部分だけを E2E 成功としない。tool unknown は final 成功へ進めない。
- 最小試験: `rr_15_asr_tool_tts_reconnect_e2e`、`rr_15_e2e_tts_failure_keeps_answer`。
- 完了: A12 を通常アプリ境界で満たす。実音声の性能/体感は L03 に分離。

### P7 学習と運用

#### E28 immutable features と学習 schema — RR-30

- 状態: 部分。依存: E11/E19/E26。
- 対象: learning/schema、features、decision 保存。
- 実装: decision 時点の feature/候補/actor版/source lineage を固定。後の feedback は label にのみ反映。nullable quality を維持。
- 失敗時: 最新 World/設定で過去 feature を再生成しない。不足情報は unknown。
- 最小試験: `rr_30_feature_snapshot_immutable`、`rr_30_nullable_quality`、A31。
- 完了: 後続イベント追加後も過去 decision を同じ特徴で再現可能。

#### E29 増分 extraction と checkpoint — RR-31

- 状態: 部分。依存: E28。
- 対象: learning repository、新規 extract/jobs。
- 実装: 安定した全体 cursor または root+seq の明示 cursor を定義する。root-local seq の単純 MAX を全体時系列に流用しない。upper bound 固定、page と cursor 同時 commit、dirty 再処理。
- 失敗時: page 保存失敗で cursor 前進0。restart で重複/欠落0。
- 最小試験: `rr_31_crash_before_checkpoint`、`rr_31_same_page_twice`、`rr_31_multiple_roots_cursor`、A32/A33。
- 完了: 境界後に更新された row を境界内の値として読まない。

#### E30 label と conflict — RR-32

- 状態: 部分。依存: E17/E19/E29。
- 対象: learning labeler/repository。
- 実装: explicit/verified/model_inferred を分離。cancel/superseded/unknown/conflict を二値成功失敗へ潰さない。後日訂正は labelRevision。
- 失敗時: reviewer 意見、HTTP成功、沈黙だけを正答ラベルにしない。
- 最小試験: `rr_32_silence_not_success`、`rr_32_cancel_not_failure`、`rr_32_conflict_excluded`、A34。
- 完了: 再処理で旧 dataset を変更せず、新版へ正しく反映。

#### E31 dataset と atomic export — RR-33

- 状態: 部分。依存: E30。
- 対象: learning/export、新規 dataset 組立、job 接続。
- 実装: conversation/root lineage/fixture 派生の group split、時系列境界、本文なしJSONL、hash、manifest 最後の rename、DB ready 条件。
- 失敗時: .tmp、manifestのみ、hash不一致を ready にしない。file rename後DB失敗は再開で照合。
- 最小試験: `rr_33_group_split`、`rr_33_no_future_features`、`rr_33_partial_file_not_ready`、A35/A36。
- 完了: 同 source版の digest 一致、train/eval 重複0、本文/token export0。

#### E32 scheduler と foreground pause — RR-34

- 状態: 部分。依存: E29/E31。
- 対象: learning/scheduler/runner、既存 setup/shutdown/foreground 接続。
- 実装: FakeClock の wall/monotonic 分離、night/idle/missed/DST/rollback、日次job一意、manual tickも同gate。foregroundでpage境界pause、local labeler cancel。
- 失敗時: skipped/paused を completed にしない。第2 DB writer、外部cronを追加しない。
- 最小試験: `rr_34_missed_night`、`rr_34_clock_rollback`、`rr_34_foreground_preempts`、A37。
- 完了: 会話開始がnightly推論slot解放待ちで止まらない。負荷測定は L04。

#### E33 shadow と artifact 境界 — RR-35/36

- 状態: 部分。依存: E28/E30/E31。
- 対象: learning artifact/evaluate/statistics/shadow、routing ranker 接続。
- 実装: 最低20 eligible例、欠損cost、cold start、hash/feature/candidate検証。shadowは実dispatch不変。既存 adaptive 機能とrouting対象を区別し、自動昇格を追加しない。
- 失敗時: 無効artifactはrules、未選択候補の成果を捏造しない。offline replay でtoolを再実行しない。
- 最小試験: `rr_35_small_sample_rules`、`rr_35_shadow_no_second_call`、`rr_36_no_counterfactual_labels`、A38/A39/A42。
- 完了: shadow on/offで実adapter回数一致、unsupported/insufficient_dataを報告。

#### E34 失効 journal と診断UI — RR-28/37

- 状態: 部分。依存: E25/E31/E33。
- 対象: learning/invalidation、既存forget/delete hook、routing snapshot/設定UI。
- 実装: source削除とDB artifact失効を同transaction、file cleanup journal/retry、cache世代検査。担当・理由・latency・未実施・unknown費用・学習状態を本文なしで表示。
- 失敗時: file削除失敗でもDB artifactは利用不可。errorにtoken/path/本文を含めない。
- 最小試験: `rr_37_forget_invalidates_before_dispatch`、`rr_37_cleanup_retry`、`rr_28_redacted_diagnostics`、A40。
- 完了: 元source失効後の採点0、UI再読込で状態一致。必要なら E34a失効/E34b UI に分割。

### P8 全体 gate

#### E35 offline 受入マトリクス — RR-39

- 状態: 未着手。依存: E00〜E34。
- 対象: acceptance-matrix、offline integration suite、results。
- 実装: A01〜A42の各条件を実testへ対応。成功だけでなく禁止されたdispatch/採用/発話の0回を検査。V1〜V4対象回帰を実行。
- 失敗時: 欠けた受入を他test名で代用しない。各失敗を対応Eへ戻す。
- 最小検証: A全42行に実test/command/件数/結果/HEAD。0 tests拒否。
- 完了: 全offline観点合格。live必要行は別セルを未検証として残す。

#### E36 size・全体 regression・rollback — RR-39

- 状態: 部分。依存: E35。
- 対象: 変更対象 module、証跡、個別に必要なsize登録。
- 実装: V5、desktop smoke、P1 mock測定。routing無効化→新規受付停止→子cancel→副作用drain→旧経路再開を確認。DB downgradeしない。
- 失敗時: 既存並行変更の障害は切り分け、全体合格と書かない。自変更のsize違反は分割して直す。baseline一括緩和禁止。
- 最小試験: `rr_39_disable_drains_before_legacy_resume`、A01再回帰、P1 warm5+100。
- 完了: 自変更の新規違反0、全体commandの結果と外因が説明可能。全体failが残ればRR-39は部分。

#### E37 最終報告 — RR-00〜39

- 状態: 未着手。依存: E36、必要なL01〜L04。
- 対象: work-cards、results、acceptance-matrix、利用/rollback手順。
- 実装: RR→E/L→A/P→test→証跡を照合。利用可能/unsupported actor、live許可範囲、sample、失敗数、既知遅延、未実施を明示。
- 失敗時: live未許可なら「offline完了・全体未完」として残す。学習基盤完了をML品質改善と表現しない。
- 最小検証: RR40/A42/P5対応、local link、記載command、git diff --check。
- 完了: 指定受入と性能gateの証跡が揃い、部分実装を完了に数えていない。

## 6. 明示許可が必要な live lane

以下は準備だけなら offline で進めてよい。**実認証モデルの起動は、この文書を渡すだけでは許可されない。** モデルID、認証利用、試験fixture、回数上限、費用上限または費用不明時の扱いを示し、ユーザーの明示許可を得た lane だけ実行する。許可は他モデル・無制限試行へ拡張しない。

### L01 Sol・SDK隔離 — RR-19/20/21/29

- 状態: 未着手。依存: E16/E26。認証済みSDKと許可modelが必要。
- 実行: 空の一時profile/cwd、限定host tool、固定SDK/CLI。実roundtrip、変更revision/modelでnew thread、継承MCP/native tool/任意ネットワーク拒否を確認。既存個人threadを使わない。
- 試験: `rr_live_20_sdk_isolation`、`rr_live_21_tool_roundtrip`。隔離fixtureの禁止先は外部サービスではなく管理された拒否検出先を使う。
- 失敗時: actor unsupported。設定を緩和せず原因を記録。
- 完了: 実機の禁止行為0、許可gateway呼出成功、秘密非記録。mock成功とは別証跡。

### L02 Astra承諾dispatch — RR-26/29

- 状態: 未着手。依存: E21/L01。Astraの独立許可と予算が必要。
- 実行: 提案のみ、辞退、期限切れ、承諾、二重承諾、dispatch前撤回。製品内proposal承諾も省略しない。
- 試験: `rr_live_26_approved_dispatch_once`。
- 失敗時: 課金/利用不可を報告し、別modelへ黙って置換しない。
- 完了: 有効承諾で1回、その他0回、実usageと見積差を記録。

### L03 音声・分類・品質・遅延 — RR-08/15/29/39

- 状態: 未着手。依存: E27/E35/L01。使用する全modelと音声処理の実行許可が必要。
- 実行: 固定日本語分類100件、音声30件以上、品質100件。baselineと同pool/温度/音声設定で交互測定。CPU/GPU、同居model、warm/cold、sampleを記録。
- 試験/指標: A12のlive、P2 TTFA p95<=1500ms、P3同actor追加p95<=max(1000ms,baseline p95の10%)、P4成功率差と区間。分類の誤cancel/誤承諾/実質依頼の誤簡易完了0。
- 失敗時: clarify閾値を下げて安全gateを通さない。最終回答遅延をackの速さで隠さない。P4差の下限<-2ptは導入保留。
- 完了: 品質を断定できない標本数・区間も明示し、受入仕様のP2〜P4を満たす。

### L04 夜間負荷と会話優先 — RR-34/39

- 状態: 未着手。依存: E32/E34/L03。モデル利用があれば別途その範囲の許可が必要。
- 実行: page100件、実機30件、途中foreground開始。原データは隔離fixture。
- 指標: P5 DB write p95<=50ms、cancel要求<=100msは仮想時計で検査、実機TTFA p95悪化<=10%。P1はE36の結果を併記。
- 失敗時: nightly pause。会話側の期限・安全条件を緩和しない。
- 完了: foreground優先、再開整合性、負荷目標を満たす。

## 7. 検証と証跡の運用

### 各単位の標準記録

```text
unit: E番号（枝番があれば含める）
RR / A / P:
status: 未着手 | 部分 | 完了
HEAD / 既存dirty / 今回変更ファイル:
入口と期待するdispatch列:
test名 / command / lane / 実行件数 / 結果:
DB件数・eventSeq・operationId・speech開始数:
未実施理由 / 次の依存単位:
```

本文・prompt・credentialを証跡に貼らない。実装用 fixture は架空データにする。進捗は既存 `spec/evidence/role-routing/progress.md` へ追記し、全体結果は `results.md`、対応は `acceptance-matrix.md` に保存する。

### command

実行前に現在のpackage/scriptと試験名の存在を確認する。以下は既存V1〜V6に合わせた基準。

```sh
cargo test --locked --manifest-path src-tauri/Cargo.toml --lib rr_NN_
cargo fmt --check --manifest-path src-tauri/Cargo.toml
cargo test --locked --manifest-path src-tauri/Cargo.toml --lib runtime::context
cargo test --locked --manifest-path src-tauri/Cargo.toml --lib providers::chat_completions
cargo test --locked --manifest-path src-tauri/Cargo.toml --lib tool_selection
cargo test --locked --manifest-path src-tauri/Cargo.toml --test sqlite_architecture
bun run typecheck
bun test ./tests/role-routing-codex.test.ts
bun run ipc:check
bun run s11tnext:check
bun run size:check
git diff --check
```

IPC/prompt変更時は正規の生成commandを実行して生成物を確認する。新規 frontend/session tests は作成後に対象commandへ追加する。`bun run size:register` は全未登録を拾うため無条件実行しない。追加ファイルの登録が必要なら現行scriptの仕様を読み、対象だけの差分をレビューする。既存上限の引上げで通過させない。

E36で `bun run check:local`、`bun run test:rust-packages`、`bun run desktop:smoke` を実行する。live fixture は ignored かつ明示 opt-in で通常suiteから隔離する。許可後も対象を絞り、包括的なignored test起動で許可外モデルを呼ばない。

文書だけを保存する段階ではランタイム試験は不要。local link、IDの欠落・重複、依存先、diffを検査する。

## 8. 最初のコミットと作業継続

E00/E01 の基準・契約を整えた後、最初の実装コミットは次の3つにする。依頼がない限り自動でcommitする必要はなく、差分の切り方として使う。

1. **E02: stale結果の採用拒否。** Provider/SDK両方の採用引数とtransaction検査。旧revision/保留/取消/DB失敗の実保存試験。
2. **E03/E04: step完了とroot完了の分離。** 5ファイルを超えるなら2a schema/claim、2b output/finalizeへ分ける。2 stepの台帳、二重terminal、usage帰属を検証。
3. **E07/E08/E10の最小縦通し。** 各責務を3a compiler、3b driver、3c通常入口に分割してよい。E05/E06/E09などの依存は先に満たす。fake複数stepと既存単一actor回帰まで。未完成の外部tool/review/premiumを有効化しない。

「3コミットで全executorを詰め込む」ことより、各差分がレビューできて合格条件を満たすことを優先する。以後はE番号を単位に継続する。

### Terra に渡す依頼文

```text
SAAA の spec/docs/saaa-role-routing-completion-roadmap.md を読み、
E00から依存順に role routing を完成させてください。
これは実装依頼です。既存の計画・契約・証跡・コードを確認し、
完了済みを再実装せず、不足部分を通常turnの実経路まで接続してください。

各単位は対象を絞り、正常・失敗・更新・取消・競合の指定試験を通してから
進捗を記録してください。型と保存関数だけで完了にしないでください。
5実装ファイルを超える単位は枝番に分け、安全条件を維持してください。
offline laneは個別の確認待ちにせず進めてください。
認証済みモデルを起動するlive laneは、実行内容・モデル・回数・予算を
提示して明示許可を得るまで実行せず、独立したoffline作業を続けてください。

security boundary、既存tool owner、scope、policy snapshotを維持し、
tokenや会話本文を追加台帳・ログ・証跡へ漏らさないでください。
role-routing外のdirty変更を上書き/取り込み/revertせず、
size baselineを一括登録・一括緩和しないでください。
既存の別Codexタスクへメッセージを送らないでください。
live未検証や全体gate失敗が残る場合、全体完成とは報告しないでください。
```

この依頼文は保存用であり、今回 Terra や別タスクへ送信したものではない。
