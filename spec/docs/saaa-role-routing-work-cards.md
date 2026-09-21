# Role Routing 作業カード

状態: **監査済み・未完了**。以下の進捗表は 2026-09-21 の作業treeを確認して更新した。`部分`はコード断片または限定経路があるだけで、カードの合格条件を満たした意味ではない。`未着手`は実装がない。全カードを完了とする報告はまだしてはならない。
[計画](saaa-role-routing-plan.md) / [実行契約](saaa-role-routing-execution-contract.md) / [学習契約](saaa-role-routing-learning-contract.md) / [受入](saaa-role-routing-acceptance.md)

## 完了・未完了の要約（2026-09-21）

カードの合格条件と指定試験を基準に判定する。**完了カードは0/40件**である。部分実装は完了ではなく、未実装の安全条件・縦通し経路・試験を残している。

| 区分 | 件数 | カード |
| --- | ---: | --- |
| 完了 | 0 | なし |
| 部分 | 40 | RR-00〜39 |
| 未着手 | 0 | なし |

現時点で動作確認できたサブ機能は、receipt/ledgerの作成、queue上限拒否、FIFO queue解放、root deadline、snapshot/cancel IPC、Codex JSONL sidecarの同梱とfake protocol検証、`codex_sdk` actorの限定dispatch、学習データ処理の一部である。これらは各カードの一部を満たすだけで、R1〜R3の受入完了を意味しない。

優先して残る実装は、role専用context projection（RR-07）、Provider/Sinkとtool ledger/speech ownerの実経路（RR-09/11/13）、Sol host tool loop（RR-21）、評価・revision・premium承諾（RR-24〜26）、R2競合試験（RR-29）、tool-specialist（RR-38）、全体受入・性能測定（RR-39）である。加えて、policy snapshotをqueued rootのdispatch時にも一貫して参照する保証、実認証SDKを使うlive isolation、ASR→tool→TTSのE2Eは未検証である。

外部要因で停止しているカードはない。RR-21 は未接続の実装である。固定版 `@openai/codex-sdk` の `runStreamed` は MCP 呼び出しを観測するだけでhost callbackを提供しないが、既存の認証付きloopback MCP gatewayをsidecarが唯一接続する host-controlled bridge として構成できる。C8.3を維持するため、そのbridge以外のMCP・ネットワーク・native toolは明示的に遮断し、tokenをJSONLやDBへ出してはならない。それ以外の主な阻害要因は未実装の縦通し経路と試験不足である。live laneだけは認証済みSDK・許可モデル・隔離fixtureを用意した明示的な実行が必要であり、現状は実施していない。作業treeにはrole-routing外の変更も混在するため、V5全体試験の失敗は変更単位ごとに切り分ける。

## 実装監査（2026-09-21）

| カード | 状態 | 現在の根拠 | 合格までに残ること |
| --- | --- | --- | --- |
| RR-00 | 部分 | `baseline.md` と `progress.md` にdirty差分、schema、SDK、live gateを記録 | 実機接続能力を明示fixtureで検証 |
| RR-01 | 部分 | `contracts.rs`、`reducer.rs`、`signals.rs` | event契約、Clock/ID注入、指定境界試験 |
| RR-02 | 部分 | R1 ledger DDL、migration idempotency試験 | C6全FK/複合整合性、旧DB/FK試験 |
| RR-03 | 部分 | `routing.roles` validationとpolicy snapshot。queued rootはreceiptのpolicyを優先してdispatchし、stepにはpolicy/recipe/actor由来のSHA-256 fingerprintを保存 | settings更新CAS、指定試験 |
| RR-04 | 部分 | receiptを`prepare_runtime_run` transactionへ接続し、queue上限をcommit前に拒否。active root がある receipt は queued として provider 前で待機 | retry/conflict、receipt復元 |
| RR-05 | 部分 | pure reducerのみ | coordinator、driver、mpsc、registry、IO試験 |
| RR-06 | 部分 | recipe候補の決定的選択、shadow関数 | hard filter・予算・sticky選択・decision理由 |
| RR-07 | 部分 | role dispatch時にMust-only projectionを通し、許可scope外・revoked必須sourceを拒否。必須候補がない通常会話は空projectionを許可 | revoked sourceの実データ接続、root初回条件の永続化、旧context fixture回帰 |
| RR-08 | 部分 | 保守的なfollow-up文字列分類のみ | 構造化分類、根拠/target検証、frontend prompt |
| RR-09 | 部分 | Provider開始を本文なしのroot-local activityとして永続化し、terminal rootへの追記を拒否。最終結果だけを既存root採用transactionへ渡す | delta→activity Sink、partial/timeout/frontend failureの縦通し試験 |
| RR-10 | 部分 | pure permit関数のみ | 実dispatcherでの二重permit |
| RR-11 | 部分 | 既存tool gatewayでinvoke前にoperationをreserve/dispatched化し、ownerのinvocation/result receiptをsettleする。重複operationは再実行しない | caller detach、実tool loopの縦通し試験 |
| RR-12 | 部分 | message保存transaction内でroot採用を試行 | stale/cancel/duplicate/DB failureの縦通し試験 |
| RR-13 | 部分 | role rootでは既存voice responseのack/progressを抑止し、確定後のfinalのみ既存completion経路へ渡す | routing speech ownerの実queue、永続speech状態、FakeSpeech試験 |
| RR-14 | 部分 | receipt/snapshot/replay/cancel IPC、型付きchat表示・停止操作。永続cancelをlive eventでsnapshot再読込 | live replay store、ASR receipt、legacy/duplicate試験 |
| RR-15 | 部分 | provider actor準備・role選択UI | probe、完全編集、ASR→tool→TTS E2E |
| RR-16 | 部分 | barrier用pure reducerと分類候補 | 実行中入力の保存、保留、採用barrier |
| RR-17 | 部分 | reducerのcancel/revision状態と、tool未確定中のtransactional restart拒否 | input更新/child drain実行、全順序試験 |
| RR-18 | 部分 | queue順序・in-flight restart復旧を起動writerへ接続。queued turn は provider 前で待機し、終端後に最古 queued root を `IMMEDIATE` transaction で claim | 再接続、restart 後 queued receipt の明示的再開 UX |
| RR-19 | 部分 | SDK固定版のJSONL sidecar、Rust JSONL protocol validator。`codex_sdk` actor をconversationからsidecarへdispatchし、root採用transactionへ保存 | mock SDK wire試験、実認証SDK呼び出し |
| RR-20 | 部分 | sidecarをBun compiled resourceとして同梱し、ProcessGuardで起動・cancel回収。fake executableでEOF/cancel/config隔離を検証 | live isolation gate |
| RR-21 | 部分 | sidecarは既存の認証付きloopback MCP gatewayだけを `rrRoot` に束縛して接続する。MCP sessionはactive rootの会話へ解決され、tool呼び出しは `rr_tool_links` のreserve/settleを通る。tokenはchild環境だけに渡しJSONLへ出さない | 実認証SDKによるSol tool roundtrip、revision/model変更時のnew thread、tool budget、live isolation試験 |
| RR-22 | 部分 | root deadlineをreceipt・provider route・queued待機へ接続。未知費用を拒否するpure判定。Codex SDKの確定usageを型検証し、最終回答採用transaction内でstepへ保存 | provider別費用換算、active dispatchの集計、切替上限 |
| RR-23 | 部分 | host feedback保存とdirty mark。新規user inputの明示challengeは、会話の最新assistantがcompleted role rootの回答である場合だけ同一transactionで保存 | explicit positive/negativeの実入力接続、challengeを新rootとして起動する経路 |
| RR-24 | 部分 | model/actor名を含めないreview packet、issueの型付きvalidator、author/reviewer独立性、同一root・revision内のaccepted evidence scope検証、review outputの台帳保存、reviewerのmutating tool拒否 | 独立評価executor、read-only toolの実gateway接続、通常turnへのreview step組込み |
| RR-25 | 部分 | verified issueとreview round上限を照合するrevision gate。accepted reviewからverified/unresolvedを分離し、root revisionを変更せずrevision decisionとして永続化 | 評価後revision executor、decision消費時のroot状態再検証、authorへの修正prompt組込み |
| RR-26 | 部分 | candidate/policy/revision/期限/cloud制約を照合する明示承諾gate。premium proposalをroot/policy/revision/candidate/見積費用へ束縛して永続化し、候補名を指定した承諾時に再検証 | Astra提案・承諾のIPC/UIと実dispatch接続、dispatch直前のprovider capability再検証 |
| RR-27 | 部分 | snapshot由来のchat表示とroot cancel操作 | amend/reconsider、live event、child drain |
| RR-28 | 部分 | chatで永続snapshotのphase/revision/queue先頭を表示 | 実行履歴・選択理由UI |
| RR-29 | 部分 | 隔離role-routing suite 80件がpass。queue/cancel/tool/sidecar/review/proposalの単体・限定統合を確認。sidecar bridge fixtureを起動負荷と無関係な10秒timeoutで安定化 | Provider/TTS/tool完了順のBarrier競合fixture、live lane、A13〜A30受入 |
| RR-30 | 部分 | R3 tables、限定feature snapshot | immutable全feature snapshot・C6確認 |
| RR-31 | 部分 | dirty queueとdataset materialize | event上限/checkpoint/page再開 |
| RR-32 | 部分 | explicit feedbackの限定ラベル | L2全label/conflict/revision |
| RR-33 | 部分 | 本文なしJSONL export、examples hash、manifest最後のatomic rename | group split、時系列境界、DB job接続 |
| RR-34 | 部分 | pure scheduler判定、60秒writer tick、manual materialize IPC、learning件数snapshot | foreground/shutdown pause、job状態 |
| RR-35 | 部分 | shadow artifact保存とpure score | 観測集計、shadow記録、最低例数 |
| RR-36 | 部分 | hash・feature/candidate検証済みlinear-v1 loader、観測label限定のoffline評価 | selection接続、昇格証跡 |
| RR-37 | 部分 | forget sourceからdataset/artifactを同一transactionで失効。設定画面で本文なし学習状況/手動materialize | filesystem journal |
| RR-38 | 部分 | specialistは型付きhost tool requestのみ返し、無効時や不正requestを拒否。最終回答権限を持たない | specialist差替え、既存tool gatewayへの実接続 |
| RR-39 | 部分 | offline suite 80件・desktop smokeの証跡と未完一覧を`results.md`へ記録 | V1〜V6、A01〜A42、性能・live報告 |

## 共通規則

- 以下の`rr/`は新規`src-tauri/src/role_routing/`、`rt/`は既存`src-tauri/src/runtime/`。N=新規、M=変更。新規モジュールのmod登録は付随変更。
- テスト名は`rr_NN_条件`。対応する`rr/tests/<テーマ>.rs`へ置く。実装だけのstubを完了としない。
- 各カードは目的、対象、手順、具体的試験、合格条件を持つ。指定外の周辺リファクタは禁止。5実装ファイルを超える場合は枝番に分け、同じ契約を維持する。
- 検証commandは末尾V1〜V6。失敗したら後続へ進めずそのカードで直す。環境不足は未検証と記録し、mock成功をlive成功にしない。

## R1: 会話と通常推論

### RR-00 基準・接続能力の記録

- 契約: 計画§3/8、C2/C8。N `spec/evidence/role-routing/baseline.md`、`progress.md`。
- 実装前にHEAD、dirty差分、schema version、SDK固定版、既存suite結果を記録する。既存dirty差分は変更しない。
- front/QwenのProvider ID・model ID・tools・structured output・resourceGroup・常駐可否、Codexのlogin/CLI版を秘密抜きで記録する。credentialをDBや文書へコピーしない。
- 試験: V2/V3の既存対象とV4。liveアクセスがなければ欠けた項目とgateを列挙する。
- 合格: 検証済み/未検証の区別が明示され、後続のmock作業に必要なfixture actorを定義済み。

### RR-01 型と純粋なイベント

- 契約: C0/C1。N `rr/contracts.rs`、`rr/events.rs`、`rr/mod.rs`。M crate module登録。
- C1の型、Action、phase、step status、reason codeを定義。Clockはhost時刻の注入trait、ID生成もtest注入可能にする。
- serde未知field拒否、ID・文字数・byte・finite値の境界をvalidatorで一元化。
- 試験: `rr_01_unknown_field`、`rr_01_utf8_limit`、`rr_01_invalid_id`。V1。
- 合格: permissive Valueだけの契約にせず、newtype/enumで不正なActionとstatusを拒否できる。

### RR-02 永続schemaとmigration

- 契約: C6。N `rr/schema.rs`、`rr/schema.sql`。M `persistence/schema.rs`。
- C6の11 table、FK、CHECK、indexを追加。schema versionは現行+1。初期DBと旧DBのmigrationを両方作る。
- 同時active rootをDBで拒否し、nullable targetのSET NULLとcascadeを確認する。
- 試験: `rr_02_migrate_twice`、`rr_02_one_active_root`、`rr_02_foreign_key_delete`。V1とsqlite_architecture。
- 合格: 2回migrationしてデータ不変、旧会話・settings不変、FK check 0件。異なるrootのstep/linkを複合FKで拒否する。

### RR-03 設定の検証と版管理

- 契約: C2。N `rr/settings.rs`、`rr/policy.rs`。M `persistence/settings.rs`、settings snapshot cacheの失効。
- routing.roles文書のdefaults/validation/save/loadを追加。config fingerprintとimmutable policy rowを同一transactionで保存する。
- enabled=falseの空初期設定を許可し、有効化時のrole/capability/参照/予算を厳格に検査。モデル名の条件分岐を書かない。
- 試験: `rr_03_unknown_recipe`、`rr_03_policy_cas_conflict`、`rr_03_location_mismatch`、`rr_03_disabled_empty`。V1。
- 合格: 新設定が既存routing.tasksやpiのLuna設定を書き換えない。

### RR-04 repositoryと受付transaction

- 契約: C5/C6/C7。N `rr/repository.rs`、`rr/inputs.rs`。M `rt/turns.rs`のprepare共通helper抽出。
- message/root/runtime/input/scope/eventを一つのtransactionで作る。model IOは行わない。
- inputId/sourceId重複は元receiptを返す。payload差異はconflict。queue上限拒否ではmessageを保存しない。
- 試験: `rr_04_receipt_retry`、`rr_04_same_id_changed_payload`、`rr_04_queue_full_no_message`。V1/V2。
- 合格: receipt成功時にはSQLiteから同じ受付を必ず復元できる。

### RR-05 reducerとactorの骨格

- 契約: C0/C5。N `rr/reducer.rs`、`rr/coordinator.rs`、`rr/driver.rs`。
- pure reducer→DB commit→Effectの順に実装。async workerの完了をmpscイベントで返し、actorはIO待ちしない。
- conversation単位のregistryを作り、thread終了時の解放と再取得を定義。永続状態をactor cacheだけに置かない。
- 試験: `rr_05_commit_before_dispatch`、`rr_05_one_actor_per_conversation`、`rr_05_io_does_not_block_input`。V1。
- 合格: FakeAdapterでroot受付からresult候補まで動作、DB失敗ならdispatch 0回。

### RR-06 候補生成とルールranker

- 契約: C3。N `rr/selection.rs`、`rr/ranker.rs`。
- hard filter、初期選択表、tie break、switch marginを実装。rulesはnetwork/DBなしの純粋関数。
- cloud禁止、能力不足、price不明、既存actor維持を理由コード付きで返す。
- 試験: `rr_06_cloud_filter`、`rr_06_unknown_cost`、`rr_06_sticky_actor`、`rr_06_invalid_ranker_fallback`。V1。
- 合格: 候補集合と除外理由をdecisionに保存でき、未知candidateの実行0回。

### RR-07 context projectionの分離

- 契約: C4。N `rt/context/role_projection.rs`、`rr/context.rs`。M `rt/conversation_inputs.rs`。
- 既存broker/loadの共通関数へ明示的なinput境界を渡し、root初回条件+amendmentsをMustで組み立てる。
- scope/epoch/来歴の検査とgeneration入力記録を維持。World shadow本文のdispatchは追加しない。Scopeの取り消しを毎step確認する。
- 試験: `rr_07_amendment_present_once`、`rr_07_scope_no_widening`、`rr_07_revoked_source`。V1/V2。
- 合格: 旧context fixture結果不変、最新条件が欠けたenvelopeは送信不可。

### RR-08 role別promptと構造化分類

- 契約: C3.1/C4/C9。N `rr/classifier.rs`、contexts配下のrole定義。M 生成設定は既存形式に従う。
- kind/evidence/target/confidenceをparseし、hostがtargetと根拠範囲を検証。timeoutはunclear。
- frontend replyKey allowlistと混合依頼拒否を実装。引用や否定だけのchallengeを拒否するfixtureを作る。
- 試験: `rr_08_quote_not_feedback`、`rr_08_mixed_greeting`、`rr_08_bad_target`、`rr_08_timeout_unclear`。V1とs11tnext:build/check。
- 合格: classifierが直接tool dispatch、承諾、完了を発行できない。

### RR-09 Provider adapterと内部Sink

- 契約: C8.1/C8.2。N `rr/adapters/provider.rs`、`rr/adapters/sink.rs`。M `providers/stream/mod.rs`。
- ActorAdapterを既存HTTP/SSEへ接続。未確定Deltaは内部bufferへ、activityはrootの安全な進捗へ変換する。
- root/step generationの来歴を記録し、partial response/timeoutを失敗にする。frontend失敗はreasonerを落とさない。
- 試験: `rr_09_partial_stream_no_answer`、`rr_09_child_delta_not_spoken`、`rr_09_frontend_failure`。V1とchat_completions suite。
- 合格: mock HTTPから最終候補取得まで到達し、既存callerのSSE挙動不変。

### RR-10 role別tool offerとpermit

- 契約: C5/C8.4。N `rr/tools.rs`。M `providers/chat_completions/mod.rs`、`providers/stream/dispatch.rs`。
- offer時とinvoke直前の両方でrole制約を適用。frontendは0tools、reviewはread-only。
- Root/step/revisionをactorへ照合し、permitを得た呼び出しだけ既存dispatcherへ送る。旧callerは従来path。
- 試験: `rr_10_frontend_tool_rejected`、`rr_10_unoffered_tool`、`rr_10_revision_before_permit`。V1と既存tool tests。
- 合格: 誤ったtool要求の副作用0。promptだけで制限しない。

### RR-11 ツール実行の紐付け

- 契約: C6/C8.4。N `rr/tool_ledger.rs`。M `tool_selection/gateway.rs`、`tool_selection/service.rs`の最小hook。
- invocation IDとoperationKeyをlinkへ保存し、resultRefと確定状態を同じownerから取得する。
- caller futureをdropしても既存invocation管理を止めない。既知のunknownを再実行しない。
- 試験: `rr_11_detach_keeps_ledger`、`rr_11_duplicate_operation`、`rr_11_unknown_no_retry`。V1とtool_selection suite。
- 合格: Qwenの実tool loopを通じ、search/describe/invokeの記録がrootに遡れる。

### RR-12 結果の原子的採用

- 契約: C5/C6。N `rr/acceptance.rs`。M `providers/session_store.rs`。
- 既存回答保存をtransaction内helperへ抽出。旧公開関数は同じ動作を維持する。
- runtime running、revision、cancel、scopeをtransaction内で照合し、message/root/eventを同時確定する。
- 試験: `rr_12_stale_result`、`rr_12_cancel_wins`、`rr_12_duplicate_completion`、`rr_12_db_failure_no_speech`。V1/V2。
- 合格: 同じrootの最終回答は最大1件。失敗時に完成messageだけ残らない。

### RR-13 発話owner

- 契約: C9。N `rr/speech.rs`、`rr/speech_queue.rs`。M `rt/event_hub.rs`は必要な共通adapterだけ。
- ack deadline、priority、最終回答の重複抑止、speech状態保存を実装。旧voice_responseはRouting rootで起動しない。
- FakeSpeechで開始/停止完了を制御。未確定textをStreamingSpeechRuntimeへ渡さない。
- 試験: `rr_13_final_before_ack`、`rr_13_ack_stop_timeout`、`rr_13_tts_failure_preserves_answer`。V1。
- 合格: speech同時実行数<=1、TTS failureが保存済み回答を失敗へ戻さない。

### RR-14 IPC・起動・UI接続

- 契約: C7。枝番aはbackend、bはfrontend。N `rr/ipc.rs`、`src/lib/roleRouting.ts`、`src/features/chat/useRoleRouting.ts`、`src/lib/roleRoutingStore.ts`。
- M app_state/lib/test_state/quality_evalのstate初期化、IPC bindings、useConversationTurn。機械的初期化以外はa/bで分離。
- 新IPCのreceipt/snapshot/seq replayを接続。enabled=falseでは既存submit。ASR settleは受付時。
- 試験: `rr_14_reconnect_replay`、`tests/role-routing-session.test.ts`のduplicate/ASR/busy/legacy。V1/V3/V4。
- 合格: UI reload後も同じrootを表示、最終回答までASR queueを占有しない。

### RR-15 R1設定UIと縦通しgate

- 契約: C2/受入A01〜A12。N `src/features/settings/RoleRoutingSection.tsx`、`src/lib/roleRoutingSchemas.ts`。
- M settings defaults/draft/page。Provider選択、role割当、probe結果、無効化を編集可能にする。秘密は入力しない。
- 模擬ASR入力→1.2B受付→Qwen tool→DB確定→TTSを一つのテストで通す。live front/Qwenも任意の明示laneで実行。
- 試験: `rr_15_r1_end_to_end`、settings validation suite、V5/V6。
- 合格: A01〜A12とlive gateの結果をR1報告に残す。未確認ならR1のoffline完了までとする。

## R2: 会話の継続と委任

### RR-16 実行中入力の分類barrier

- 契約: C3/C5/C7。M `rr/inputs.rs`、`rr/coordinator.rs`、`rr/classifier.rs`。
- 追加入力を保存した瞬間に採用/新tool開始のbarrierを立てる。分類結果が確定するまで旧回答を採用しない。
- status/socialならbarrierを解除、条件追加ならrevision更新、unclearならclarify。分類待ち中のresultは保存して保留する。
- 試験: `rr_16_result_during_classification`、`rr_16_status_does_not_cancel`、`rr_16_ambiguous_holds_result`。V1。
- 合格: 分類の遅さにより古い条件の回答が先に発話されない。

### RR-17 更新・cancel・drain

- 契約: C5/C11。N `rr/revision.rs`。M reducer/driver。
- child cancellationをroot cancellationと分け、条件更新ではrootを継続する。toolの確定を待って新revisionを開始。
- result→updateとupdate→resultの順序、cancel→tool-completeの順序をFakeClock/Barrierで固定する。
- 試験: `rr_17_update_then_result`、`rr_17_result_then_update`、`rr_17_tool_unknown_blocks_restart`。V1。
- 合格: 旧結果は証跡に残り最終回答にはならない。mutationの自動再実行0。

### RR-18 queue・再接続・再起動

- 契約: C5/C6/C7。N `rr/recovery.rs`。M inputs/repository。
- queued rootを受付順で開始。同時開始をpartial unique indexとtransactionで防ぐ。
- startup reconcileとUI再購読を接続。pending speechとproposalを失効させる。
- 試験: `rr_18_restart_no_auto_dispatch`、`rr_18_queue_order`、`rr_18_cancel_queued`。V1/V3。
- 合格: 再起動で同じtoolを実行し直さず、UIにinterruptedとして見える。

### RR-19 SDK wire契約とsidecar

- 契約: C8.3。N `scripts/role-routing/codex-sidecar.ts`、`scripts/role-routing/protocol.ts`、`rr/adapters/codex_protocol.rs`。
- run/cancel/terminal frameを実装。Codex constructorとthread factoryをinjectし、mock SDKでrunStreamedを検証する。
- turn.completed欠落、重複terminal、未知ID、outputSchema不一致を拒否。認証を使うlive呼び出しはまだ行わない。
- 試験: `tests/role-routing-codex.test.ts`、`rr_19_frame_validation`。V1/V3。
- 合格: SDK実型でtypecheckし、as anyによるAPI捏造がない。

### RR-20 SDK隔離・同梱・能力gate

- 契約: C8.3。N `rr/adapters/codex.rs`。M build.rs、resource登録、既存ProcessGuard接続。
- 同梱Codexのpath解決、empty cwd、read-only、native tools無効化、継承MCP無効化を固定版で検証する。
- Fake executableでargv/config/EOF/cancelを検証。live隔離fixtureでSDKがhost外のtoolを実行しないことを確認する。
- 試験: `rr_20_process_cancel_tree`、`rr_20_config_isolation`、`rr_live_20_sdk_isolation`（ignored）。V1/V4/V6。
- 合格: tools隔離が不明ならactor unsupported。通らない設定を黙って緩めない。

### RR-21 Sol推論とhost tool loop

- 契約: C8.3/C8.4。N `rr/adapters/codex_loop.rs`。M tools/driver。
- SDK answer/tool_request/escalationをhostで処理。toolは既存gateway、結果を同stepの新SDK turnへ渡す。
- root/revision/model fingerprintが変わったらnew thread。同step followup以外で旧threadをresumeしない。
- 試験: `rr_21_sol_tool_roundtrip`、`rr_21_changed_revision_new_thread`、`rr_21_tool_budget`。V1/V3。
- 合格: Solの結果をrootへ採用でき、外部Codexタスクへの送信0。

### RR-22 委任と時間・費用上限

- 契約: C2/C3/C8。N `rr/budget.rs`。M selection/driver。
- request_escalation、automatic switch上限、step/root deadline、費用不明時の制約を実装。
- tool実行中はdelegate予約に留め、settle後に再検証。クラウド禁止はdispatch直前にも確認。
- 試験: `rr_22_qwen_to_sol`、`rr_22_cloud_revoked_before_dispatch`、`rr_22_loop_budget`。V1。
- 合格: 委任理由が記録され、上限超過は説明可能な終了になる。

### RR-23 回答への反応の紐付け

- 契約: C10/L2。N `rr/feedback.rs`。M classifier/repository。
- target answerを一意に解決し、evidence span、明示/推定、曖昧性を保存。既存tool feedbackのDBを流用して混在させない。
- challengeは新rootとして旧回答を参照する。既にcompletedのrootをrunningに戻さない。
- 試験: `rr_23_wrong_conversation`、`rr_23_bare_really_ambiguous`、`rr_23_quoted_negative`。V1。
- 合格: 普通の追加質問を勝手な低評価labelにしない。

### RR-24 独立評価の実行

- 契約: C1/C3/C4/C10。N `rr/review.rs`。M prompt生成物。
- authorと別actorを選び、モデル名を伏せた依頼・回答・根拠を渡す。review toolsは読み取りのみ。
- issue schema、evidenceRefの存在・scope、verdictを検査。根拠不明はunverified。
- 試験: `rr_24_sol_reviewed_by_qwen`、`rr_24_self_review_not_independent`、`rr_24_false_evidence`。V1。
- 合格: reviewerの主張だけでverifiedTaskSuccessを更新しない。

実装状況（2026-09-21）: `ReviewRequest` は回答本文とevidence refだけをserialiseし、author/reviewerのactor/model名を含めない。`record_review_response` はreview stepとrespond stepからactorを復元して独立性を確認し、同一rootかつreview revision以前のaccepted outputだけをevidenceとして受理して`rr_outputs(kind=review)`へ記録する。`rr_24_` 5件はpass。reviewerを実際にproviderへdispatchするexecutor、read-only permitを実MCP gatewayへ結線する処理、通常の応答rootへreview stepを追加する処理は未実装であり、このカードは部分完了のままである。

### RR-25 評価後の修正

- 契約: C3/C10。N `rr/revision_recipe.rs`。M driver。
- verified issueとunverified issueを分けてauthorへ渡す。改善していない場合の1回上限とunresolvedを実装。
- supportedなら再検討結果を簡潔に説明し、根拠なしの結論反転をしないfixtureを作る。
- 試験: `rr_25_review_then_revise`、`rr_25_unsupported_critique`、`rr_25_review_round_limit`。V1。
- 合格: Sol→Qwen評価→Sol修正が一つの新rootで完了する。

実装状況（2026-09-21）: accepted review outputからhostが`ReviewRevisionDecision`を作成する。verified issueだけが`revisionAllowed`の根拠になり、unverified / unresolved issueは同じdecisionに保存されるが自動revisionを許可しない。decisionの保存はroot revisionやdispatchを変更しないため、将来のexecutorはroot状態・上限を再検証してから消費しなければならない。`rr_25_` 3件はpass。Sol→Qwen→Solを実行するexecutorとauthor promptへの組込みは未実装である。

### RR-26 Astra提案と承諾

- 契約: C10。N `rr/proposals.rs`。M IPC/selection。
- proposalをcandidate/revision/policyへ束縛。期限、decline、価格不明表示、利用不可を実装。
- 承諾で新しいreasoning stepを開始する前にhard filterを再実行。「はい」だけで承諾しない。

実装状況（2026-09-21）: `rr_premium_proposals` にproposal receiptを保存する。作成時はactive rootのpolicy/revision、premium actor、cloud location、期限、設定済みの費用上限を検査する。承諾はproposal IDと候補IDの両方を必須とし、rootのpolicy/revision/phaseと期限・cloud許可を再照合するため、単独の「はい」は承諾にならない。`rr_26_` 3件はpass。提案・承諾のIPC/UI、承諾後のprovider dispatch、dispatch直前のcapability/価格hard filter再検証は未実装である。
- 試験: `rr_26_premium_no_implicit_execution`、`rr_26_stale_approval`、`rr_26_accept_but_cloud_forbidden`。V1。
- 合格: 提案だけではSDK起動0回。明示承諾と有効設定でのみ起動。

### RR-27 会話UIの継続操作

- 契約: C7/C9/C10。M useRoleRouting/roleRoutingStore。N `src/features/chat/RoutingProposal.tsx`。
- 思考中の追加入力、確認質問、提案ボタン、処理停止と音声停止の区別を実装。
- UI表示は保存済みreceipt/event/snapshotから作り、actorの選択をfrontendで推測しない。
- 試験: frontend update/cancel/proposal/late event/ASR receipt。V3/V4。
- 合格: 画面の承諾後にreloadしても二重委任しない。

### RR-28 実行履歴・理由の表示

- 契約: C6/C7。N `rr/diagnostics.rs`、`src/features/settings/RoutingDiagnostics.tsx`。
- rootごとの担当、行動、revision、選択理由、利用不可理由、latencyを表示。内部思考、APIキー、workspace本文は表示しない。
- Settingsでpolicy有効化前の検証、fallback理由、unknown費用を見られるようにする。
- 試験: `rr_28_redacted_diagnostics`、frontend snapshot fixture。V1/V3。
- 合格: 不明値を0にせず、再考と単なる障害fallbackを区別できる。

### RR-29 R2競合・実機gate

- 契約: 受入A13〜A30。N `rr/tests/r2_integration.rs`、R2報告。
- Provider/TTS/toolの完了順を手動Barrierで入れ替え、sleepによる偶然の試験にしない。
- Sol/Astra利用可否は別live lane。Astraの課金を伴う試験は設定上の許可・予算内だけで実施する。
- 試験: 全A13〜A30、V5/V6。
- 合格: 統合経路の二重実行・古い発話0。live未完は未検証と残す。

実装状況（2026-09-21）: `cargo test --locked --manifest-path src-tauri/Cargo.toml --target-dir /tmp/saaa-rr-policy --lib rr_` は80件pass。Codex bridge fixtureはtokenのJSONL非露出を検証するもので起動性能を測るものではないため、full suiteの負荷で生じる偶発timeoutを避けてtimeoutを10秒へ固定した。Provider/TTS/toolの実完了順を扱うBarrier競合fixture、A13〜A30の網羅、live laneは未実施である。

## R3: 学習データと運用

### RR-30 学習schema・特徴snapshot

- 契約: L1/L4。N `rr/learning/schema.rs`、`rr/learning/features.rs`。M schema登録。
- 6 tableとindexを加算。featuresはdecision時点で保存し、夜間に最新worldから再作成しない。
- source lineage、dataset version、nullable品質をtyped structで定義する。
- 試験: `rr_30_feature_snapshot_immutable`、`rr_30_nullable_quality`、migration。V1。
- 合格: 将来のfeedbackが過去featuresへ漏れない。

### RR-31 増分抽出とcheckpoint

- 契約: L3/L4。N `rr/learning/extract.rs`、`rr/learning/jobs.rs`。
- upper seqを固定してpageを抽出。dirty rootをupsertし、page保存とcursor進行を同一transactionで行う。境界時点のevent snapshotから再構成し、最新rowの値で上書きしない。
- 後日feedbackでlabelRevisionを更新。重複実行時のunique制約を利用する。
- 試験: `rr_31_crash_before_checkpoint`、`rr_31_late_feedback`、`rr_31_same_page_twice`。V1。
- 合格: 失敗による欠落/二重例0。

### RR-32 ラベル生成

- 契約: L2。N `rr/learning/labels.rs`、`rr/learning/labeler.rs`。
- explicit/verified/model_inferredの区分、unknown、矛盾、superseded除外を実装。
- optional local labelerはschema validatedで別列へ保存。クラウドラベラーは初期実装しない。
- 試験: `rr_32_silence_not_success`、`rr_32_cancel_not_failure`、`rr_32_conflict_excluded`。V1。
- 合格: HTTP成功やreviewer意見だけを正答labelにしない。

### RR-33 dataset組立とexport

- 契約: L1/L4/L6。N `rr/learning/dataset.rs`、`rr/learning/export.rs`。
- group単位のsplit、時系列境界、本文なしJSONL、manifest/hashを作る。
- .tmpからrename、manifest最後、DB ready条件を実装。write失敗でcompletedにしない。
- 試験: `rr_33_group_split`、`rr_33_no_future_features`、`rr_33_partial_file_not_ready`。V1。
- 合格: 同じsource版から同じexamples digest（時刻・datasetIdはdigest対象外）。訓練/評価のgroup重複0。

### RR-34 夜間schedulerと手動tick

- 契約: L3/L8。N `rr/learning/scheduler.rs`、`rr/learning/runner.rs`。M app setup/shutdown登録。
- FakeClockでnight window、idle、sleep catchup、DST、foreground pauseを実装。既存background中断と接続する。
- 手動run once IPCも同じgateを通す。別DB writerや外部cronは作らない。
- 試験: `rr_34_missed_night`、`rr_34_clock_rollback`、`rr_34_foreground_preempts`。V1。
- 合格: 1日1job、foreground開始で夜間推論を中断、未実行が成功扱いされない。

### RR-35 集計・shadow ranker

- 契約: L5/C3.3。N `rr/learning/statistics.rs`、`rr/learning/shadow.rs`。
- 20例の下限、Beta平均、latency/cost欠損を処理。shadow推薦を保存して実dispatchは変えない。
- decision後の結果を同decisionのranker入力に入れない。
- 試験: `rr_35_small_sample_rules`、`rr_35_shadow_no_second_call`、`rr_35_missing_cost`。V1。
- 合格: shadow有効/無効でreal adapter呼出回数が同じ。

### RR-36 artifact loader・offline評価

- 契約: L6。N `rr/learning/artifact.rs`、`rr/learning/evaluate.rs`。
- JSON artifactのhash/feature/candidate fingerprint検査とlinear-v1の純粋推論を実装。
- fixture weightsで採点し、未選択結果の捏造を拒否。offline toolはmockのみ。
- 試験: `rr_36_invalid_hash`、`rr_36_new_model_cold_start`、`rr_36_no_counterfactual_labels`。V1。
- 合格: 未知artifactや無効artifactはrulesへ。Python等の任意コード実行0。

### RR-37 削除・失効・学習状況UI

- 契約: L7/L8。枝番a=削除、b=UI。N `rr/learning/invalidation.rs`、`src/features/settings/RoutingLearningSection.tsx`。
- M 既存会話削除/forget hook、settings page。source削除とartifact失効を同一writerへ接続する。
- filesystem削除再試行、last success/paused/error/件数表示を実装。
- 試験: `rr_37_forget_invalidates_before_dispatch`、`rr_37_cleanup_retry`、UI missing/failed表示。V1/V3。
- 合格: 元記録削除後のartifact採点0、本文をerror表示しない。

### RR-38 tool-specialist差し替え試験

- 契約: C2/C8/L0。N `rr/recipes/tool_specialist.rs`、対応fixture。
- tools担当roleが設定されている場合の有限recipeを追加。親が目的を指定し、specialistはhost tool requestだけ返す。
- Needle3の実接続は未検証のままmockで契約を確認。結果解釈と最終回答は親reasonerへ戻す。
- 試験: `rr_38_specialist_same_gate`、`rr_38_specialist_no_final_authority`、`rr_38_specialist_disabled`。V1。
- 合格: specialistを替えても認可/ledger/voice ownerが変わらない。

### RR-39 全体gate・性能・報告

- 契約: 全受入A01〜A42、性能P1〜P5。N `spec/evidence/role-routing/results.md`、実機評価fixture/runner。
- V5を実行。fake統合、liveモデル接続、音声体感、夜間負荷を別々に記録する。
- git diff、未完カード、unsupported adapter、未確認model、学習データ不足を明示する。
- 試験: V5/V6、全acceptance、local backend負荷測定。
- 合格: 全要求→カード→試験→証跡が辿れる。R3完了を学習モデル本番採用と表現しない。

## 検証command

### V1 カード別Rust

```sh
# NNは実装カード番号。0 testsの場合は失敗扱い。
cargo test --locked --manifest-path src-tauri/Cargo.toml --lib rr_NN_
cargo fmt --check --manifest-path src-tauri/Cargo.toml
```

### V2 接続先の回帰

```sh
cargo test --locked --manifest-path src-tauri/Cargo.toml --lib runtime::context
cargo test --locked --manifest-path src-tauri/Cargo.toml --lib providers::chat_completions
cargo test --locked --manifest-path src-tauri/Cargo.toml --lib tool_selection
cargo test --locked --manifest-path src-tauri/Cargo.toml --test sqlite_architecture
```

### V3 Frontend / sidecar

```sh
bun run typecheck
bun test tests/role-routing-session.test.ts tests/role-routing-codex.test.ts
```

V3のファイルはRR-14/19で追加する。未追加段階は存在する対象だけ実行し、未実施を記録する。

### V4 生成物と構造

```sh
bun run ipc:generate
bun run ipc:check
bun run s11tnext:check
bun run size:register
bun run size:check
```

size:registerは追加ファイルだけを確認し、上限を緩めない。IPC生成物を手編集しない。

### V5 milestone全体

```sh
bun run check:local
bun run test:rust-packages
bun run desktop:smoke
```

失敗が既存原因でもログと再現commandを残し、全体合格と記載しない。文書変更だけの本計画改訂ではこれらのランタイム試験は不要。

### V6 Live lane（実装時に追加する試験）

```sh
cargo test --locked --manifest-path src-tauri/Cargo.toml --lib rr_live_ -- --ignored --nocapture
```

live fixtureは空の一時profile、限定tool、許可されたモデルを使い、既存個人会話・別Codexタスクを操作しない。mode、model ID、CLI/SDK版、sample数、失敗数、TTFA/回答latencyを証跡に残す。

## 実装担当へ渡す依頼例

「RR-16のみ実装してください。C5の追加入力barrierを守り、分類結果が出る前の旧回答を発話しないでください。旧start_turnと非Routing経路は変更しません。resultが分類より先に返るfixture、status質問でcancelされないfixture、unclearで結果が保留されるfixtureを追加し、V1の件数と結果をprogress.mdへ記録してください。」
