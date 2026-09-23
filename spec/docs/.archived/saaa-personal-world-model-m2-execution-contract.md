# Personal World Model M2A 固定実装契約

作成日: 2026-09-20。実装前の契約。全体の範囲は[M2計画](saaa-personal-world-model-m2-plan.md)、実行順は[詳細カード](saaa-personal-world-model-m2-work-cards.md)を参照する。

## R0. 配置・互換性

core = `crates/personal-state-core/src/world/`、adapter = `src-tauri/src/memory/personal_state/world/`。新規ファイル・関数は本書で予定として定義する。既存ファイルの確認時点はHEAD f5a1069。

| 責務 | ファイル・接続点 | 変更内容 |
| --- | --- | --- |
| Frameの型・純粋整形 | core/runtime_frame.rs（新規） | wire型・予算・Focus・stamp計算 |
| 要求とScopeの検証 | adapter/runtime_scope.rs（新規） | context/scope.rs::loadと既存scope表を読む |
| 会議ownerの安全なsnapshot | meeting/types.rs、meeting/mod.rs | WorldMeetingSnapshotとworld_snapshot()を追加 |
| 会議DB状態の参照 | adapter/runtime_meeting.rs（新規） | meeting_sessionsの最小列を読む |
| coding ownerの最小参照 | coding/world_snapshot.rs（新規）、coding/mod.rs | repository::authorizeを維持して状態を返す |
| codingのSource検証 | adapter/runtime_coding.rs（新規） | owner snapshotと既存Sourceの版・Projectを照合 |
| graph入力の組立 | adapter/runtime_graph.rs（新規） | 認可済み入力と残予算からactivate_v2を呼ぶ |
| 容量の入口検査 | adapter/runtime_capacity.rs（新規） | store::load前のbounded件数検査 |
| 取得・再検証 | adapter/runtime_frame.rs（新規） | prepare_frame / revalidate_frame。SqliteReadersを利用 |
| 外部根拠の適格性 | adapter/evidence_eligibility.rs（新規） | memory-recall-v1をtransient_onlyと判定 |
| 既存graph | adapter/query_v2.rs::activate_v2 | 内部呼出しを再利用。五要素の意味・schemaを変更しない |

WorldEntity / SourceKey / WorldSliceV2の保存形式、新しいSource role運用、DB schemaは変更しない。runtime_refはFrame内で区別された識別型。Graphのentity_idへ文字列を挿し込まない。新しいcrate・dependency・公開IPCは追加しない。

## R1. 型と上限

coreのwire型はSerialize / Deserialize、snake_case、deny_unknown_fields。IDは1〜160 UTF-8 byteかつ既存validate_identifierに通る形式、Scopeは既存のkind:id。制御文字、未知enum、数値overflowは拒否する。

```text
RuntimeKind = meeting_session | coding_job
RuntimeRef { kind: RuntimeKind, id: String }
RuntimePhase = starting | running | paused | stopping | terminal | unknown
MeetingLivePhase = idle | preflight | ready | active | paused | stopping | completed | failed
MeetingOwnerState = active | paused | stopping | completed | saved | failed | interrupted
CodingOwnerState = queued | running | cancel_requested | settled | failed | interrupted | outcome_unknown
RuntimeOwnerState = Meeting(MeetingOwnerState) | Coding(CodingOwnerState)
```

RuntimeOwnerStateのJSONは `{"kind":"meeting_session","state":"paused"}` のようにkind/stateを明示する。対象がdiscarded・消去済みならRuntimeStateViewを返さずnoticeにする。

| 型 | フィールド |
| --- | --- |
| RuntimeStateView | reference:RuntimeRef、scope_key:String、owner_state:RuntimeOwnerState、phase:RuntimePhase、job_revision:Option<u64>、current_run_id:Option<String>、reported_complete:Option<bool>、owner_digest:String |
| RuntimeFocus | reference:RuntimeRef、project_scope:String、reason:active_projectまたはcurrent_work |
| FrameNotice | code:FrameNoticeCode、reference:Option<RuntimeRef> |
| WorldFrame | schema_version=1、run_id:String、project_scope:String、captured_at_ms:i64、expires_at_ms:i64、graph:Option<WorldSliceV2>、runtime:Vec<RuntimeStateView>、runtime_focus:Vec<RuntimeFocus>、notices:Vec<FrameNotice>、truncated:bool |

RuntimeStateViewにはname、本文、token、workspace path、session path、PID、命令、result_jsonを持たせない。必要な表示名や文章はM3で認可済み参照から取得する。reported_completeはcoding ownerが返したbooleanの観測だけで、Goal達成や実際の成果保証ではない。

adapter側の内部型はSerializeしない。

```text
FrameRequest {
  run_id, project_scope, access: AccessRequest,
  runtime_refs: Vec<RuntimeRef>, graph_request: Option<GraphRequest>,
  max_bytes: usize, ttl_ms: u64
}
GraphRequest { seeds: Vec<WorldSeed>, causal_direction, limits: LimitsV2, flags: IncludeFlags,
               explicit_question: bool }
PreparedWorldFrame { frame: WorldFrame, request: OwnedFrameRequest,
                     stamp: FrameStamp, instance_id: String }
FrameStamp { ledger_revision, input_epoch, policy_revision,
             scope_digest, owner_digests, content_digest }
FrameValidity = current | changed | expired | scope_denied | unavailable
```

OwnedFrameRequestはAccessRequestの借用を所有文字列へコピーした内部struct。prepared生成時の認可を新たに作らない。graph_requestは既存queryの型を使い、runtimeの値をcondition_observationsへ変換しない。M2Aのgraph照会は両observations配列とtemporary_attention_entity_idsを空で呼ぶ。通常のv2 APIで既存会話観測を使う機能は変えない。

参照はkind/idで整列し重複排除してから上限8件。graph seedsは既存規則の正規化後4件。ttlは1〜1,000 ms、0はInvalidInput、上限超は1,000へ制限する。max_bytesは8,192とのminで、下限を引き上げない。未知RuntimeKindはunsupported_referenceで失敗し、別種のTaskへ読み替えない。

## R2. Scopeと認可

run_idは既存runtime_runsにある実行中runを指定する。架空のrequestをSourceとして生成しない。要求は信頼済みRuntime側から供給し、LLM出力にauthorized=trueを書かせない。

既存 `scope::load(c, run_id)` からrecorded ScopeSnapshotを読む。status=resolvedを必須にし、以下を同じDB snapshotで確認する。

1. AccessRequestのprincipal / scope=primary / policy_revision / authorizedとpersonal_scopeが一致。purposeはReasoning / Diagnostics、classificationはConfidential以上。この初期profileでは低classificationへの自動格下げをしない。
2. runtime_runsのstatus=running、input_message_idが同conversationの保存済みuser/transcript、sourceが削除されていない。
3. runtime_run_scopesに指定Projectがfocusまたはparentで存在し、現在context_scopesはactive。記録epochとcontext_scope_epochsが一致。
4. 各対象scopeが同runのfocus / current / parentのいずれかとして明示され、現在activeかつepoch一致。
5. Project→対象scopeに直接のcontext_scope_links（parentまたはowns）がある。任意の無向・推移的リンクから許可を推定しない。

meeting_session(id)のscopeは `resource:<id>`、coding_job(id)は `task:<id>`。対象scopeのkind / opaque_idも実値と照合する。既存scope::register / linkで明示登録されたもののみ。Worldのqueryからregister / link / resolveを呼ばない。resolveは書込みを行うため、この読取経路ではloadだけを使う。

要求のProject不明・曖昧・対象scope不許可はframe-scope-deniedで要求全体を拒否する。許可を確認してからownerのID検索を行う。許可済みIDがownerに存在しない場合はruntime_unavailableでその対象を省く。この区別から未許可対象の名前・存在を返さない。

M2Aには新たなScope登録UIを作らない。既存内部Scope登録がない会議・作業は使えず、全会議・全Taskを走査して代替しない。合成統合fixtureでは既存register / linkとscope::resolveで、この前提を明示的に作る。

## R3. 会議snapshot

meeting/types.rsに次を追加し、meeting/mod.rs::MeetingRuntime::world_snapshot()から既存Mutexを短時間lockして返す。

```text
WorldMeetingSnapshot { session_id: Option<String>, state: MeetingState }
```

adapterはMeetingStateをcoreのMeetingLivePhaseへ明示変換する。coreはmeeting moduleやTauriへ依存しない。

既存snapshot()の返却をそのままWorldへserializeしない。capture_token、entries、error.message等を含めない。world_snapshotはread-onlyで、stateを変更せず、DB・ネットワークへ触らない。

meeting_sessionsから読む列はid / status / started_at / ended_at / saved_at。文字列時刻は既存Unix ms文字列としてparseし、不正ならframe-owner-corrupt。前後のlive snapshotと合わせて判定する。

| DB状態 | live snapshot | 返却 |
| --- | --- | --- |
| active | 同id・Active | owner=active、phase=running |
| paused | 同id・Paused | owner=paused、phase=paused |
| activeまたはpaused | 同id・Stopping | owner=stopping、phase=stopping |
| completed / saved / failed / interrupted | 同idでActive/Paused/Stoppingではない、または別id/None | DB状態、phase=terminal |
| discardedまたは行なし | 任意 | 対象省略、runtime_unavailable |
| 上記以外の組合せ | 任意 | 対象省略、runtime_unstable |

preflight / ready / idleは会議が進行中という証拠ではない。activeのDB行だけが残りlive sessionがなければactiveとは返さない。再起動時は既存meeting::reconcileの結果を利用し、Worldがrecoverを起動しない。

owner_digestはcanonical配列 `['meeting_session', id, status, started_at, ended_at, saved_at, live_session_id_or_null, live_state]` のSHA-256小文字hex。同じ内容へ戻れば同じdigestでよく、イベント通番とは呼ばない。終端行を読む場合、別の進行会議またはNoneのlive情報はID/stateともnullへ正規化する。前後snapshot比較にも同じ正規化を使う。無関係な会議の状態変化で当該終端Frameを変えない。

## R4. coding job snapshot

coding/world_snapshot.rsに `read_world_snapshot(c, conversation_id, job_id) -> Result<CodingWorldSnapshot, String>` を追加する。既存repository::authorizeを利用する。新規の操作・cancel・continueを発行しない。

最初に該当jobのcoding_runs件数を上限33まで数える。32を超える場合はruntime_capacity_omitted。次にauthorizeを通し、coding_jobsとcurrent_run_idが指すcoding_runsから次の列だけを読む。

- job: id、conversation_id、source_id、revision、state、current_run_id。
- current run: id、job_id、source_id、state、delivery、ended_at。
- result: SQLiteのjson_validとjson_typeでbooleanと確認できるcompleteだけ。JSON全体を返さず、欠落・不正・非booleanはnull。

initialとcurrent、および過去run最大32件のsource_idについて、元会話messageの存在・同conversation・user/transcript、personal_sourcesの現在版とavailable、tombstoneなし、指定Projectへのpersonal_source_scope_refsを検査する。sourceの版・Project対応をowner_digestに含める。authorize失敗はruntime_unavailableとし、別conversationの内容を返さない。

| job state | phase | 注意 |
| --- | --- | --- |
| queued | starting | 実行中や成功としない |
| running | running | current runがrunningかつdelivery=acceptedの場合のみ。それ以外はunknown |
| cancel_requested | stopping | 停止要求であり、停止完了とは表現しない |
| settled / failed / interrupted | terminal | settledも成果の成功保証ではない |
| outcome_unknown | unknown | 完了・停止を断定しない |
| 未知文字列 | 対象省略 | runtime_unsupported_state。別状態へfallbackしない |

revisionが同じでもjob state / current_run_id / run state / delivery / completeが変わればowner_digestは変わる。canonical配列は `[kind,job_id,conversation_id,revision,job_state,current_run_id,run_state,delivery,ended_at,reported_complete,sorted_source_versions]`。source_versionsは `[id,version,project_scope]` のID順配列。本文・path・PIDをdigestの入力にも加えない。

## R5. 容量とgraph取得

runtime_capacity.rs::check_frame_capacityはrequest認可後、store::loadより前に呼ぶ。Projectのpersonal_world_entities / relations / focusは各表の該当Project行を合算して100件以内。全ledger記録は計画§6の六表合計2,000件以内。各件数は `SELECT COUNT(*) FROM (SELECT 1 FROM table WHERE ... LIMIT remaining+1)` で調べ、超過時に後続のtableを読まない。

プロファイル超過はgraph=None、notice=world_capacity_omitted、truncated=true。恒久保存上限や履歴は変更しない。Runtime読み取りは続けられる。計数結果には検査した行数をテスト用statsで残すが、他Projectの件数をFrame本文へ出さない。

容量内なら同じread snapshotで `activate_v2` を一回だけ呼ぶ。入力のproject/access/now/request_idは外側の認可済み値を使い、request_id=run_id。callerから別のAccessRequestやnowをgraph内部へ渡さない。graphのNode上限は `min(requested_nodes, 30 - 採用Runtime数)`。関連深さ4、因果深さ3、Edge60、paths10、fetch500 / scan500は維持する。

projection_stale / pending_reviewはgraph=Noneと対応noticeに変換する。graphの認可エラーはFrame全体の拒否へ伝播する。破損・SQLエラーを知識不足へ変換しない。空seedに対してProject/Goal名を捏造して照会しない。graph_request=Noneならgraphは取得せずgraph_not_requested。

Runtimeのactive / availableを、任意の既存Relationの条件成立に変換しない。条件とowner状態を結ぶ意味契約はM2B以降。M2Aでは二つを並べて返すだけである。

## R6. Frame構築とScope・状態の再検証

adapter/runtime_frame.rsに入口を置く。製品配線のない内部APIとし、通常のturns.rs / conversation_context.rs / Brokerへはまだ呼出しを追加しない。

```text
WorldFrameService::new(readers, meeting_reader, clock) -> WorldFrameService
WorldFrameService::prepare_frame(&self, request) -> Result<PreparedWorldFrame, FrameError>
WorldFrameService::revalidate_frame(&self, &prepared) -> Result<FrameValidity, FrameError>
```

readersは既存SqliteReaders、meeting_readerはWorldMeetingSnapshotだけを返す小さなtrait。製品implはMeetingRuntime::world_snapshot、試験はfake。clockはUnix msを返す注入可能な関数。instance_idはサービスの構築時に内部で一度生成するprivateなopaque IDで、callerに指定させず毎回生成しない。PreparedWorldFrameのrequest/stamp/instance_idもprivateにし、frameは読取参照だけを公開する。既存readersとArcのmeeting ownerを注入し、新しいAppStateフィールドや通常会話の配線は加えない。DBへ保存しない。再起動で変わる。

prepareの処理順:

1. 型・個数・byte・ttlを検証する。SqliteReaders::readでR2を確認し、Scope stampを取得する。
2. 対象に会議がある場合だけlive snapshotを読む。lockを解放する。
3. SqliteReaders::readでread transactionを張り、R2を再確認する。前回stampから変わっていればframe-changedを返す。同snapshotでRuntime正本、容量、graphを読む。
4. DB transactionが閉じた後、会議live snapshotをもう一度読む。対象のsnapshotが変わっていれば会議単位だけ除外しruntime_unstable。自動retryしない。
5. coreのassemble_frameで予算・参照整合性を確認し、stampと期限を付ける。処理終了時点で期限を過ぎていればframe-expired。

SqliteReadersの製品readはtransactionを張る。Serialized test backendは張らないため、整合性試験は一時ファイルをSqliteReaders::openで開いて行う。必要ならfixture側でread transactionを明示する。World専用connection poolを追加しない。meeting mutexをDB読取やawaitの間に保持しない。

FrameStampのscope_digestは許可対象のscope key / active状態 / 記録epoch / 現epoch / direct linkと、run_id / input_message_id / current instruction digestをcanonical配列にしてhashする。既存ScopeSnapshot.digestだけでは後からのlink変更を検出できないため、direct linkの存在を再照合する。

content_digestはWorldFrameからcaptured_at_ms / expires_at_msとgraph.as_of_msだけを除いたcanonical JSONのhash。graphのrevision、根拠、条件、notice、runtime owner_digestを含める。field順をstruct定義で固定し、Mapの任意順に依存しない。instance_idとrequest全体のfingerprintもPrepared内で照合する。

revalidateはinstance_id一致、同run/access、`captured_at <= now < expires_at`を先に確認する。その後同じ要求を現在時点で再読取し、ledger revision / input_epoch / policy / scope_digest / owner_digests / content_digestを比較する。時刻以外の差はchanged、認可の差はscope_denied、正本を読めなければunavailable。graphを容量で省略したFrameも、policy / scope / Runtimeを再検査する。ledger revision等のheaderはpersonal_scopeの最小列から読み、stamp取得のためにstore::loadを呼ばない。古いFrameを部分的に書換えてCurrentにしない。

frameの利用を許した直後にもownerは変化し得る。M2Aの結果は確認時点のsnapshotであり、生成中の保証ではない。M3ではdispatchと出力の境界に再検証を接続する。

## R7. 整形・Focus・総予算

Runtimeは入力RuntimeRefのkind/id順。phase=runningのcoding jobにcurrent_work、phase=runningの会議にactive_projectを付ける。paused / stopping / terminal / unknownには新規のactive Focusを付けない。runtime_focusは対応runtime viewと一緒に採否を決める。既存graph.focusのcurrent_work / explicit_interestを上書きしない。

Projectが存在するだけでactive_projectを生成しない。runtime_focusからGoalの採用・重要度・権限を変更しない。graph内のhas_goal / serves_goalは既存v2の認可・失効に従い、runtimeに対応するGoalを自動選択しない。

最初に空WorldFrameをserializeして固定費を測る。Runtime単位はview＋Focus＋対象noticeで採用し、合計最大2,048 byteの枠内へkind/id順で入れる。収まらない単位は全体を省略しruntime_budget_omitted。残り予算をgraphへ渡し、graph自体の上限は6,144 byte。Frame全体8,192 byteと要求max_bytesの小さい方を最終上限にする。

graph挿入に必要なJSON固定費も差し引き、組立後にserde_json::to_vecで最終byte長をassertする。noticeの追加で収まらなくなった場合はまずgraph全体を落とし、それでも超過ならkind/id降順にRuntime単位を落とし、根拠や条件だけを切らない。noticeはcode/referenceでdedup、最大16件。16件を超える場合はcode/reference順の先頭15件とnotices_truncated一件にする。noticeの余白は要求参照から作る最大16件の実際のJSON長で先に確保する。予算で対象を落とすたびnoticeを再計算し、有限回の削減で収まらなければframe-budget-too-small。

graph node数＋Runtime view数<=30。runtime_focusの参照先は採用runtime内にあること。graphの参照整合性は既存v2のまま。空Frameさえ収まらなければframe-budget-too-small。max_bytes=1の成功返却は禁止する。

## R8. 外部Evidenceの適格性

新しいcontextStillプロトコルを作らず、既存 `typed_recall::parse_call_tool_result` で受理したmemory-recall-v1が何に利用可能かだけ判定する。

```text
EvidenceEligibility = transient_only | persistent_eligible
EvidenceIneligibility = missing_stable_id | missing_revision | missing_revalidation |
                        missing_deletion_contract | missing_scope_proof
EvidenceAssessment { provider: context_still, contract: memory_recall_v1,
                     eligibility, reasons: Vec<EvidenceIneligibility> }
assess_contextstill_v1() -> EvidenceAssessment
```

現行の結果は必ずtransient_only、上記五つのreasonを定義順に返す。呼出元がboolを渡してpersistent_eligibleへ変えるAPIにしない。未知契約はframe-unsupported-evidence-contractとして拒否する。titleや本文hashをstable_id / revisionの代用にしない。

これは接続の能力判定であり、本文の内容が真実・安全・現在有効と判定する関数ではない。現行responseの検証を緩めず、metadataフィールドを勝手に追加受理しない。既存有効fixtureと不正sourceRef追加fixtureを試験へ使い、前者でも永続化は0件、後者は既存parserが拒否することを確認する。

M2Aはネットワークを呼ばず、WorldFrameへ外部本文を混ぜない。M3で既存recall結果をそのTurnの参考に使う場合も、永続根拠の資格とは分ける。persistent_eligibleは将来のenum予約値であり、M2Aのproduction実装から返してはいけない。

## R9. エラー・省略の対応

| 条件 | 結果 |
| --- | --- |
| 不正ID / enum / ttl0 | frame-invalid-input |
| 参照9件、seed正規化後5件 | frame-limit |
| request / scope / principal / purpose / policy不許可 | frame-scope-denied |
| Scope読取中の変更 | frame-changed |
| nowがcaptured_atより前、instance不一致、TTL到達 | revalidateはexpired。prepare期限到達はframe-expired |
| 不正時刻・必須列破損・DB破損 | frame-owner-corruptまたは既存database_error。誤って正常値を返さない |
| 許可対象の消去・owner authorize失敗 | runtime_unavailable notice、対象省略 |
| DBとliveが不整合 | runtime_unstable notice、会議単位省略 |
| coding state未知文字列 | runtime_unsupported_state notice、当該job省略 |
| coding run履歴33件以上 | runtime_capacity_omitted notice、当該job省略 |
| graph投影stale / pending | world_projection_stale / world_pending notice、graph=None |
| graph容量超過 | world_capacity_omitted、graph=None、truncated=true |
| 空Frameも予算超過 | frame-budget-too-small |
| unknown evidence contract | frame-unsupported-evidence-contract |

FrameNoticeCodeはここにあるsnake_caseと、R5のgraph_not_requested、R7のruntime_budget_omitted / notices_truncatedに限定する。例外文や本文をnoticeへ入れない。revalidateがchanged / expired等を返した場合、旧Frameを捨ててprepareし直すか、その回のWorld参照を省く。再検証を無制限retryしない。

## R10. テストfixture

共通値: owner principal、project:p、run1、user message msg1、now=1,000、ttl=1,000、expires_at=2,000。会議id=m1、scope=resource:m1。coding job id=j1、scope=task:j1。Projectとtargetにdirect link、run scopeにProject parentとtarget focus/currentを明示する。Objective=o1は既存会話Source付きActive、Goal=g1は既存v2 Entity。

- 999で再検証はexpired（時計巻戻り）、1,999で状態不変ならcurrent、2,000ならexpired。
- revision=7のままjob state running→cancel_requestedでchanged。
- 同じScope epochのままdirect linkを削除してもscope_denied。
- 会議live snapshotをbefore=Active / after=Pausedにしてruntime_unstable、active Focusなし。
- graphのObjectiveだけ期限切れならchanged。runtimeも含め旧FrameはCurrentにしない。
- 削除されたmsg1でrun context不許可。別Projectの同名対象の名称・状態0件。
- 100→101投影件数でgraph=None、store::load呼出し0。ledger 2,000→2,001でも同じ。
- source範囲・完了ログを一切読まないことをSQL列と返却JSONの両方で確認。

会議・codingの状態更新はfixtureでowner正本へ行う。Frame取得中にWorld用台帳へ状態を書かない。SQLのtotal_changesと主要表件数を取得前後で比較し、Frame取得・再検証・Evidence能力判定の書込み0件をassertする。
