# Role Routing 実行契約

状態: 設計。本文中の新規型・table・IPCは未実装。
[全体計画](saaa-role-routing-plan.md) / [作業カード](saaa-role-routing-work-cards.md) / [受入試験](saaa-role-routing-acceptance.md)

## C0. 所有権と不変条件

- `role_routing`をcrate直下のmoduleとして追加する。`AppState.role_routing`はactor registry、接続adapter、時計、通知fan-outを保持する。永続データの正本はSQLite。
- conversationごとに一つのactor。純粋な`reduce(snapshot, event) -> Transition`と、IOを行う`driver`を分ける。TransitionはDB変更と開始・中止・発話などのEffectを返す。DB commit後にのみEffectを実行する。
- rootの終了状態は既存`runtime_runs.status`だけを正本にする。新root行には現在のphase・revision・active slotを置くが、completed/failed等の別statusは作らない。
- rootに紐付く推論stepは最大1つが実行中。frontend/classify用stepは共有の別枠で最大1。評価と修正は逐次。ツール呼び出しも同じroot内で逐次。
- 入力受付、revision更新、tool permit、結果採用、cancel受付は同じactorで順序づける。外部IO中はactorを塞がず、完了をイベントとして戻す。
- actor内部でDB transactionや同期Mutexを保持したままawaitしない。
- UI切断はcancelではない。明示中止、アプリ終了、期限切れだけをcancelとして扱う。再接続はsnapshotとevent cursorから復元する。
- 権限・scope・送信先はhostが決定する。モデル出力やWorldFrameの文言を認可として扱わない。

## C1. 語彙・識別子・型

全JSONはcamelCase、`schemaVersion:1`、未知field拒否。列挙はsnake_case。IDは既存`validate_identifier`の上限・文字集合を使う。JSON内のfloatはfiniteのみ。本文サイズはUTF-8 bytesで検査する。

| 型 | 必須field / 意味 |
| --- | --- |
| RoutingInput | inputId、conversationId、content、origin(text/voice)、sourceId?、targetRootId?、targetAnswerId?、intentHint(auto/update/new_task/cancel/approve/decline)、scopeRefs、presentationMode |
| InputReceipt | inputId、messageId、rootId?、eventSeq、disposition(accepted/queued/needs_clarification/duplicate)、revision? |
| RootSnapshot | rootId、conversationId、runtimeStatus、phase、revision、policyVersion、activeStepId?、pendingProposalId?、queueLength、lastEventSeq |
| RoutingSignal | kind、targetAnswerId?、evidenceMessageId、evidenceStart、evidenceEnd、confidence、source(host/frontend/reasoner)、extractorVersion |
| Candidate | candidateId、recipeId、actorIds、requiredCapabilities、eligibility、exclusionReason?、estimatedLatencyMs?、estimatedCostMicros? |
| Decision | decisionId、rootId、revision、triggerInputId、signals、candidate list、selectedCandidateId?、rankerVersion、policyVersion、reasonCodes |
| StepRequest | stepId、rootId、revision、purpose、actorId、policyVersion、contextManifest、deadlineMs、remainingToolCalls、toolPolicy |
| StepResult | stepId、revision、status、answer?、review?、escalation?、usage、errorCode? |
| Usage | inputTokens?、outputTokens?、cachedInputTokens?、estimatedCostMicros?、costSource?、elapsedMs |
| SourceRef | kind、id、version、digest、scopeKey。本文を重複保存せず再取得に使用 |
| Review | targetAnswerId、verdict(supported/issues_found/insufficient_evidence)、issues最大8件、unresolved最大8件 |
| ReviewIssue | kind(condition/logic/evidence/calculation/tool_result)、claim、evidenceRefs、severity(minor/major)、verification(verified/unverified) |
| Escalation | reasonCode、unresolved最大8件、requestedRoleId。actor/modelの直接指定は不可 |

Step purposeは`respond/reconsider/review/revise/frontend/classify/tool_specialist`。rr_stepsのlifecycle statusはplanned/running/draining/succeeded/failed/cancelled/interrupted。結果statusは`succeeded/failed/cancelled/interrupted`。旧revisionの結果も実際のstatusで保存し、採否はoutput.accepted=falseとsuperseded eventで表す。reviewは他の回答を検証する作業であり、新たな最終回答を自動で作らない。

初期Action enumは`respond/explain/clarify/reconsider_same/reconsider_other/review_other/revise/propose_upgrade/finalize/cancel`。新しいActionをコードなしで増やすことはできない。設定で変更できるのは既存Actionの組合せ、担当、条件、予算。新しい実行primitiveにはコードとschemaVersion更新が必要。

## C2. 設定契約

新しいsettings documentを`namespace=routing.roles,key=default,schema_version=1`として追加。既存`routing.tasks`は維持する。全設定はvalidation後に同一transactionでrevisionを増やし、`rr_policy_versions`へimmutable snapshotを作る。認証キーは既存Keychainから参照し、設定・snapshotに保存しない。

```json
{
  "schemaVersion": 1,
  "enabled": false,
  "actors": [
    {"id":"front","label":"会話モデル","aliases":["1.2B"],"transport":"provider","providerId":"front-local","model":null,"location":"local","resourceGroup":"front-gpu","maxInputBytes":16000,"capabilities":["classify","social_reply"]},
    {"id":"primary","label":"通常推論","aliases":["Qwen"],"transport":"provider","providerId":"reasoner-local","model":null,"location":"local","resourceGroup":"reasoner-gpu","maxInputBytes":64000,"capabilities":["reason","review","host_tools"]},
    {"id":"advanced","label":"高度推論","aliases":["Sol"],"transport":"codex_sdk","providerId":null,"model":"gpt-5.6-sol","location":"cloud","resourceGroup":"codex","maxInputBytes":64000,"capabilities":["reason","review","host_tools"]},
    {"id":"premium","label":"追加検証","aliases":["Astra"],"transport":"codex_sdk","providerId":null,"model":"gpt-6-astra","location":"cloud","resourceGroup":"codex","maxInputBytes":64000,"capabilities":["reason","review","host_tools"]}
  ],
  "roles":{"frontend":"front","reasoner":"primary","advanced":"advanced","reviewer":"primary","premium":"premium","toolSpecialist":null},
  "recipes":[
    {"id":"direct","action":"respond","roles":["reasoner"],"enabled":true},
    {"id":"advanced_rethink","action":"reconsider_other","roles":["advanced"],"enabled":true},
    {"id":"cross_review","action":"review_other","roles":["reviewer","advanced"],"enabled":true},
    {"id":"premium_proposal","action":"propose_upgrade","roles":["premium"],"enabled":true}
  ],
  "limits":{"maxReasoningSteps":4,"maxToolCalls":32,"rootTimeoutMs":180000,"stepTimeoutMs":60000,"frontendTimeoutMs":1200,"classificationTimeoutMs":1500,"maxQueuedInputs":4,"maxReviewRounds":1,"maxAutomaticSwitches":2,"maxEstimatedCostMicros":null},
  "speech":{"mode":"author_verbatim","ackDelayMs":250,"maxAckChars":80,"progressMinIntervalMs":15000,"maxProgressPerRoot":2},
  "selection":{"mode":"rules","shadowArtifactId":null,"classificationMinConfidence":0.85,"weights":{"quality":0.6,"latency":0.25,"cost":0.15},"switchMargin":0.15},
  "premiumApproval":"per_request",
  "learning":{"enabled":false,"localStart":"02:00","localEnd":"05:00","idleSeconds":300,"maxRunSeconds":600,"batchSize":100,"allowLocalLabeler":false}
}
```

これは有効化前の記入例。provider IDは実環境から選択する。Sol/Astra IDも実アカウントで利用できることをgateで確認し、利用不可ならactor healthはunavailable。役割を別actorへ割り当てられるが、review_otherでは回答作成actorと異なるactorを必須とする。

Validation規則:

- actor最大16、recipe最大32、ID重複不可。labelは80文字以下、aliasesは最大8件・各40文字以下、正規化後の別actor間重複不可。ユーザーの明示担当指定はaliasesからhostがactor IDへ解決する。role参照・recipe参照は全て解決可能。recipeの順序は最大3step、既知テンプレートのみ。
- provider transportのmodelはnullで、既存Providerのmodelを使う。locationの矛盾は拒否。Harnessが動的に違うモデルを返す構成はR1で受理せず、役割が固定できるOpenAI互換Providerを登録する。
- `reasoner`必須。frontend不在なら定型文で継続。advanced/premium不在なら該当recipeを除外し、理由を表示する。
- capabilitiesは自己申告だけで有効にしない。adapterが実装する能力との積集合を使う。read-only reviewはhost_toolsを持っていてもmutationを許可しない。
- maxReasoningStepsは1〜8、maxToolCallsは0〜32、root timeoutは1000〜600000ms、step timeoutは1000〜root timeout、frontend/classificationは100〜3000ms、queueは1〜8、review roundsは0〜2、switchesは0〜4。
- classificationMinConfidenceは0〜1、初期0.85。weightsは0以上、合計1。cost不明は0円にしない。金額上限を指定したactorで単価不明なら自動実行候補から除外。
- `author_verbatim`だけを初期実装する。未実装speech modeは保存を拒否する。
- schemaVersion不明、cycle、未知Action、任意script/URLのrecipeは拒否。CAS不一致の設定保存はconflict。
- 現在の設定と同一digestの保存はno-opとしversionを増やさない。過去版へのrollbackは同じdigestでも新しいversionを採番する。
- enabled=falseの初期文書はactors/roles/recipesが空でもよい。enabled=trueへの変更時には上記必須項目とprobe fingerprintを検査する。

## C3. 判定手順と初期ルール

順序は変えない: hostの中止/承諾処理 → 入力の対象確定 → signals → 候補生成 → hard filter → rules/ranker → 継続性 → DB decision → dispatch直前再検証。

cloudAllowedは、root開始時とdispatch時の両方で既存security.local_only_when_selectedと選択されたprimaryのlocationから計算する。初期primaryがlocalかつlocal-only=trueならS/Aを除外する。model labelやranker scoreでこの条件を変更しない。

### C3.1 入力の分類

frontendには直近のユーザー入力、直近確定回答のIDと短い抜粋、active rootの目的・phaseだけを渡す。tool結果やworkspace全体は渡さない。最大16KB。schemaはkind、targetAnswerId、evidenceStart/End、confidence、replyKey?のみ。kindはnew_task/add_constraint/status_query/social/answer_challenge/explanation_request/explicit_actor_request/unclear。

confidence>=policy.classificationMinConfidence（初期0.85）、evidenceの範囲が入力本文内、targetがhostから渡したIDのいずれか、の3条件が揃っても権限判断には使わない。閾値はpolicy版に保存する。失敗・timeout・引用/否定の曖昧性はunclear。閾値を下げてlive gateを通さない。

即時host操作はUIの明示intent、または正規化後の単独「中止して」「キャンセル」で対象rootが一意の場合だけ。引用を含む長文からkeywordだけでcancelしない。承諾も提案ID付きUI操作、または唯一の未失効提案に対する単独「はい、Astraでお願いします」等のallowlist。単独「はい」は自動承諾しない。

### C3.2 初期選択表

| signal / 状態 | 行動 | fallback |
| --- | --- | --- |
| 新しい実質依頼 | reasonerでrespond | reasoner不可なら設定エラー。frontで回答しない |
| allowlist完全一致の挨拶・お礼のみ | frontendでsocial reply | host定型文 |
| 説明要求で対象回答確定 | 同actorでexplain、toolsはread-only | actor不可ならreasonerに元回答と根拠を渡す |
| reasoner回答への明確なchallenge | advanced_rethink | cloud禁止/不可ならreconsider_same |
| advanced回答へのchallenge | reviewerでreview→必要ならadvancedでrevise | 独立reviewerなしなら確認質問。独立評価を偽称しない |
| reviewerがverified major issueを報告 | authorでrevise | author不可なら許可されたreasoner |
| unverified issue / insufficient_evidence | 根拠取得を1回行うかclarify | 同じ評価loopを無限に回さない |
| review後もmajor issue未解消、または明示premium要求 | premium proposal | 利用可否不明なら未確認と表示。実行はしない |
| active rootへの条件追加 | C5のrevision更新 | 対象不明ならclarify |
| active rootへのstatus/social | 現処理を維持し、frontで状態案内 | host定型文 |
| active root中の別依頼 | queue | 上限ならbusy、入力を受付済みにしない |
| 「本当に？」のみ | 確認の意図としてclarifyか根拠説明 | 自動で低評価labelにしない |

ユーザーが「そんな結論でよい？」と直前の結論を明示するfixtureはanswer_challengeとして扱う。意味判定のlive精度は受入仕様で別測定する。ML導入後も明示cancel・承諾・hard filterはranking対象外。

### C3.3 ランカー境界

`Ranker::rank(&RankingInput) -> Result<Vec<ScoredCandidate>, RankError>`は副作用なし。候補IDは入力集合と完全一致、重複/NaN/未知IDは全結果を破棄してrulesへ戻す。初期scoreはルール優先順位から決定し、同点はrecipeIdの昇順。切り替えは新候補scoreが現担当+switchMargin以上か明示再考のときだけ。tools実行中はscoreに関係なくdispatchを待つ。

## C4. コンテキストとプロンプト

`ContextManifest`は既存context_generationsとinputsを使う。rootの初回run scopeを上限として、各revisionのユーザー入力を順序付きで付加する。追加入力のscopeRefsで権限を広げない。別projectの要求はnew_taskにする。

現在の`conversation_inputs::load`はruntime_runs.input_message_id基準なので、そのまま使い回さない。`runtime/context/role_projection.rs`を追加し、既存load/brokerを共有するhelperに明示的なinput message境界を渡す。root初回入力と追加条件はMust扱い。任意記憶は既存scope/epoch検証後に入れる。

入力envelopeの順序: 共通personaとrole instructions → 制約と当該役割の能力 → original user request → accepted amendments → relevant history → evidence/tool results → 未解決点。引用・ツール結果はuntrusted dataとして囲む。rootを変えたときに前のrootの制約を無条件に継承しない。

promptは`contexts/`配下にfrontend/classifier/reasoner/reviewer/reviserの定義を追加し、既存s11tnext生成手順で管理する。必須指示は次のとおり。

- frontend: allowlistの短い会話のみ直接応答。完了・調査・実行の断定はhostが渡したstateに限定。
- reasoner: 条件を保持し、ツール結果と推測を区別。能力不足なら構造化したEscalationを返す。
- reviewer: author名や性能tierを見せず、回答・依頼・根拠からissueを抽出。レビュー自体に最終回答の権威を与えない。
- reviser: issueごとの採否と根拠を内部結果に残し、正当な根拠がない結論反転をしない。

WorldFrameは既存M3のdispatch禁止を迂回しない。R1ではhostが得られる実行中/idle/unknownの特徴のみ。Frame本文をLLMへ入れるのは別途既存World接続gateが完了している場合だけ。Frame失効時はunknown、permissionやtask完了に変換しない。

## C5. 永続状態と競合

root phaseは`queued/classifying/planning/thinking/reviewing/revising/draining/awaiting_user/finalizing/idle`。runtimeStatusがterminalならphaseはidle、active_slot=false。queuedから開始する際にdeadlineを設定し、queue待ち時間は思考予算に含めない。awaiting_userはroot期限を延長せず、期限でinterruptedにする。

| 現在 | イベント | DB変更 | commit後Effect |
| --- | --- | --- | --- |
| なし | new input | message/root/input/event作成 | classifyまたはplan |
| thinking等 | accepted amendment | revision+1、input追記、旧stepにcancel_requested、phase=draining | child cancel、未開始speechを無効化 |
| draining | old result | stepの実結果を保存、採用はsuperseded | tool settle後に最新revisionをdispatch |
| thinking/reviewing | current result | step/resultを保存 | review/revise/finalizeを選択 |
| 任意active | cancel | rootの採用禁止、phase=draining | 子cancel、speech停止 |
| draining（amendment） | 全child確定 | phase=planning、runtimeはrunningのまま | 最新revisionをdispatch |
| draining（cancel/error） | 全child確定 | runtime cancelled/failed、active_slot=false | queued root開始 |
| finalizing | accept | revision一致確認、回答保存、runtime completed、root active_slot=falseを同一transaction | 確定回答を発話予約 |
| 任意active | restart | runtime interrupted、未終了step interrupted | 自動再実行しない |
| idle/terminal | feedback | 新root、targetAnswerIdと旧rootへの参照 | 新しい再考recipe |

受付前に、当該conversationのqueued rootsと未分類inputの合計をmaxQueuedInputs以下に予約する。auto入力も分類前に1枠を確保する。cancel/承諾は別枠。分類結果がnew_taskなら予約をqueued rootへ移し、追加条件/status/socialなら解放する。受け付けた後でqueue満杯としてmessageを取り消さない。

入力受付commit時、rr_inputs.resolutionをpendingにして即座に採用barrierを立てる。pending/clarification_pendingが1件でもあれば旧結果の採用と新しいtool permit発行を停止する。phaseをclassifyingにしても既存推論は分類確定まで停止しない。結果が先に届けばrr_outputsへ保存して保留する。status/socialと解決した場合はbarrierを外し保留結果を再検査、条件追加ならrevision更新、曖昧ならclarification_pendingとして確認を待つ。確認の返答inputがどの保留inputを解決したかrr_inputs.resolves_input_idへ保存する。分類は受信順で行い、先行する不明入力を飛ばして後の結果を採用しない。

actor event順序が最終回答の採用commit→次入力受付commitなら、回答を保持し追加条件は新rootになる。入力受付commit→最終結果ならbarrierにより旧結果を保留する。単にProvider結果が届いただけでは採用済みとしない。順序の違いをテストで明示する。

cancelと明示的なproposal承諾は分類barrierを待たない。条件追加ではchild cancelのみ、rootのcancel_requestedは立てない。drain_reasonをamendment/cancel/errorとして保存し、原因ごとの遷移を区別する。deadline到達時は保留を解除して回答するのではなく、子をcancelして結果を不採用とし、rootをinterruptedとして説明する。unknownなtoolが残ればC5末尾のmutation禁止を保持する。

`accept_result` transaction条件はruntime.status=running、active_slot=1、revision一致、stepがcurrent、cancel_requested=0、未解決inputが0、scope epoch有効。満たさない場合に回答messageを作らない。既存`persist_conversation_success_with_state`の中に別writerを入れない。transactionを受けるhelperへ分解し、message IDを返してroot.result_message_idとeventを同一transactionで保存する。

tool permitはactorがrevision/cancel/権限を検証して発行し、dispatch直前に再検証する。permitにrootId/stepId/revision/operationKey/expiryを含める。更新とpermitのcommit順を正本とする。permit前に更新された操作は0回実行。permit後に更新された操作はcancelを要求するが、実際に実行された可能性を残す。

再開時にrunning toolが残る場合はownerの実績を照会する。remote outcome不明はinterrupted_unknown。新しい同種mutationは拒否し、状態確認を要求する。unknownなmutationはroot終了後もconversationと対象resourceに対するdispatch blockerとして既存invocation ledgerから照会し、新rootへの乗り換えで迂回させない。「一度だけ実行」をremote systemの保証なしに主張しない。

## C6. SQLite契約

実装先`role_routing/schema.rs`と`schema.sql`。既存migration transaction内から一度呼ぶ。時刻は新tableでUTC Unix ms INTEGER、既存のTEXT時刻はadapterで変換する。各JSON列にjson_valid、ID/FK/enumにCHECKを付ける。以下は列を省略しない論理schema。`?`はnullable、それ以外はNOT NULL。ID、enum、digest、*_jsonはTEXT、revision/version/ordinal/seqと*_msはINTEGER、confidenceはREAL、booleanはINTEGERの0/1。型に迷う列を独自にTEXT化せず、この規則と対応型を合わせる。

| table | 列 | 主キー・一意制約 |
| --- | --- | --- |
| rr_policy_versions | id, version, config_json, digest, created_at_ms | PK id、UNIQUE version、digestは非UNIQUE index |
| rr_roots | root_id, conversation_id, policy_id, revision, phase, active_slot, origin, presentation_mode, started_at_ms?, deadline_at_ms?, cancel_requested, drain_reason?, result_message_id?, previous_root_id?, target_answer_id?, scope_digest | PK root_id→runtime_runs、conversation→conversations、policy→versions、message refs→messages |
| rr_inputs | input_id, conversation_id, root_id?, message_id, source_id?, kind, resolution, resolves_input_id?, revision?, target_answer_id?, payload_digest, disposition, received_at_ms | PK input_id、message_id UNIQUE、root/conversation/message FK、conversation+source_id partial UNIQUE where source_id non-null |
| rr_decisions | id, root_id, revision, input_id?, features_json, candidates_json, selected_id?, action, reason_codes_json, ranker_version, policy_id, created_at_ms | PK id、root/input/policy FK |
| rr_steps | id, root_id, decision_id, revision, ordinal, actor_id, purpose, status, cancel_requested, config_fingerprint, adapter_state_json, started_at_ms?, completed_at_ms?, error_code?, usage_json, context_generation_id? | PK id、UNIQUE(root_id,ordinal)、root/decision/generation FK |
| rr_outputs | id, step_id, kind, payload_json, digest, accepted, created_at_ms | PK id、step FK CASCADE、UNIQUE(step_id,kind) |
| rr_tool_links | root_id, step_id, revision, operation_key, invocation_id?, dispatch_state, result_ref?, created_at_ms | PK(root_id,operation_key)、step FK、invocation FK→tool_selection_invocations（SET NULL） |
| rr_events | seq, root_id?, conversation_id, revision?, kind, payload_json, created_at_ms, dedupe_key | seq INTEGER PRIMARY KEY AUTOINCREMENT、dedupe_key UNIQUE |
| rr_feedback | id, target_answer_id, target_root_id, source_message_id, target_decision_id?, kind, evidence_json, label_source, confidence?, extractor_version, status, created_at_ms | PK id、UNIQUE(target_answer_id,source_message_id,kind,extractor_version) |
| rr_proposals | id, root_id, revision, candidate_id, unresolved_json, status, expires_at_ms, decided_input_id? | PK id、root FK、input FK |
| rr_speech | id, root_id, revision, kind, source_output_id?, reply_key?, reply_catalog_version, state, created_at_ms, started_at_ms?, ended_at_ms? | PK id、root/output FK |

追加index: rr_roots(conversation_id) UNIQUE WHERE active_slot=1、rr_inputs(root_id,received_at_ms)、rr_decisions(root_id,revision)、rr_steps(root_id,status)、rr_events(conversation_id,seq)、rr_feedback(target_root_id,created_at_ms)。active_slotは同じconversation内でqueuedを除くroot一つだけに1を立てる。queued rootはruntime runningでもactive_slot=0。

列の制約:

- revision>=1、ordinal>=1、boolean列は0/1。phase/status/kindはC1/C5/C7/C9のenum。
- rr_outputs kindはanswer/review/escalation/frontend/classification。payload最大64KB、review最大16KB、front/classification最大4KB。内部思考本文を保存しない。
- rr_roots.drain_reasonはnull/amendment/cancel/error。rr_steps.adapter_state_jsonはSDK threadIdなどのopaqueな継続識別子のみ。token、prompt、外部credentialは不可。
- rr_inputs.kindは初期unclassified、解決後C3.1のkindまたはcancel/approve/decline。resolution=pending/clarification_pending/resolved/rejected、resolves_input_idは同会話の先行rr_inputs FK。保留入力は最大4件で、cancel/承諾はこの上限と別に処理する。
- rr_inputs.dispositionはaccepted/queued/needs_clarification。duplicateは既存rowのreceiptを返すだけで新rowを作らない。
- rr_events.payloadはID、enum、数値とbounded reasonのみ、本文0。IPCで表示する本文は送信時にmessage/output参照から権限再確認して取得する。イベントは全選択、開始、完了、採用、破棄、発話、feedbackを記録する。tokenごとの保存はしない。
- rr_proposals status=pending/accepted/declined/expired、既定TTL=10分。root期限より長く生きない。
- rr_feedback status=pending/accepted/ambiguous/rejected、label_source=explicit_user/verified_outcome/model_inferred。target_decision_idは対象回答のproducer stepから解決し、曖昧ならnull。target_decision削除時はSET NULL。
- rr_tool_links dispatch_state=reserved/dispatched/settled/unknown、実行状態の正本は既存invocation ledger。リンク側を独立した成功判定にしない。
- rr_speech state=queued/playing/completed/cancelled/failed/interrupted。再起動時queued/playingはinterrupted。自動再生しない。

削除: conversation→root/input/event cascade、root→decision/step/tool_link/feedback/proposal/speech cascade、step→output cascade、参照回答message削除→feedback cascade、target_answer_id/result_message_idはSET NULL。previous_root_idはrr_roots FKでSET NULL。source_message_id削除はfeedback cascade、rr_inputs.message_id削除はinput cascade。decision.input_idはnullableにしてinput削除時SET NULL（C1のdecision作成時は必須）。policyは参照がある間保持。削除を検知したとき学習artifact失効も同一writer経由で行う（L7）。FK循環を避け、root→activeStep FKは作らず問い合わせで求める。

root以外のIPC操作（中止・承諾）も、利用者が行った操作として定型のuser messageを保存し、rr_inputsへ同じinputIdを紐付ける。隠れた新しい推論runは作らない。user messageのorigin metadataにUI操作由来を記録し、モデル生成文と区別する。

新規root作成は既存prepareの入力保存/scope解決helperを抽出して再利用する。`runtime_runs.route_kind`はconversation.respondを使い、新しい文字列をCHECKへ足さない。既存input_message_idはroot初回入力のまま。追加messageはrr_inputsで関連付け、scopeをattachする。

stepIdとrootIdの組、inputとtargetのconversationはrepositoryで一致を検査する。単一列FKだけで異なるrootのstepを結び付けられないよう、rr_steps(id,root_id) UNIQUEとrr_tool_links(step_id,root_id)複合FKを設ける。

再起動reconcileは既存runs処理後に実行し、runtimeがinterruptedになったrootのactive_slot解除、stepのinterrupted化、pending proposal失効を行う。中途半端なjobの自動resumeは禁止。

## C7. IPCとUI

R1段階ではpending分類barrierの型と保存を先に用意し、実行中の入力は別依頼queueへ送る。R2/RR-16完了後にautoの追加条件分類を有効化する。R1で未対応の途中更新を受理して黙って無視しない。

新規型は`role_routing/ipc.rs`にts-rsで定義し、既存export例とbindings登録へ追加。`src/lib/generated/roleRouting.ts`は生成物。既存RuntimeEventの意味を変えない。

| command | 入力 | 出力 |
| --- | --- | --- |
| submit_routing_input | RoutingInput | InputReceipt。DB保存後直ちに返す |
| get_routing_snapshot | conversationId | active root、queued roots、直近proposal、lastEventSeq |
| subscribe_routing_events | conversationId、afterSeq、Channel | live購読を先に登録し、上限seqまでreplay後にbufferを連結 |
| cancel_routing_root | rootId、inputId | 保存済みcancelのreceipt |
| respond_routing_proposal | proposalId、expectedRevision、accept、inputId | receipt |
| get_routing_capabilities | なし | configured/probed/unsupportedと理由、policyVersion |

イベントは`{seq,conversationId,rootId,revision,kind,data}`。kind=input_accepted/phase_changed/actor_changed/interim_reply/answer_committed/proposal_created/speech_changed/root_finished。answer_committedだけに確定messageを含む。interim_replyはreplyKeyと表示文を含むが、最終assistant messageとして保存しない。

subscribeは同actor上で購読を登録してreplay上限Hを取得する。登録後のliveイベントを一時bufferに溜め、afterSeq<seq<=Hをpage送信後、seq>Hのbufferを順に送る。UIはseqで重複排除する。buffer上限256を超えた場合はresync_requiredを返し、snapshotから再購読させる。権限失効・削除済み本文は再送せず、content_unavailableとしてIDだけを返す。

UIはseqで重複排除し、run終了とspeech終了を別管理する。reloadでactive rootをsnapshotから戻す。新hookは`useRoleRouting.ts`、表示storeは`roleRoutingStore.ts`。既存`useConversationTurn`はenabled時のみsubmitを新hookへ渡す。音声のonSettled(true)はInputReceiptを得た時点で返す。回答完了までASR delivery queueを占有しない。

inputIdは再送で同一にする。同じIDでpayload digestが違えばconflict。同じsourceId/会話で重複したASR finalは元receiptを返す。queue上限到達時はmessage保存をせずbusyを返し、onSettled(false)。receipt喪失時は同じinputIdで再送できる。

mode切替時にactive rootがある場合はC0のdrainを待つ。legacyと新hookが同じ入力を同時送信しない。Chat composerは思考中も入力可。停止ボタンはroot cancel、音声停止ボタンはspeechだけを止める。

## C8. アダプターとツール

### C8.1 共通adapter

`ActorAdapter::run(StepRequest, ContextEnvelope, StepSink, childCancel) -> StepResult`。Sinkは内部の進捗だけを受け、rootのMessageCompletedを直接発行しない。stepはcontext_generationを開始/dispatch/完了する。Transport retryは内容送信前の接続失敗で1回まで。ツール実行後の自動再試行は0。

`resourceGroup`ごとに推論permit一つ。異なるgroupのfrontendとreasonerは並行。同groupならreasonerを優先し、frontendは待たずhost定型文へ。夜間推論は全foregroundに劣後する。既存Personal State slotとの接続はbackground中断の共有だけにし、新Qwen呼び出しで同じ非再入slotを二重取得しない。

### C8.2 Qwenと1.2B

Provider adapterは既存stream_model_providerとchat-completionsのHTTP/SSE/tool-loopを再利用する。新role contextをOptionで追加し、Noneの既存callerは従来動作。role contextがある場合、役割に応じたtool allowlist、root revision permit、捕捉Sinkを適用する。

frontend/classify/reviewはtools=falseまたはread-only offer。reviewで必要な根拠取得はhostが認可した読み取りtoolのみ。chat_completionsがcoding instructionを自動挿入する処理もrole制約と整合させ、frontendにcodingを提示しない。

通常Qwenはnative tool callingを使う。`request_escalation`はRouting内部用の追加toolとして定義し、外部実行せずEscalationを返す。名前/引数schemaを固定し、max1回/step、tool budgetに計上する。結果をpartial finalとして発話しない。

### C8.3 Codex SDK sidecar

新規`scripts/role-routing/codex-sidecar.ts`を固定SDK 0.144.4で実装。既存pi extensionを変更しない。`build.rs`のWebFetchと同じcompile方法でbundled binaryを生成し、SDKのcodexPathOverrideに同梱Codex binaryを渡す。PATH依存でユーザーの別CLIへ切り替えない。

JSONL v1: host→`{version,id,op:"run",stepId,model,prompt,outputSchema,timeoutMs}` / `{version,id,op:"cancel"}`。child→started/activity/result/failed/cancelled。全frameにversion/id/stepId。stdoutはprotocolのみ、stderrはbounded/redacted。line上限1MiB、累積8MiB、final text64KB、step timeoutはroot残時間とのmin。EOF before terminalはprotocol error。

runStreamedへAbortController.signalとoutputSchemaを渡す。`turn.completed`と妥当な最終JSONの両方が必要。`item.completed`だけで成功にしない。cancel後は新tool requestを受けず、2秒で停止未確認ならProcessGuardで子ツリーを終了する。killだけでremote副作用の取消成功としない。

初期SDKはhost-mediated tools方式: final JSONを`{kind:"answer",text}` / `{kind:"tool_request",name,arguments}` / `{kind:"escalation",reasonCode,unresolved}`に制限。tool_requestは同じtool gateで実行して結果を次のSDK turnへ返す。SDK内native shell、file write、web、MCPを直接使う経路は提供しない。

新しい隔離threadをroot+revision+actorごとにstartする。同一stepのtool-followupだけresumeする。旧Codex appタスクやcodex_threadsのIDを読んでresumeしない。thread IDはstep側のconfig_fingerprintに紐付くprivate adapter stateに保存し、異なるmodelへ流用しない。threadIdはrr_steps.adapter_state_jsonへ保存する。再起動ではresumeせずinterrupted。

workingDirectoryは空の専用runtime directory、skipGitRepoCheck=true、sandboxMode=read-only、networkAccessEnabled=false、webSearchMode=disabled、approvalPolicy=never。SDKの認証通信は許可される既存Codex認証を利用するが、LLMのツール通信とは分けて扱う。必要なworkspace内容は認可済みcontext/tool結果として渡す。

native tool無効化は固定CLIの実能力を確認する。features.shell_tool=false等のflag、継承MCPの明示無効化、空cwdだけで十分と断定しない。RR-20で偽Codex executableによる引数試験と、隔離fixtureでnative操作が実行されないlive試験を行う。隔離を証明できない版ではSDK actorをunsupportedとして止め、app-serverやAPIへの黙示代替をしない。

### C8.4 tool gate

既存`tool_selection`のsearch/describe/invokeを呼ぶ。外部MCP serverへHTTPで折り返さない。`RequestContext`はrootのprincipal/conversation/run/messageからhostが作る。modelからprincipal/project/runの引数を受けない。

gateway呼び出し前に共通`ToolPermit`を要求する。search/describeは読み取りだがscopeは毎回検査する。invokeのoperationKeyはhost発行のstepId+call ordinal。retryでは同じkeyを使用。args hashだけの重複排除は、正当な同内容の再実行を消すため使わない。

tool結果は既存resultRefを引き継ぐ。新版stepに見せる際はscope/ACL epochを再検証する。旧stepのmodelが同じ副作用を再提案した場合、既知のoperationを対応付けられなければ自動実行せず確認する。

coding_startのような既存の非gateway toolも同じpermit wrapperを通し、開始したjob IDを保存する。実装編集は既存coding job ownerへ任せ、SDK native file writesへ変更しない。未知のtool、未提示tool、tools=falseの要求は副作用0で拒否する。

## C9. 発話と軽量モデル

発話ownerはconversation単位。priorityはユーザー割り込み > cancel > current final > clarification > ack/progress。最終回答が来たら未開始のackを取消し、再生中のackはstopを要求して終了確認後にfinalへ。2秒で停止確認できなければそのrootの音声をfailedにして画面回答を残す。二つの音声を同時に始めない。

開始時に1.2BへreplyKey選択を依頼し、250msでまだ推論中ならackを一度予約。1200msのdeadlineは入力受付時から数え、1.2Bが間に合わなければhostの「確認します。」。実際にtool待ちでないのに「調べています」と言わない。progressは実際のphase変化時のみ、15秒以上の間隔、最大2回。音声が不要な設定ならUIのみ。

1.2Bの初期出力はreplyKeyを選ぶ方式で、greeting/acknowledge/thinking/checking_evidence/reviewing/waiting_confirmationのallowlist。自由文を発話に使わない。greeting/acknowledgeの直接完了は入力が挨拶等に限定されることをhost検証する。複合依頼「ありがとう、ただ条件が違う」は対象外。

音声の新しい発言を受け付けた時点で既存speechをbarge-in停止し、未開始の古いfinalを取消す。元の採用済みmessageは削除しない。rootがcompletedでもspeech ownerの停止処理は有効にする。

replyKeyの実際の文はversion付きのhost辞書に置き、rr_speechにreply_catalog_versionを保存する。再購読時に別の文へ変わらないよう、過去版の辞書を参照する。

最終回答はauthor_verbatim: 思考担当の確定textを共通TTSへ送る。全文が長い場合も1.2Bへ再要約しない。初期は画面・音声同文。structured speechSummaryの導入は別policy/schema拡張。

発話直前にもroot結果とrevisionを確認する。ただし既に採用・発話済みの文章は消去できないため、ユーザーの割り込みで再生を止め、その後の訂正として扱う。rr_speechに開始/終了/取消を記録し、TTFAと最終回答までの時間を区別する。

## C10. 不満・評価・上位提案

feedbackのtargetは明示answerId、なければ同じconversationの直近確定assistant回答で、その後別の実質的回答がない場合だけ。候補が複数ならclarify。異なるconversationのID、存在しないID、引用内だけの反応はrejected/ambiguousにする。

review結果のissues_foundは誤答確定ではない。issue.evidenceRefsをhostが検証し、計算やテスト等で確認できた項目だけverifiedにする。未検証issueはreviserへ仮説として渡す。userが「違う」と言ったことも客観的正誤とは分けて保存する。

premium proposalは未解決点、提案actor、想定の送信先、費用が不明ならunknownを表示する。「Solでは不可能」と断定しない。approvalはproposalId、revision、policyVersion、candidateIdに束縛する。承諾後にモデル・条件が変わったら新提案が必要。設定で既に許可された通常Sol委任に毎回承諾を求めない。

## C11. 障害分類

| 障害 | 動作 |
| --- | --- |
| frontend失敗 | 定型受付を使い、reasonerを継続 |
| classify不明 | activeならclarify、新規依頼ならQwen通常応答 |
| reasoner利用不可 | 明示失敗。frontの知識回答で代替しない |
| Sol容量/認証/能力エラー | 許可されたQwenで継続可能なら新decision、なければ説明。黙示モデル変更なし |
| context/scope失効 | 当該step出力を不採用。再取得1回、それでも失効なら停止 |
| tool結果不明 | unknownで保留。自動retryしない |
| DB書込失敗 | receipt/実行/採用を成功としない。発話前なら発話しない |
| TTS失敗 | 回答保存は成功のまま、音声だけ失敗 |
| UI切断 | 継続して保存。再接続はseqから復元 |
| process restart | 進行中をinterrupted。自動再実行・自動発話なし |
| ranker不正/停止 | rulesへfallbackし、その理由をeventに保存 |

## C12. モジュール境界と実装順序の注意

contracts/events/settings/selection/ranker/reducerはProvider IOを知らない。repositoryはDB transactionだけを扱う。coordinatorは順序とEffects、driverはadapter/tool/speechの開始と完了イベント、adapterはtransportだけを所有する。repositoryから直接TTSを呼ばず、adapterからrootを完了させない。

R1のtool gateはnative Qwen tool loopへの最小hook、R2のSDK tool loopは同じhookをhostから呼ぶ。外部MCP serverのsession/run生成規則を変更してRoutingに合わせない。tool_selectionやWorldの作業と競合した場合は現在のpublic boundaryに合わせ、別ledgerを作って回避しない。

nightly tableのmigrationはR3開始時にその時点のschema version+1として加算する。R1で出荷済みmigrationのSQLへ後からtableを追加して済ませない。全settings validatorの登録とsnapshot読取cacheの失効もRR-03で対応し、保存後に旧policyを新rootが使わないことを試験する。
