# 委任仕事の残件修正・統合受入計画（Terra向け）

作成日: 2026-09-21。状態: 未着手。担当想定: Terra。対象: 自然な依頼から実行・検証・報告まで続く委任仕事。

本書は[元の実装計画](saaa-delegated-work-completion-plan.md)の未達部分を具体化する後続計画である。完成条件を縮小せず、独立監査 `spec/evidence/delegated-work/independent-audit-2026-09-21.md` の A01〜A12 を修正・検証する。「実UIだけが残件」という旧判定を引き継がない。番号・API名・試験名のうち「新規」と記したものは実装予定であり、現在存在する証跡ではない。

## 1. 完成条件と作業範囲

ユーザーが自然文で対象を指定して仕事を任せると、確認済み委任の範囲で一度受け付け、別の話題・別会話へ移っても仕事が進む。A/Bのどちらだけを撤回しても他方は残る。実行枠の競合は自動解消し、結果と検証根拠を保存してChatへ届ける。判断待ちも黙って滞留しない。hold解除、設定変更、forget、Scope revoke、sleep/restartを通して権限・実行・報告が一致する。

既存のCoding service、Pi runner、SDK profile、SQLite writer、Schedule loop、Situation、TTSを使う。第二のCoding Agent、別DB、別daemonは作らない。必要なschema・IPC・既存adapter拡張は本書の対象。World/role-routing/学習の独立改修や任意アプリ操作の製品機能追加は対象外。登録済みCapabilityも利用可能なら同じ実行契約へ接続し、未対応能力は具体的な判断待ちにする。

作業開始時はHEAD・dirty・対象ファイルhashを記録する。他作業の未コミット変更をreset/checkout/deleteしない。共有ファイルは編集直前に読み直す。他のCodexタスクへの送信や新規タスク作成は行わない。外部条件が必要な実受入を除き、カードごとのユーザー確認待ちにせず進める。

再利用する成果は、origin分離、single Coding slot、SQLite上のintent/予約/cursor/outbox、plan/dependency/replan、SDK read-onlyの実証、TTS終了・失敗callbackの永続化である。これらの部品を捨てて作り直さない。一方、`SqliteWriter::write`はtransactionではなく、fixture成功は一連の動作の証明でもない。

## 2. 先に固定する実装契約

### 2.1 正本と状態

| 対象 | 決定 |
| --- | --- |
| Goalと委任 | 権限statusと仕事の進捗を分離する。既存Goal/Delegationのactive/paused/withdrawnは権限として保持し、新規 `steward_goal_progress` を仕事の進捗の正本とする。active上限8件は未完了Goalを数える |
| Task | queued / dispatching / running / awaiting_dependency / awaiting_user / verifying / done / failed / cancelled / outcome_unknownを型で表す。既存loop_stateのCHECK制約は制御されたmigrationで拡張する。影の状態列を二つ書き続けない |
| 意味 | Coding settledは実行終端。Task doneはそのstepのverifier成功。Goal doneは採用済みplanの必須stepとGoal verifierが成功した状態。awaiting_userを依存成功とみなさない |
| unknown | 送信した可能性があり相手の結果を特定できない状態。自動再送・成功扱い・新しい後続stepを禁止。既存Coding recoveryで照合し、必要なユーザー判断を報告する |
| 状態の更新 | writer transaction内の共通transition関数だけで進める。list/statusは読取り専用。個別TaskのGoal/Delegationを検査し、latest_workを全Taskへ適用しない |
| source | 実在source ID/version、会話、引用の文字範囲、Scope key/epoch、Goal revision、Delegation revisionを束縛する。UIの明示登録は専用のユーザー操作receiptをsourceとし、架空のChat発言を作らない |
| 成果物 | job/run、対象、recipe revision、終了code、失敗一覧または取得済みログ、生成元、content digest、verifier結果を参照で結ぶ。モデルの「成功しました」だけを判定根拠にしない |

### 2.2 自然文と権限

モデルの提案と権限の登録を分離する。hostはcurrent runからsourceを確定し、モデルにsourceの差替えや自分で確認済みにする操作を許さない。新しい権限や既存権限の拡大は、対象・操作・予算・期限・完了条件を提示してUIで一度確認する。既存の確認済み委任に包含される依頼は追加の権限確認を要求しない。

自然文の意味判断は、sourceに束縛した `request / status / withdraw / reject / clarify` の判定結果として保存する。生のtool callをその判定結果にしない。hostが引用範囲・source版・明示対象・既存委任への包含を検証できないときは確認に戻す。否定/引用/仮定を単語の有無だけで許可する実装を避ける。意味判定がモデルを使う場合も新規権限の正本にはせず、権限検査と分離して30ケース以上で挙動を測る。複数解釈が残る依頼を勝手に採用しない。

sourceと操作が同じでも「対象テストX」と「対象テストY」は別仕事になり得る。dedupeには正規化した対象・verifier・委任参照を含める。同じ提案の再配送は同じ結果を返す。上限確認より先に既存idempotency keyを照合し、8件目の再送を9件目として拒否しない。

UIがなくても新権限を採用する逃げ道は作らない。一方、確認済みgrantの継続に毎回確認を追加する逃げ道も作らない。hostが検証するsourceの構造・引用と、自然文意味の判定精度は別々に試験し、任意自然文の完全な意味理解を実装済みとは主張しない。

### 2.3 受付・実行・再送

受付transactionはGoal/progress・委任参照・plan/steps・初回Task・予約・pending intent・source bindingをまとめる。確認待ち提案は実行可能Taskを作らない。既存権限内の受付とUIで確認した新規受付は同じdomain serviceへ収束させる。

dispatchの線形化点では現行source/Scope/委任版・設定・profile・予算・single slotを再検査する。Coding job/run/origin、Taskとのbinding、local receiptを同一transactionで確定してからrunnerを起動する。外部のPi受領状態とローカルCoding受付receiptを混同しない。preflight/probe等の外部I/Oの後にもcommit直前の再検査を行う。

| dispatch結果 | durable状態と処置 |
| --- | --- |
| busy / foreground優先 / 設定OFF | 未送信を確認できるときはpendingを保持して理由とnext_eligible_atを保存。slot解放/設定ON/foreground終了でwake。予算を消費しない |
| scope/委任失効・対象不正 | cancelledまたはawaiting_user。未消費予約を解放。後続候補を妨げない |
| local accepted | 同じTaskには同じjob/runを返す。別jobを作らない。runner送信は既存Coding delivery契約で管理 |
| 永続的な実行失敗 | failed。採用planが許す場合だけ有限replan。自動repairや権限拡大はしない |
| 送信結果不明 | outcome_unknown。予約を安易に解放せず、重複効果を防ぐため既存所有照合へ送る |

一つのGoalのretryが古いrow順を占有し続けないよう、dispatch可能な候補を会話横断で選ぶ。同じ資格の候補はdurable enqueue順、再試行はnext_eligible_atで待機させる。busyはLLMを呼ばず、無限高速retryもしない。

### 2.4 実行境界・予算・結果

readとtest_runに同じ汎用shell権限を渡さない。readはworkspaceに限定したread/list/search等、test_runはユーザーが確認登録したrecipe ID/revisionからhostが選ぶ固定argvだけとする。recipeは対象、cwd、argv、環境変数許可、入力版、出力先、network、timeoutを持つ。shell文字列をモデルから受け取らない。read用SDKの汎用shellをtool集合で制限できなければ、そのprofileは厳密read/testの対応済みにせず、既存Pi拡張点でscoped tool adapterを実装する。

最低一つの実read経路と一つの実test recipeを完成させる。testは破棄可能copyまたは明示した出力先で行い、元workspaceへのwriteとnetworkを境界で拒否する。使用profile・実行可能operationの対応表をUIにも返し、実行開始後に初めてunsupportedと分かる構成にしない。

budget_runsは外部実行が受理されたか受理不明の試行に対して一度だけ消費し、未送信失敗は解放する。budget_msはGoal配下の実行時間の累積上限とし、queue/hold待ちを除く。開始時に残量からdeadlineを決め、単一runも残量で停止する。awakeではmonotonic clock、再open時は保存時刻と所有状態で保守的に照合し、再起動で予算をリセットしない。有効期限はqueue待ちを含むwall clockで別管理する。

### 2.5 設定・配送の方針

| 条件 | 処置 |
| --- | --- |
| Memory OFF / Coding OFF | 新規受付実行・dispatch・継続stepを止める。runningにはdurable停止要求。未送信のqueueは保持し、ONで再検査。会話内の既存報告の閲覧とcancel/forgetは可能 |
| Schedule OFF | 新たな期限Actを止める。既に受け付けた一般の委任仕事と報告pumpは止めない |
| foreground応答中 | 新しい重い推論を開始しない。既存runの状態回収・停止・永続化は進める。結果は現在の応答へ混ぜず独立報告として扱う |
| Situation hold | 報告を保持し、解除時にGoalごとの最新状態を集約。次user turnを待たない |
| mute / auto-speak OFF / silent | 会話内配送は可能。音声はsuppressed/not_requestedとし、ONへ戻して過去の音声を一斉再生しない。以後の新規報告には現行設定を適用 |
| playback不明 | delivery_unknownを保存し自動再生しない。永続Chat報告は残す |
| withdraw / revoke | 新規効果を止め、必要な取消状況を本文最小の報告で通知。既に実行済み効果を取り消したと主張しない |
| forget | sourceに依存する未配送report・pending speech・派生本文・Goal summaryを失効/消去する。再接続・callback・hold解除で復活させない |

音声のexactly-onceは保証しない。会話messageの一度だけ挿入はDB transactionで保証する。通知の一意キーはTask/Goalの対象revisionとdestinationで固定し、再描画ごとに新revisionを作らない。

OFFで停止した送信済みrunは、ONだけで再実行しない。既存recoveryと結果/verifierを照合し、失敗・未知の扱いを適用する。自動復帰するのは未送信が確定したpending仕事だけである。

### 2.6 最小のIPC変更

以下は実装予定の名前。既存同等APIがあれば統合し、別名の二重経路を残さない。Rust DTOから型を生成し、JSONの自由なstatus文字列をfrontendごとに解釈しない。

| API | 入出力と所有カード |
| --- | --- |
| work_propose | current sourceから提案。返却をaccepted / requires_confirmation / clarify / rejectedで区別し、proposal/Goal/Task IDとreasonを返す。03/04 |
| work_confirm（新規） | UI専用。proposal ID・expected revision・表示内容digestを受け、保存proposalと照合して確認receiptを発行。model tool一覧には載せない。03/19b |
| register_steward_goal | 確認済み表示内容とstartModeを受ける。UI receiptをsourceとし、grant_onlyまたはstartへ進める。04/19b |
| list_steward_goals（新規） | Goal/progressを起点にTask/evidence/reportを返す読取り専用snapshot。cursor/revisionを返す。19a |
| work_resolve（新規） | expected revision付きのUI判断。user_confirmation_requiredの承認/否認、対象確定、照合再実行を型で区別する。tests_passの根拠不足を手動doneで隠さず、unknownも所有照合なしで解除しない。12/19b |
| recipe登録・照会（新規） | UIで確認した固定argv等をrevision/digest付きで保存し、modelにはIDと能力だけを渡す。10/19b |
| delegated-report-committed（新規event） | conversation/message ID、report revision、durable cursorのみ。再接続は読取りAPIで不足分を取得する。20 |

## 3. 監査・旧カードとの対応

| 監査 | 修正カード | 旧カードの再受入 |
| --- | --- | --- |
| A01 複数Goal再open | DWR-01、02b、24 | DW-02/14 |
| A02 自然文plan欠落 | DWR-03、04、21 | DW-03/10 |
| A03 busy/OFFから復帰不能 | DWR-06、07、14a、17 | DW-06/12/14 |
| A04 撤回状態の混線 | DWR-05、14b、22 | DW-02/13/14 |
| A05 sourceと権限の混同 | DWR-02a、03、04、19b、21 | DW-01/03/04 |
| A06 transaction欠落 | DWR-04、06、15、22 | DW-02/06/11 |
| A07 wake/Chat更新欠落 | DWR-16、17、20 | DW-08/11/14 |
| A08 verifier・結果不足 | DWR-10、12、13、18 | DW-10/11 |
| A09 profile/recipe/予算不足 | DWR-08〜11、24 | DW-07 |
| A10 OFF/forget/revoke | DWR-14a〜14c、18、22 | DW-12/13 |
| A11 未開始Goal非表示 | DWR-19a/19b | DW-04/14 |
| A12 receipt/bindingの順序 | DWR-06、07、22 | DW-05/06/08/10 |

旧DW-00のbaselineと部品の成功証跡は残す。DW-01〜13は上表の対象条件を再受入するまで部分完了または再検証待ち。DW-14は実装修正と実受入の両方が未完了とする。

## 4. 実施順と各カードの共通ルール

推奨順: DWR-00→01→02a→02b→03→04→05→06→07→08→09→10→11→12→13→14a→14b→14c→15→16→17→18→19a→19b→20→21→22→23→24→25→26。依存が満たされれば外部待ちカードを越えて独立作業を進めてよいが、未受入の安全境界を有効化しない。DWR-23の環境調査は00の後から先行可能。

各カードは概ね5実装ファイル以内を目安とし、テスト・生成物は別枠。広がる場合は枝番を追加して依存・合格条件を維持する。repository.rs/report.rs/tests.rsへ全処理を追記せず、以下の新規moduleへ責務ごとに抽出する。module宣言と生成bindingの機械的変更は実装ファイル数に数えない。schema/IPCはTerra内で単一所有し、別作業との競合は最新内容を読んで調整する。

カードごとに「既存経路で失敗する回帰試験→実装修正→対象試験→証跡」を残す。テストからproduction dispatcherを直接再実行したりDBにdoneを書き込んだりして、製品の自動継続を代行しない。fixtureは外部Pi/モデル応答とclock/fault点に限定し、受付・dispatch・state transition・outbox・IPCは本番関数を使う。

以下の `dw_rNN_*` とTS試験名は新規に作る具体的な試験名。行番号ではなく関数・責務を探索して編集する。Rust pathは `src-tauri/src/` 相対、TS等はrepository root相対。

### DWR-00: baselineと証跡の確定

- 依存: なし。対象: 既存plan、独立監査、`spec/evidence/delegated-work/`。
- 実施: HEAD/dirty/hash、既存テスト件数、profile/SDK/Pi/Xcode版、必要Context契約を採取。新規 `repair-progress.md` と `repair-results.md` に本書のカード欄を作る。旧resultsのSDK/TTS未完了記述は日付付き履歴に分ける。
- 検証: A01〜A12に対し「再現済み・コード確認・追加実験」の区別を保持。RC-C1〜C4の現行APIを確認し、DWの独自Context検査を増設しない。
- 完了: 全カードの未着手行と、既存成功の再利用範囲が記録されている。UI未接続でも後続のhost修正を開始する。

### DWR-01: 複数Goalで壊れない起動migration

- 依存: 00。対象: `steward/schema.rs`、`persistence/schema.rs`。試験: 新規 `steward/tests/migration.rs`。
- 実施: 旧one_active_goal indexを再作成しない移行に修正。必要なschema versionは着手時の最新番号から割当て、他作業の番号を上書きしない。旧rowとFK/索引を維持する。
- 試験: `dw_r01_reopen_two_and_eight_active_goals`、`dw_r01_upgrade_legacy_goal_and_job`。一時ファイルDBをclose/reopenし、実 `initialize_database` を2回以上通す。旧DB/現DB、0/1/2/8件、既存jobとreportを含める。
- 完了: Goal数・binding・reportを保ったまま再open。SQL抜粋だけの試験で代替しない。

### DWR-02a: authority・対象・結果の型契約

- 依存: 01。対象: `steward/contracts.rs`、新規 `steward/authority.rs`、`steward/execution_contracts.rs`。
- 実施: source binding、確認receipt、Scope/委任revision、対象path/recipe ID、expires_at、dispatch結果、実行結果、verifier outcomeを型化。PlanStepにrecipe/capability参照とverifier入力を持たせる。重複step ID、cycle、不明dependency、16 step超過、2 replan超過を拒否する。
- 試験: `dw_r02_contract_rejects_stale_or_ambiguous_bindings`、`dw_r02_plan_rejects_duplicate_step_ids`。source文字範囲はUnicodeの単位を固定し、日本語・絵文字でも同じ原文を参照する。
- 完了: 権限とモデルの自由文が別の型で表現され、未知状態・未対応能力をJSON文字列の暗黙分岐にしない。

### DWR-02b: 状態・binding・evidenceのschema移行

- 依存: 02a。対象: `steward/schema.rs`、新規 `steward/schema_execution.rs`、`coding/repository.rs`、`persistence/schema.rs`。
- 実施: §2.1のGoal progress、Task状態、source/authority binding、dispatch retry情報、recipe/evidence参照を永続化。Task表のCHECK変更はtransaction内のtable移行で行い、参照元・trigger・indexを再構成する。legacy行は元の権限範囲を広げず、結果根拠のないdoneを新たに検証済みと解釈しない。
- 試験: `dw_r02_migration_preserves_all_lineage_and_constraints`。全状態を含むfile DB再open、`foreign_key_check`、同じmigration再実行、途中failure rollback、認識外旧状態を含める。
- 完了: 再migrationでspeech/dispatch状態が不適切に巻き戻らず、既存job/Task/reportから追跡できる。

### DWR-03: sourceに束縛した受付判定と権限確認

- 依存: 02b。対象: `steward/tools.rs`、新規 `steward/intake.rs`、`steward/authority.rs`、`steward/contracts.rs`。
- 実施: current persisted runのsourceから依頼判定を保存し、work_proposeを既存grant内・新grant確認待ち・対象確認・拒否へ分岐。既存grantの対象/operation/残予算/期限を照合する。model toolから確認receiptを発行できないよう、UI経路と分ける。
- 試験: `dw_r03_quoted_negative_and_tool_sources_never_grant_authority`、`dw_r03_existing_grant_needs_no_repeat_confirmation`、`dw_r03_stale_confirmation_is_rejected`。悪いtool callをhostへ直接渡し、source差替え・引用不一致・期限切れ・旧revisionを拒否する。
- 完了: 新権限がmodel提案だけでactiveにならない。包含される明示依頼を毎回新たな権限確認へ戻さない。03が通る前に04の自然文実行を有効化しない。

### DWR-04: 全入口の原子的な受付とplan作成

- 依存: 03。対象: `steward/repository.rs`、新規 `steward/admission.rs`、`steward/commands.rs`、`steward/tools.rs`、`steward/reduce.rs`。
- 実施: model、UI、旧固定トリガを共通admissionへ接続。確認後の自然文はplan/初回stepまで作成してwakeする。UI直接登録は「登録して開始」と「委任だけ登録」を明示的に選べる契約にし、どちらも即一覧へ出す。sourceは実messageまたは明示UI操作receiptに束縛する。
- 試験: `dw_r04_proposal_creates_plan_task_reservation_and_intent_atomically`、`dw_r04_duplicate_at_goal_limit_returns_original_receipt`、`dw_r04_admission_fault_rolls_back_every_row`。途中SQL failureで部分Goal/予約を残さない。
- 完了: 自然文受付成功なのにTask0件の状態は「委任だけ登録」と明示された場合以外に存在しない。unsupportedは理由付きawaiting_userで表示できる。

### DWR-05: Goalごとの状態遷移と撤回の隔離

- 依存: 04。対象: `steward/repository.rs`、新規 `steward/transitions.rs`、`steward/driver.rs`。
- 実施: sync_from_codingとapply_terminal_eventを共通transitionへ収束。TaskごとのGoal/Delegation/revisionを検査。cancelled/withdrawnからrunning/doneへの逆戻りを禁止し、late resultは監査参照だけにする。
- 試験: `dw_r05_withdraw_either_goal_preserves_the_other`、`dw_r05_late_result_and_status_refresh_cannot_revive_work`。A/B両方向、queued/running/awaiting_user、list/status後まで確認。
- 完了: sibling状態・予約・reportに変化がなく、同じ終端eventの再適用が無害。

### DWR-06: dispatch受付・job binding・receiptの同時commit

- 依存: 05。対象: `coding/service.rs`、`coding/service_transactions.rs`、新規 `steward/dispatch.rs`、`steward/reduce.rs`、`runtime/pi/runner.rs`。
- 実施: A12の高速終端/受領前クラッシュを先に再現するfault pointを追加。委任再検査・CAS・slot・Coding job/run/origin・Task binding・local receiptを同時commit。runnerはcommit後のみ開始。user-turn側の既存受付を保持する。
- 試験: `dw_r06_instant_terminal_keeps_binding_artifact_and_successor`、`dw_r06_crash_between_local_receipt_and_spawn_does_not_duplicate`、`dw_r06_revoke_before_dispatch_creates_no_job`。本番service経由でfile DBとfixture processを使う。
- 完了: bindingなしの終端をcursorだけが消費しない。service内部のtransactionからwriterを再入してdeadlockしない。I/Oをwriter lock中に実行しない。

### DWR-07: busy再開・公平queue・restart照合

- 依存: 06。対象: `steward/dispatch.rs`、新規 `steward/queue.rs`、`steward/repository.rs`、`coding/recovery.rs`、`schedule/tick.rs`。
- 実施: §2.3の結果分類とnext_eligible_at。acceptedはoriginから同じreceiptを返し、busyより先にidempotent lookupする。unknownは既存recoveryで照合。期限切れ・無効・backoff中の先頭が他候補を塞がない。Schedule Startedはlocal receipt後のみ。
- 試験: `dw_r07_busy_then_slot_release_starts_sibling_without_service_retry`、`dw_r07_backoff_is_fair_across_conversations`、`dw_r07_unknown_never_resends`、`dw_r07_schedule_requires_current_delegation_and_receipt`。
- 完了: テスト自身がBのexecuteを再呼出しせず、共通dispatcherが開始する。架空/撤回済み/期限切れAct0、外部送信不明の自動retry0。

### DWR-08: 実profile能力を短い実験で確定

- 依存: 02a。対象: `runtime/pi/process.rs`、`scripts/pi-codex-sdk/index.ts`、`runtime/pi/tests.rs`、新規 `spec/evidence/delegated-work/profile-matrix.md`。
- 実施: pinned SDK/Piの実装と公式契約を調べ、built-in shell無効化、scoped read tool、登録recipe実行、network/filesystem/timeout制御の可否を小さい実験で記録する。promptによる自粛は能力として数えない。
- 分岐: SDKが必要境界を強制できれば既存拡張を使う。できなければread-only SDK profileの実証は保持し、厳密な委任操作は09/10のPi scoped tool/host recipe adapterへ接続する。新しいCoding Agentは作らない。
- 完了: profile×operationのsupported/unsupportedと理由、実装対象が確定。unsupported記録だけでDW-07を完了にしない。09/10で少なくとも一つずつ成立させる。

### DWR-09: scoped readの実行境界

- 依存: 06、08。対象: 新規 `runtime/pi/delegated_profile.rs`、`runtime/pi/process.rs`、`scripts/pi-codex-sdk/index.ts`、必要なら新規 `scripts/pi-delegated-tools/index.ts`。
- 実施: workspace内の限定read/list/searchだけを公開し、任意shell/test/write/networkは公開しない。canonical path・symlink脱出・外部readを実行時に拒否する。SDK state/authをモデル可読workspaceへ置く構成は見直し、trusted adapter状態/認証をscoped readから除外する。workspace内config/extension自動読込みで境界を変えさせない。
- 試験: `dw_r09_read_boundary_rejects_write_network_shell_and_escape`。外部marker、symlink、workspace内trusted state、追加MCP/tool、悪いmodel tool callを含める。認証SDK/Piの実read成功も別途実施する。
- 完了: 従来のread成功と拒否3種を再実行可能な試験へ保存。旧allow file-read* profileを狭いscope対応と誤表示しない。

### DWR-10: 登録test recipeと構造化結果

- 依存: 09。対象: 新規 `steward/recipes.rs`、新規 `runtime/pi/recipe_runner.rs`、`steward/execution_contracts.rs`、`runtime/pi/runner.rs`、`steward/commands.rs`。
- 実施: 明示登録済みrecipe revisionから固定argvを構築し、既存Coding run/slot/所有追跡を使って実行。test copy/input revision、許可出力先、env allowlist、network denyを適用。exit code・対象・失敗一覧またはparser失敗・log/artifact digestを保存。recipe内容が登録後に変わった場合は再検査する。
- 試験: `dw_r10_registered_recipe_produces_pass_and_fail_evidence`、`dw_r10_model_cannot_change_recipe_argv_or_output_scope`。小さい実test runnerをpass/fail両方で動かす。未知recipe、変更revision、元workspace write、子process network拒否を確認。
- 完了: 実test結果を取得でき、失敗exitをLLM成功文で上書きできない。recipe失敗とモデル通信失敗を区別する。

### DWR-11: 時間・回数・出力量の予算を実行へ適用

- 依存: 07、10。対象: 新規 `steward/budget.rs`、`steward/dispatch.rs`、`runtime/pi/runner.rs`、`runtime/pi/recipe_runner.rs`、`runtime/pi/process.rs`。
- 実施: §2.4の予約/消費/解放、残りdeadlineをrunへ渡す。timeout・cancelではprocess groupを停止し子孫の回収を確認。既存出力量上限を維持し、制御可能なCPU/memory/process上限と未対応platformをprofile表に記録する。
- 試験: `dw_r11_deadline_stops_running_process_and_children`、`dw_r11_restart_and_busy_do_not_reset_or_double_charge_budget`。短いsleep fixtureを小予算で停止し、受理不明時は予約が勝手に解放されないことを確認。
- 完了: UIの1秒/60秒等の予算が固定1800秒に置換されない。期限超過で後続step0、停止不能はunknownとしてslotを保護する。

### DWR-12: 根拠を持つstep/Goal verifier

- 依存: 10、11。対象: 新規 `steward/verifier.rs`、`steward/execution_contracts.rs`、`steward/transitions.rs`、`runtime/pi/session_reader.rs`。
- 実施: test_report_obtainedは指定対象の結果と失敗一覧/log取得を確認し、exit failureでも調査完了にできる。tests_passは同じ対象・revisionの成功結果のみ。根拠欠落/parse不能はawaiting_user。user_confirmation_requiredは確認receiptなしでdoneにしない。read stepは取得artifact等のstep専用verifierを持ち、Goalのtests_passを流用しない。
- 試験: `dw_r12_verifier_distinguishes_report_pass_failure_and_missing_evidence`。成功/失敗code、異なる対象、旧revision、LLMだけの成功主張、結果0件、曖昧完了条件のmatrix。
- 完了: verifier outcomeから根拠artifact/runへ追跡でき、失敗と未知を区別する。

### DWR-13: plan進行・有限replan・Goal完了

- 依存: 12。対象: 新規 `steward/plans.rs`、`steward/queue.rs`、`steward/transitions.rs`、`steward/repository.rs`。
- 実施: step verifier成功のみで依存を解放。Task成果物・step完了・次Task intent・Goal progressを同じtransactionに保存。有限replanは失敗原因に合う採用済み代替/再試行だけとし、未知結果を再実行しない。Goalの具体的対象を各stepへ維持する。
- 試験: `dw_r13_plan_finishes_goal_and_releases_active_limit`、`dw_r13_failed_unknown_and_awaiting_user_do_not_unlock_success_dependencies`、`dw_r13_replan_is_bounded_and_idempotent`。2段階read→testと最大2回replan、重複eventを含める。
- 完了: 完了Goalがactive上限を永久占有しない。全step結果のないGoal done0。

### DWR-14a: 設定OFF/ONとforeground優先の共通gate

- 依存: 07、11。対象: `steward/authority.rs`、`steward/dispatch.rs`、`steward/tools.rs`、`coding/service.rs`、`schedule/tick.rs`。
- 実施: Memory/Coding/foregroundを全入口とdispatch直前で検査。OFFでは未送信claimを消費せず、runningにはdurable停止要求。ONでqueueを再検査。Schedule OFFと一般報告pumpを分離する。
- 試験: `dw_r14_off_on_matrix_covers_tool_ui_schedule_and_background_queue`。model/UI/legacy/schedule/workerそれぞれに対し、新規受付・queued・runningを組合せる。
- 完了: 無効中の新規効果0、ON後のpending復帰、foreground終了後の公平開始。read-only status/取消は使用可能。

### DWR-14b: withdrawとScope revokeの線形化

- 依存: 05、06、14a。対象: `steward/commands.rs`、`steward/tools.rs`、`steward/transitions.rs`、`runtime/context/scope.rs`、`runtime/pi/runner.rs`。
- 実施: revokeとstop要求を同一writer transactionで確定。ユーザー操作のauthorize後に同じtransaction内で停止intentを記録し、撤回後の再authorize失敗を無視する構造を解消。既存RC scope epoch検査に接続する。
- 試験: `dw_r14_revoke_before_or_after_dispatch_has_defined_effects`、`dw_r14_withdraw_both_orders_stops_actual_fixture_process`。前後順序、別Goal継続、late event、実process停止を確認。
- 完了: 先行撤回でdispatch0、後行撤回でstop receiptと停止/unknownの実態が一致。既存Scope機構を別実装しない。

### DWR-14c: forgetで未配送内容を復活させない

- 依存: 14b。対象: 新規 `steward/invalidation.rs`、`memory/personal_state/commands.rs`、`steward/repository.rs`、`steward/report.rs`、`runtime/context/scope.rs`。
- 実施: source/versionからGoal/Task/artifact/report/message/speechの派生を追跡。forget時に本文・summary・未配送digestを消去/失効し、必要な監査IDと状態だけを残す。音声開始・結果publish直前にもepochを確認し、既に再生済み音声を消せたとは扱わない。
- 試験: `dw_r14_forget_during_hold_or_pending_speech_prevents_redisplay`。held、message commit済みspeech未開始、playback中、late renderer callback、再openを含める。
- 完了: 直接DB・list・Chat再接続・hold解除のどこからも忘却本文が復活しない。テストは実本文markerで検出する。

### DWR-15: report outboxとmessage配送の原子性

- 依存: 13、14c。対象: 新規 `steward/outbox.rs`、`steward/report.rs`、`steward/transitions.rs`、`steward/driver.rs`。
- 実施: 終端/verifier更新・report claim・outbox作成を一つのtransactionにする。次の配送transactionでmessage挿入・delivery状態・message ID・UI配送cursorを同時更新する。TTSとUI emitはcommit後。transactionを受け取る内部関数と境界を所有する関数を分ける。
- 試験: `dw_r15_crash_at_each_report_boundary_keeps_one_message`、`dw_r15_claim_failure_never_loses_report`。message挿入直後、mark_flushed直前、commit直後、UI emit前にfaultを入れfile DB再open。
- 完了: 報告欠落0、同一revisionのmessage1通。行数だけでなくmessage本文/参照・配送stateが整合する。

### DWR-16: UIに依存しない報告・queue pump

- 依存: 07、15。対象: 新規 `steward/pump.rs`、`schedule/handle.rs`、`schedule/tick.rs`、`steward/driver.rs`、`app_state.rs`。
- 実施: 既存schedule start_loopの待機をwake/期限/shutdownのselectへ拡張。Schedule enabledに関係なく一般のdurable仕事をdrainし、期限Actだけ45秒tickに限定する。report0件でも未処理終端・ready Task・recovery対象から会話を見つける。loopを二重起動せず、LLM常時pollをしない。drainは有界batchとし、長いprobe/外部I/Oはloopを塞がないeffect処理へ渡す。
- 試験: `dw_r16_unmounted_ui_and_disabled_schedule_do_not_stop_completion`、`dw_r16_lost_wakeup_is_recovered_from_database`。in-memory wakeを捨ててstartup/照合で回復させる。
- 完了: UI pollingもuser turnもない状態でfixture job→verifier→outbox→message→次Taskまで進む。DB lock中にspeech/SDKを呼ばない。

### DWR-17: 終端・Situation・設定・復帰のwake接続

- 依存: 14a、16。対象: `runtime/pi/runner.rs`、`situation/tick.rs`、`schedule/commands.rs`、`lib.rs`、`steward/pump.rs`。
- 実施: 終端commit後、hold解除確定、設定ON、slot解放、startup/resumeでpumpをwakeする。必要なwork event/revisionはwriterで永続化し、通知は高速化のみとする。時計の大幅変化は期限・所有状態を再照合する。
- 試験: `dw_r17_hold_release_and_restart_deliver_without_user_turn`、`dw_r17_repeated_wakes_have_no_duplicate_effect`。本物のproduction hookを通し、テストからpublishを直呼びしない。
- 完了: sleep中の処理を約束せず、復帰後の照合で失効期限・未配送・実行中processを確認できる。

### DWR-18: 内容を持つ完了/判断待ち報告とTTS方針

- 依存: 12、15、17。対象: 新規 `steward/report_content.rs`、`steward/report.rs`、`steward/outbox.rs`、`voice_behavior.rs`内の既存speech policy接続点。
- 実施: Goal名・依頼対象・結果要約・失敗一覧・verifier根拠・次に必要な判断を報告する。awaiting_user/unknownもreport対象にする。hold中の古い中間報告を最新状態へ集約し、speechは§2.5を適用する。新しいLLM要約が失敗しても構造化結果から報告できる。
- 試験: `dw_r18_reports_show_evidence_and_actionable_unknown`、`dw_r18_mute_hold_and_speech_restart_matrix`。既存real TTSの正常/renderer失敗2試験を保持し、callback後のforgetも確認。
- 完了: `task id: done`だけの報告が残らず、音声OFFでもChatは届く。不明playbackの自動再生0。

### DWR-19a: Goal起点の読取り専用IPC

- 依存: 13、15。対象: 新規 `steward/views.rs`、`steward/commands.rs`、`steward/contracts.rs`、`src/features/coding/stewardApi.ts`。
- 実施: Task0件のGoal、提案確認待ち、progress、可用profile、予約/予算、report/evidence、判断待ち理由を返すGoal一覧DTOを追加。既存Task一覧は必要な互換を維持。list/statusからdispatch・state sync・publishの副作用を除く。
- 試験: `dw_r19_registered_goal_without_tasks_is_visible_and_withdrawable`、`dw_r19_queries_never_dispatch_or_mutate_delivery`。実DBとIPC契約を検証しbindingを生成。
- 完了: 登録直後にA/Bを区別でき、一覧を開かないことが実行停止の原因にならない。

### DWR-19b: 提案確認・Goal管理・recipe登録UI

- 依存: 03、04、10、19a。対象: `src/features/coding/StewardPanel.tsx`、新規 `DelegationConfirmation.tsx`、新規 `TestRecipeForm.tsx`、`stewardApi.ts`、`steward/commands.rs`。
- 実施: 提案に対する一度の確認、委任のみ登録/登録して開始、個別撤回、期限/対象/予算、recipe確認登録、根拠参照、状態照会、§2.6の判断待ち解決を提供。対象/予算拡大は新revision確認へ進める。確認画面を表示した後の条件変更はreceiptを無効化する。必要な有効化設定に現行UI導線がなければ同カードの枝番で追加する。製品UIへtransaction等の内部用語を露出しない。
- 試験: 新規 `tests/delegated-work-panel.test.tsx`。登録後の実DTO、A/B逆順撤回、stale確認、操作不能profile、awaiting_user解決。mock payload確認だけでなく19aとのcontract fixtureを接続する。
- 完了: 未開始Goalも表示し、確認済み権限内の継続で同じ確認を繰り返さない。

### DWR-20: Chatへの配送eventと再接続

- 依存: 18、19a。対象: `steward/outbox.rs`、`steward/pump.rs`、`src/features/chat/useConversationTurn.ts`、`src/features/chat/messageHistoryStore.ts`、新規 `src/features/chat/useDelegatedReports.ts`。
- 実施: commit済みmessage ID/conversation ID/revisionを含むeventを送り、Chatは履歴を再取得してIDでmergeする。別会話の報告を現在応答に混ぜない。event重複/順序逆転/切断時はdurable cursorから追いつき、読んでいる古い履歴やstreaming応答を壊さない。
- 試験: 新規 `tests/delegated-work-chat.test.tsx`。重複event、再接続、会話切替、生成中の背景結果、古い履歴閲覧中を確認する。
- 完了: 次の発話やパネルpollなしでChat表示。再接続でも同じmessageは1件。

### DWR-21: 自然文ケースを本物の受付経路で評価

- 依存: 03、04、19b。対象: `tests/delegated-work-cases.test.ts`、新規 `tests/fixtures/delegated-work/requests.json`、新規 `scripts/delegated-work-eval.ts`、各providerの既存coding bridge試験。
- 実施: Markdownの件数検査を、期待intent・対象・権限・確認要否・Task数を持つ固定corpusへ変換。新規仕事の明示表現を20件以上用意し、status/withdrawは別枠。非採用/確認10件以上と通常の雑談を追加する。streamed/AgentSession/対応音声入口から同じhost判定へ到達させる。
- 試験: `dw_r21_tool_intake_matrix`。scriptedモデル応答の回帰laneと、実モデルで文を入力するlive laneを分ける。liveでは使用model/version・入力case ID・tool call・最終host判断を記録。
- 完了: 引用からauthority/Task0、既存grant内の明示依頼は実行受付、新grantは確認待ち。不合格例を除いて成功率を算出しない。

### DWR-22: crash・race・複数Goalの経路統合試験

- 依存: 06〜07、13〜18、20〜21。対象: 新規 `steward/tests/integration.rs`、新規 `steward/tests/faults.rs`、`coding/integration/tests.rs`、新規 `tests/fixtures/delegated-work/` のprocess fixture。
- 実施: §5のfault matrixをproduction admission/dispatcher/runner/pumpで検証。A/B両方向撤回、slot競合、read→test、replan、hold、OFF/ON、forget/revokeを同じfile DBで組合せる。fault injectionはtest/専用featureに限定する。
- 試験: `dw_r22_full_flow_without_ui_or_new_turn`、`dw_r22_crash_matrix_preserves_effect_and_delivery_counts`。新規user messageの捏造0も検査。
- 完了: test自身がservice retry・terminal状態・reportを代入せず期待状態へ到達。全fault点のreopen後DBとprocess状態に根拠がある。

### DWR-23: macOS実UIの自動受入harness

- 依存: 00（調査）、19b/20（業務scenario）。対象: `package.json`、`src-tauri/Cargo.toml`、`src-tauri/src/lib.rs`、新規 `wdio.delegated.conf.ts`、新規 `tests/desktop/delegated-work.e2e.ts`。capability/configは必要なら23bへ分割。
- 実施: 第一候補はWebdriverIO embedded方式。test専用Cargo featureで `tauri-plugin-wdio-webdriver`、必要なWDIO API pluginを導入し、実WKWebViewの要素をクリック/入力する最小往復を先に通す。production releaseに制御endpoint/追加capabilityを含めない。隔離DBと破棄可能workspaceを使い、業務IPCをmockしない。
- 分岐: pinned依存との互換性等で成立しなければエラーと再現手順を保存し、Xcode XCUIAutomation harnessまたはnative CUAが利用できる環境へ切替える。ブラウザ+IPC mockを実UI成功に置換しない。必要OS権限は明示し、権限ダイアログを勝手に回避しない。
- 完了: 操作→実IPC→実DBの往復、screenshot、log、cleanupを確認。instrumented buildであることを記録し、配布版検証との差を24で埋める。

### DWR-24: 実モデル・実profile・実UIの通し受入

- 依存: 09〜11、18〜23。[元計画](saaa-delegated-work-completion-plan.md) §5の全条件を実施する。
- 実施: UIから有効化、workspace/recipe/委任登録、自然文A依頼、別話題、B追加、競合、A撤回、B完了、会議hold/解除、実test失敗、判断待ち、forget、OFF/ON、sleep/wake、同一DB restart。逆順B撤回を別scenarioで確認。実modelで拒否境界と成果物内容を確認する。
- 証跡: 操作列・画面・case ID・DB trace・process停止・実SDK/profile/renderer結果。配布版でも少なくとも自然文read完了→Chat報告と複数Goal再openを再確認し、test featureとの差を記録する。
- 完了: 実UI上の結果とDB/resultの一致。UI、認証、audio、OS sleep制御が不足する場合は該当scenarioだけ外部待ちにし、DWR-24/旧DW-14を未完了のまま残す。期限・担当・必要操作を含む手動runbookを渡し、実演したとは書かない。

### DWR-25: 応答時間と復帰時の照合

- 依存: 17、22。対象: 新規 `steward/tests/latency.rs`、新規 `scripts/delegated-work-performance.ts`、`steward/pump.rs` の計測点。
- 実施: 終端commit→driver適用、hold解除確定→message commit、期限→receipt、UI event→表示を別時計で記録。awakeの対象ごとに30以上のサンプル、p95算出方法・件数・最大値・除外理由を保存する。workerをテストから直接進めて待機時間を省かない。
- 合格: driver p95 2秒以内、hold解除→message p95 2秒以内、期限1tick+5秒以内。foreground/権限による正当な延期は理由別に記録。sleep中は保証せず、復帰直後の照合で再送・報告重複0。
- 完了: fixtureと実アプリの測定を区別。遅延の原因を修正するか未達として残し、単体reducerの速度で代替しない。

### DWR-26: 全gateと完了証跡の更新

- 依存: 01〜25（外部待ちは状態を保持）。対象: 元計画§0、progress/results、repair-progress/results、module-size baseline。
- 実施: §6のgate、対象prefixの試験件数、全A-IDの証拠を照合する。大型moduleは責務ごとに分割し、新規moduleだけ正規手順でbaseline登録する。既存ratchetを一括緩和して通さない。
- 完了: 実装済み・自動受入済み・実受入済み・外部待ちが別列。SDK/TTSの古い未完了記述と新結果の日時・profileが整合する。全条件を満たした旧カードだけ完了へ戻す。

## 5. 必須のfault matrixと最終scenario

| fault位置 | 回復後の期待 |
| --- | --- |
| admission途中 | 全rollback。同じsource/keyで完全な受付を一度だけ作れる |
| claim直前/直後、Coding commit前 | 未送信を証明できるものだけpendingへ戻す。部分予約・孤立Taskなし |
| local receipt commit後、spawn前 | job/bindingを発見し既存deliveryを照合。別jobの作成0 |
| Pi送信直前/送信後、受領保存前 | prepared/送信可能性を区別。不明はunknown。自動再送0 |
| 結果commit前/後 | 成果物/step/verifier/cursorの整合。再適用で次stepとreplanは増えない |
| report claim後、outbox挿入前 | rollbackで再処理可能。report消失0 |
| message挿入後、delivery更新前 | rollbackでmessage0またはcommitで1。再openで2にならない |
| message commit後、UI emit前 | DBから再配送し表示1件 |
| speech claim後/再生中 | restartでdelivery_unknown。自動再生0 |
| 上記とwithdraw/forget/revoke競合 | 線形化順序が一意。先行失効は新規効果0、後行停止はreceipt/unknownを報告 |

最終scenarioはDWR-24の操作を一つの長い試験だけに押し込めず、正常系、失敗/判断待ち、A/B両方向撤回、hold/音声、OFF/ON、forget/revoke、sleep/restartに分けて再実行可能にする。その上でA/B・話題切替・競合・撤回・hold・restartを一続きに行う総合試験を1本残す。意図的に不明にしたrunを勝手に成功させて後半へ進めない。

## 6. 検証コマンドと証拠の形式

既存コマンドはrepository rootから実行する。新規 `dw_r` prefixは0件なら失敗として扱う。各カードの実装に関係する対象試験を先に実行し、節目で以下を通す。

```sh
cargo test --manifest-path src-tauri/Cargo.toml --lib dw_r -- --test-threads=1
cargo test --manifest-path src-tauri/Cargo.toml --lib steward:: -- --test-threads=1
cargo test --manifest-path src-tauri/Cargo.toml --lib coding:: -- --test-threads=1
cargo test --manifest-path src-tauri/Cargo.toml --lib schedule::tests -- --test-threads=1
bun test tests/delegated-work-cases.test.ts tests/steward-panel.test.tsx tests/coding-steward.test.ts tests/pi-codex-sdk.test.ts
bun run ipc:generate
bun run ipc:check
bun run typecheck
bun run size:check
bun run check:local
bun run spec:check
bun run desktop:smoke
```

新規のpanel/chat/desktop試験とeval/performance scriptは各カードで実在するpackage scriptへ登録し、repair-resultsに正確な呼出し方を残す。実SDK/TTS試験はopt-inで別実行し、ignoredをpassと数えない。GUIやaudioを持たないCIでは理由付きskipとし、最終実受入は対応hostで必ず実施する。

全体gateが他作業由来で落ちた場合、ファイルとエラーを別枠で記録して修正済み条件と分ける。対象自身のsize超過・未登録baseline・型/IPC不整合は本テーマの残件として解消する。`check:local`がformat段階で止まったとき、後続lint/clippy/testが成功したと報告しない。

証跡rowは `cardId / caseId / HEAD+対象hash / source(kind,id,version) / grantRevision / goalRevision / plan,step / task / intent,attempt / job,run,receipt / evidence,verifier / report,message,speech / observedAt / expected / actual / status` を持つ。実データ本文・認証・個人pathを残さず、case IDと合成markerで追跡する。実SDK失敗理由を「モデルが拒否と言った」だけで記録せず、host/toolの拒否結果と副作用の有無を確認する。

## 7. UI受入の外部条件と着手指示

macOSの第一候補は、外部tauri-driverを必要としないembedded WebDriverである。SAAAには現時点でtest plugin/harnessがなく、導入後の互換性実証が必要。[Tauri公式WebDriver資料](https://v2.tauri.app/develop/tests/webdriver/)と[WebdriverIO Plugin Setup](https://webdriver.io/docs/desktop-testing/tauri/plugin-setup/)を参照する。

実受入に必要なのは、実modelの認証、対応profile、破棄可能Git workspace、隔離DB、操作可能なGUI session、音声出力、sleep/wake/restartの制御である。現在のCUAがnative操作を提供しないことと、macOSで自動化手段が存在しないことを混同しない。test harnessは本計画の実装対象だが、OSの許可や実機利用まで自動的に満たされるとは扱わない。

Terraへの着手指示:

> DWR-00から依存順に実装してください。監査A01〜A12を対応表で追跡し、カード単位で実装・試験・証跡を更新してください。自然文のplan欠落だけを直して未確認権限を実行へ流さず、03/04を一体として成立させてください。UIのpollingやテストの手動再呼出しに依存しないdispatch・検証・配送まで閉じてください。SDK/Pi制約やUI harnessの不足は本書の分岐で解決し、実受入の外部条件だけを理由付きで残してください。共有ワークツリーを保全し、別タスクへは送信せず、全条件が証明できたカードだけ完了にしてください。
