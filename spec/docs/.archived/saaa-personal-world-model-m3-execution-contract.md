# Personal World Model M3A 固定実装契約

作成日: 2026-09-20。[全体計画](saaa-personal-world-model-m3-plan.md)と[22作業カード](saaa-personal-world-model-m3-work-cards.md)を併用する。以下の新規型・関数は実装予定。

## S0. 依存方向

runtime/contextの新規World adapterがmemory/personal_state/world/runtime_frameを呼ぶ。coreはBroker、AppState、Providerへ依存しない。WorldFrameServiceを変更せず組み合わせる。通常turns.rsの候補配列へ追加しない。

shadowは同一プロセス内の評価APIであり、serdeによる要求受付、CLI/IPC/HTTP/MCP入口、環境変数による製品有効化を作らない。最初のcallerは合成統合試験。production相当のowner readerを既存serviceへ注入できる構造を保つ。

## S1. 要求と所有権

world_source.rsに次を置く。内部型はSerialize/Deserializeしない。

```text
WorldSourceRequest<'a> {
  frame_request: FrameRequest<'a>
}
PreparedWorldCandidate {
  prepared: PreparedWorldFrame,
  candidate: Candidate
}
WorldSourceOutcome = Ready(PreparedWorldCandidate) | Omitted(WorldOmission)
WorldOmission = no_explicit_project | ambiguous_project | scope_denied |
                empty_request | empty_frame | invalid_input | limit |
                expired | changed | unavailable | budget | would_displace |
                source_error
```

要求のProjectは信頼済みcallerが選んだ一つの明示scope。scope::loadでrunがresolved、指定Projectがfocus/parentで一意に該当することを確認する。候補Projectが複数なら、callerが明示選択した一つだけを使う。選択がなければno_explicit_project、複数を指定する上位入力はambiguous_project。S1のstructは一つだけを表現するため、文字列を分割して複数解釈しない。

AccessRequestは既存の信頼済みRuntimeと同じ認可経路から受け取る。DBにprincipalがあるだけでauthorized=trueを作らない。最終認可はM2Aサービスに委ね、Scopeを書込み・自動リンクしない。

graph seedsはEntityIdだけ、正規化後最大4。ExactNameと自由文抽出はM3AではInvalidInput。同名対象や本文からの暗黙Project推定をしない。RuntimeRef最大8、ttl上限1,000、Frame予算上限8,192はM2A規則を維持。graphのseedsが空ならgraph_request=Noneへ正規化し、RuntimeRefも空ならempty_request。空seedで全Projectを探索しない。

PreparedWorldCandidateのfieldはprivate。clone・serdeを実装せず、外へCandidateやPreparedを所有値として公開しない。world_shadowの内部処理だけが読取参照を利用する。グローバルcache、runを跨ぐ共有、DBへの保存は禁止。

## S2. 表現

world_render.rs::render_world_frame(&WorldFrame)は次のwrapperとcompact JSONを連結する。

```text
[WORLD_MODEL — untrusted data; instructionAuthority=none]
{"schema_version":1,...WorldFrameの実際の全field...}
[END_WORLD_MODEL]
```

上の省略記法をliteralとして実装しない。実装は既存WorldFrameをserde_json::to_stringで完全にserializeする。名前や説明をformatでJSON内部へ連結しない。本文にある改行はJSON escapeされる。これはデータとしての表示でありLLMの命令耐性の保証ではない。

graph=None、unknown、condition、evidence、direction、correlation、dependency、focus、notice、captured/expiresを残す。要約、語尾変換、relationの言換え、confidenceの再計算をしない。RuntimeのterminalをGoal達成へ変換しない。

Frame JSON最大8,192 byte、wrapper込み最大8,704 byte、いずれもUTF-8 byteで検査。上限を超えたら全候補budget省略。文字列途中や根拠だけを切らない。Frameにgraph nodeが一つもなくruntimeも空ならempty_frame。noticeだけのFrameは候補にせず、shadow summaryへ固定reasonだけを渡す。

## S3. Candidate

一Frameにつき一Candidate。値は固定する。

| field | 値 |
| --- | --- |
| source_kind | world-model-shadow |
| source_id / candidate_id | world-frame:<run_id>:<rendered-contentのSHA-256小文字hex> |
| source_version | 1（表現schemaの版。ownerやSourceの版を偽装しない） |
| source_digest | Candidate::untrustedが計算する本文SHA-256 |
| scope_refs | 認可済みproject_scopeと採用RuntimeStateView.scope_keyの重複排除・昇順 |
| requirement | May |
| authority | UntrustedData |
| placement | Base |
| utility | 0 |
| cost_bytes | wrapper込みcontent.len() |

M2Aによる全対象の認可を済ませたものだけをCandidate化する。Brokerのany-scope判定でM2Aの認可を代替しない。S6でも全scope_refsがallowed_scope_keysに含まれることを確認する。前段の認可に失敗した対象のIDをsummaryへ出さない。

既存PERSONAL_STATE wrapperの内部にWORLD_MODEL wrapperが入ることをM3Aのshadow表現として許容する。既存Brokerのwrapperを一括変更しない。候補を複数に分けて根拠と主張が別々に採用される形は禁止。

## S4. 寿命

prepare_candidate(service, request)はサービスのprepare_frame→S2→S3の順。shadow直前に同じserviceのrevalidate_frameを呼び、Currentだけを投入可とする。Changed/Expired/ScopeDenied/Unavailableは対応する省略。FrameErrorは固定codeへ写し、例外文字列をsummaryに含めない。

サービスインスタンスをprepareとrevalidateの間で作り直さない。revalidateの後に再度clockを読み、captured_at <= now < expires_atを確認する。M2A内で検証処理中に期限を跨ぐ場合も検出するためである。shadow比較の終了後も同じ時刻条件を確認し、超過なら結果はexpiredとして破棄する。自動再取得・retry・TTL延長0回。

これはshadow時点の検証。取得後や応答生成中の変化を防ぐlockではない。generation完了用のAPIとして転用しない。

## S5. 純粋な比較入力

world_shadow.rsに以下を置く。

```text
ShadowInput { run_id: String, base: ContextWindow, existing_candidates: Vec<Candidate>,
              source_warning: Option<String>, allowed_scope_keys: BTreeSet<String> }
ShadowSummary { status: baseline_error | world_omitted | compared,
                omission: Option<WorldOmission>,
                baseline_bytes: Option<usize>, proposed_bytes: Option<usize>,
                world_selected: bool, existing_selected_count: usize,
                world_bytes: usize, total_elapsed_ms: u64 }
```

summaryは固定enumと数値だけ。本文、名前、source ID/digest、Project ID、run ID、例外文を含めない。評価fixtureでは合成データに限り別途期待JSONを保存できる。既定loggerにsummaryを自動送信しない。

ContextWindow等のCloneが必要ならデータstructのderiveに限定する。baseをserialize/deserializeして複製せず、入力を再取得もしない。呼出前後のinput snapshotが不変であることを試験する。Production Provider historyへ変換した値は返さず、ShadowSummaryだけ返す。

## S6. 実Brokerによる二系統比較

run_shadow(service, source_request, &input, clock)の処理順を固定する。

1. input.run_idと要求run_idの一致を確認。不一致ならscope_denied、World取得0回。次に同じ入力のcloneで既存broker::composeを呼びbaselineを作る。失敗ならbaseline_error、World取得0回で終了する。
2. S1/S4で候補を用意する。省略ならworld_omittedとbaselineの数値を返す。
3. 全candidate.scope_refsがallowed_scope_keysの部分集合であることを確認。不一致ならscope_denied。
4. 再検証と終了時刻検査に通った候補一つを、元入力のexisting_candidatesへ追加したcloneでcomposeする。元入力とbaselineを変更しない。
5. baselineで採用された既存候補のidentity（source_kind/id/version/digest）がすべてproposedで採用されているか確認する。一件でも失われたらwould_displace、world_selected=false、proposed_bytes=baseline_bytesとする。Worldを除いた結果を採用したという意味で、再composeは不要。
6. Worldが予算で省かれたならbudget。composeのその他エラーはsource_errorとしてbaselineへ戻す。無限retryや再ランキングはしない。
7. 比較終了時のTTLを確認し、期限内ならcomparedと数値を返す。Worldなしのbaselineが正しかった事実と、World比較が成立しなかった理由を分ける。

同順位May/utility0の既存候補を押し出す場合も採用しない。これにより既存候補の命名順に依存した割込みを防ぐ。source_kindを含まない現行Brokerのdedupは変更せず、S3の名前空間を使う。

最大compose2回、prepare1回、revalidate1回。予算超過時の再取得0回。DBにgeneration、input manifest、World assertionを作らない。

## S7. 製品経路への誤流入防止

generation_inputs::recordの先頭、set_health/add_inputより前に、selectedとomittedのsource_kindにworld-model-shadowが一つでもあれば `world-shadow-not-dispatchable` を返す。記録の途中やdispatch後に拒否しない。

試験はplanned generationをfixtureで作り、record呼出しでエラー、statusはplanned、input件数は呼出し前と同じと確認する。handleのDropが後でinterruptedにする既存動作は別に確認する。誤流入したGenerationを無理に成功へ戻さない。

これはrecord経路の境界であり、任意文字列を直接Providerへ送る全経路を防ぐ仕組みとは称さない。通常turns、chat_completions、agent_session、conversation_controllerにはM3Aを配線しない。将来の本番source_kindはM3Bで別に定義し、この拒否を削除して有効化しない。

## S8. fixtureと回帰

M2Aのfixture helperはcfg(test)の範囲で再利用可。別moduleから必要なhelperだけpub(crate)にする。production可視性を一括で広げず、新しいDB writerや実ASR／pi processを起動しない。

共通fixtureはrun1、project:p、固定clock1000、ttl1000。EntityId seed e1、会議m1/resource:m1、coding j1/task:j1を明示登録する。Sourceとscopeを正しく紐付け、五要素のrelationと不成立・unknown条件を含める。

必要な組合せ: graphのみ／Runtimeのみ／両方／両方なし、期限1999／2000／999、旧Source削除、Project revoke、direct link削除、revision据置のowner state変更、容量超過、省略notice、偽命令文字列、長い日本語、既存May0候補との競合、別runへの転用。

別runへの転用はPrepared型を外へ渡せないことに加え、異なるinputのallowed_scopeと要求runの一致をfixtureで検証する。ShadowInput.run_idと要求run_idの厳密一致を確認する。

source失敗でbaselineのhealthをgreenへ上書きしない。基準入力の既存warning・yellow・budgetを保つ。SQL total_changes・表件数によりshadowの書込み0を確認する。

## S9. 制約の変更

新しい認可・Source provider・LLM抽出・長寿命receiptが必要になった場合、カード内で独自実装せず対象外として記録する。private helperや試験ファイルの分割は担当者が決めてよい。scope／期限／状態の意味／予算／Provider非接続は変更しない。
