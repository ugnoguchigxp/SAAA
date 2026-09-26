# Qwen → ornith handoff待機の調査（2026-09-26）

対象: `run_f6cc9796-f0c0-4008-ac9c-cee782452727`。時刻は特記しない限りJST。

追加調査: [qwen-audio-agentとのロール・ASR・LLM・TTS設計比較](/Users/y.noguchi/Code/SAAA/spec/evidence/handoff-investigation-20260926/QWEN-AUDIO-COMPARISON.md)。受付・仕事・通知を分ける構成は参考になるが、LARMの実行枠競合そのものは解消しない。SAAAの基本分担を維持し、待機期限と通知・取消の契約を補強する案をまとめた。比較先の単体試験69件が成功し、実音声E2E・性能改善は未確認。

## 結論と確度

**当該runはornithの生成失敗ではない。Agent Connectionのreadiness待ちの途中で、ユーザーが停止した試行である。** LARM側では資源allocationは直ちにreadyになったが、ornithの実行枠が別の長時間要求に使われ、semantic readiness probeが実行できなかった。アプリは接続のready/claim完了を待つため、当該runの生成要求を送っていない。

この実行枠競合は、LARM journalの99回の `probe_busy` と稼働releaseの `ExecutionGate.tryAcquire` の条件から確認できる。単なるモデル未起動とする説明は不正確である。実際、ornithのruntimeはreuseされ、別要求の生成を実行中だった。

SAAAがhandoffまで接続確保を遅らせ、その待ち時間をreasonerの120秒予算に含めていることが、競合をユーザーの長い待機に直結させている。これはQwenの独立した即応経路を保つための現行設計でもあり、単純に遅延claimを削除する修正は勧めない。

**証拠の限界:** SAAAのrunとLARMのAgent Connection IDを直接結ぶ監査項目がない。以下のallocationとの対応は、同時刻・client・profile・生成開始/解放時刻の一致による非常に強い対応付けである。元のpollのHTTPアクセス記録と接続応答bodyは保存されていないため、poll回数と各bodyは復元できない。LARMのバックグラウンドprobeをクライアントpollの証拠として数えてはいない。

## 元の試行の時系列

| 時刻 | 確認した事実 | 根拠 |
|---|---|---|
| 18:20:00.972 | 別の `openai-http` 要求 `req_8a81df2b-466f-439c-8566-c21d4219ed26` がornithで開始。routeは `llm-default`、監査metadataのcapabilityは `llm.coding` | LARM journal・inference audit metadata |
| 18:20:59.995 | 「発生してください。」をuser messageとして保存、run開始 | SQLite `conversation_messages`, `runtime_runs` |
| 18:21:00.013 | Qwenの初回応答要求開始 | `qwen-first-response-started`, `qwen-http-sending` |
| 18:21:05.524 / .526 | Qwen結果 `handoff`、「少し考えます。」保存 | `qwen-first-response-finished`, messages |
| 18:21:05.634 / .636 | 文脈構築完了、接続待ち開始 | `ornith-context-finished`, `ornith-connection-started` |
| 18:21:05.674 | SAAA用の5 Providerをreuseし、allocation pending→ready | allocation末尾 `bf9417fa-69f2-43d8-865b-94ccb28c8d07`, client=`saaa-desktop` |
| 18:21:06.593〜18:22:53.568 | ornithの `probe_busy` が99回。ほかのProviderはprobe成功を反復 | LARM journal |
| 18:21:07.755 | Agent Connection createを202で受理。requestedProfile=`SAAA` | `agent_connection_create_accepted` |
| 18:21:06〜38ごろ | TTSの開始・完了が複数回存在。最後の通常のspeech-endedは18:21:38.248 | アプリ監査。各音声の内容は監査からは断定しない |
| 18:22:53.954 / .956 | Provider session取消、run=`cancelled`, failure=`user-cancelled` | SQLite |
| 18:22:54.056 / .057 | 対応するSAAA allocation解放、Agent Connection release=204 | LARM journal |
| 18:22:57.111 | 上記別要求がHTTP 200で終了 | LARM journal。元のSAAA runの最終回答ではない |

run全体は **113.961秒**。接続待ち開始からrun終了は **108.320秒**。「少し考えます。」保存からは108.430秒。別要求は約176.139秒実行され、SAAAの接続待ち区間を覆っている。

Provider session `provider-session_1790414465625906000_26` は `allocation_id=NULL`, `output_started=0`。`provider_transport_events` は0件。対象時間帯のLARM journalにはclaim accepted/rejectedがともにない。接続完了、lease開始、当該runの生成開始もない。

**空のallocation IDは「LARMが資源を確保していない」という意味ではない。** SAAAではclaim後、Providerの生成送信準備に進んでから `bind_transport` がallocation IDをDBに書く。LARM側のallocation readyとSAAA側のこの列は別の段階を表す。

## コード経路と期限・取消

1. `src-tauri/src/providers/larm_voice/mod.rs:89` の `begin_larm_voice_session_inner` は、ASR=`harness`かつTTS=`system-tts`ならOwnerだけを作ってreturnする。保存済みroutingはこの条件に一致する。Qwenの独立HTTP要求より先に5 Provider全部をclaimしてしまうのを避ける設計。
2. Qwenは `conversation_provider_route.d/01.rs:975` 付近の `frontend_model_output` → `frontend_completion` で独立したLARM HTTP経路を使う。handoffを `commit_preface` で保存し、role stepをreasonerへ進める。
3. `stream_larm_voice_provider_once` (`providers/stream/larm_voice.rs:125`) は接続開始イベントの直後、取消または期限と競争させながら `current_at` をawaitする。
4. `current_at` → `current` (`providers/larm_voice/mod.rs:253,326`) → OwnerのOnceCell初期化。初期化workerは別taskで動くので、turnの取消・期限によってawaitが破棄されてもOwnerがcleanup責任を保持する。
5. `crates/larm-session/src/lib.rs` はcatalogのrevisionを取得し、`POST /v1/agent-connections` (`profile=SAAA`, `Prefer: wait=1`, idempotency key, existing-only) → pending/deploying/probingを `GET /v1/agent-connections/:id` でpoll → ready後に `POST :id/claim` →全Provider health確認、の順。初期化の上限は300秒。通常の各HTTP要求には別途30秒上限がある。
6. 稼働LARM release `30188f792941` の `ExecutionGate.tryAcquire` は `active >= maxConcurrentRequests` の時だけ `probe_busy` を記録して枠取得を拒否する。semantic readinessは有効な成功cacheが使えなければ `provider_busy` を返す。Controllerは全Providerのreadyが揃うまでAgent Connectionをprobingのままにする。allocation自体がreadyでもclaim可能にはならない。
7. claim・healthを抜けた後だけ、接続完了→`session.acquire("llm")`→lease完了→残り時間で生成要求、となる。元のrunはこの前段で止まっていた。
8. 接続await中の取消分岐は `ornith-connection-finished` を記録せずreturnする。従って、開始だけ残ることはこの取消経路と整合し、taskが未処理のまま取り残された証拠ではない。

元runが参照する不変ポリシー `rr-policy-11` は frontend=8秒、reasoner step=120秒、root=180秒。rootのDB期限は **18:23:59.998**。reasonerの120秒はおよそ **18:23:05.6**。ユーザー取消18:22:53.956はいずれよりも前である。通常routingの240秒ではなくrole policyの120秒がこの経路に適用される。

`wait_for_reasoner` の2秒tickにより期限時の定型応答は既に存在する。一方 `filler_decision` はtick 5/10/15付近だけ発話し、tick 20以降は発話しない。120秒予算と相づちの約30秒分が一致していない。UIには接続状態pollも存在するが、当時その文字が実画面にどう表示されていたかは未確認。接続状態はwatch値であり、create/poll/claimの履歴を永続化していない。

## 実API・実会話処理の検証

本番DBはSQLiteの `mode=ro` / `-readonly` で読んだ。試験はSQLite backupで作った隔離DBと、作業ツリーを複製した調査用ソースで実施。本番の設定・Provider・会話は変更していない。別接続を誤って再取得しないよう、隔離DBだけのlease keyを独立させた。

### A. 独立API試験

18:49:48 create=202、18:50:14 claim=200（開始から27.833秒）、5 Provider取得。claimで得たURLとcredentialを使い、QwenはHTTP 200。ornith要求はLARMまで到達したが、別要求の後ろでqueueに入り90秒で試験側timeout。接続DELETE=204。

ornith要求IDは `req_1f48eab9-6cad-4503-8a7c-8eb81eea55a1`。LARMの最終outcomeは `client_cancelled`。この試験もornithの生成失敗とは扱わない。カタログや直接モデルURLだけを実行成功の証拠にはしていない。独立プロンプトによるAPI試験なので、Qwenのアプリ用分類契約の検証にも数えない。

### B. 保存済み予算での実runtime試験

`investigation_46997c9823444e399076dd6f9c5645cd`。入力は「寿限無の名前を最初から最後まで教えてください。」。

- Qwenは実APIでhandoff。18:53:14.384に「少し考えます。」を保存。
- 18:53:14.421から接続待ち。LARM側は再びornith `probe_busy`。
- 18:55:14.424、120.003秒後に「すみません、時間内にお答えできませんでした。」を保存し `messageCompleted` を発行。実行全体126.468秒。
- Provider sessionは `cancelled`。runは `completed`、rootも `completed`、監査outcomeはsuccess。これは**期限の定型応答の完了**であって、ornithの回答成功ではない。
- 音声認識と実音声再生・WebView描画はこのランナーの対象外。実runtimeの `execute_turn`、Provider HTTP、イベント、SQLite保存までが対象。外部メモリ検索とTTSはランナーで動かしていない。モデル応答をmockにはしていない。

### C. 接続完了が予算をほぼ使い切った試行

`investigation_54c62bf63fad4046ab7d41904f557f41`。接続待ちは18:55:57.006〜18:57:55.045の **118.039秒**。18:57:55.019にLARM claim=200、5 Provider取得。lease完了後、18:57:55.069にtransport `sending` を保存し、LARMも `req_69dfa929-0216-4779-89b0-43dcfdb0f5a6` の生成開始を18:57:55.100に記録した。

しかし18:57:57.012には120秒期限の定型応答が保存された。生成に使えた時間は約2秒。調査用Ownerの終了でrelease=204となり、LARMの要求終了は `binding_invalidated`。これをモデルの生成失敗と解釈してはいけない。DBにはallocated-http、allocation ID、prepared/sendingが残り、元runとは到達段階が違う。

この試行では隔離DBの設定文書だけを延長したが、不変policyは `rr-policy-11` のままだった。従って120秒予算の試行として扱う。次の試験では隔離DBに新しいpolicy snapshotを作り、runのpolicy IDとroot期限を実際に確認した。

### D. 期限延長条件での最終回答経路確認

調査用の別DBコピーでのみroot=600秒、step=480秒の `rr-policy-12` を適用。runは `investigation_7762add7e8a742c49862b5a3085e5023`。実際にこのpolicy IDとroot期限600秒をDBで確認した。Providerは変更していない。待ち時間延長を製品修正として提案するものではない。

19:04:00.476にclaim=200、19:04:00.503に `ornith-connection-finished=success`。接続待ち時間は **245.044秒**。lease完了後19:04:00.523に生成sendingを保存。LARMでは `req_f17cd1aa-ca6e-43da-b412-06ccc5bd7f31` が受理されたが、19:04:00.077に開始済みの別要求の後ろにqueueされた。これも接続完了が直ちに生成実行可能を意味しない実測である。

19:06:00.561、LARMは **queue_timeout** で要求を終了。キュー待ちは120.002秒。SAAAは19:06:00.665にtransport `failed/capacity`、Provider session `failed/capacity`、run `failed/provider-error` を保存し、`providerFailed` と `failed` イベントを発行した。`output_started=0`。19:06:00.709にrelease=204。

**今回、ornithのモデル回答が最終messageとして保存される正常完走は確認できなかった。** 実APIを使って接続・claim・生成要求まで検証したが、延長予算でも共有実行枠の競合に阻まれた。過去の同時生成1回やmock試験をこの経路の受入成功に読み替えない。保存済み120秒条件での期限応答の保存はB/Cで確認できた。UI・音声までの正常受入は、実行枠が確保できる条件で残っている。

正常経路のコードは、claim後の接続/lease完了→ `stream_model_provider_with_api_key` →allocation binding→transport `prepared/sending/headers-received/response-started/completed`→Provider session completed→ `persist_conversation_success_with_state` / `accept_provider_turn` →message/会話イベント保存→`voice_response::complete` の `messageCompleted` / run終了。実測で通過した範囲と分けて読む必要がある。

## SAAA側ロジックの合理性レビュー（追記）

**総合判定: 接続契約と資源管理の基本方針は合理的。ただし、冷間・混雑時の対話を成立させる待機方針と、段階をまたぐ期限・結果の表現に不足がある。現状を「性能限界なので仕方ない」として受け入れるのは妥当ではない。**

ここでいう「こちら」はSAAAと同梱の `larm-session` を指す。LARM daemonのスケジューラ改修とは分けて評価した。コードの性質、前述の実測、未検証の境界条件を区別する。今回の追加作業はコードレビューと文書更新のみで、新たな負荷試験や実装修正は行っていない。

| 対象 | 判定 | 理由・根拠と残る条件 |
|---|---|---|
| Qwenの初回応答をornith接続確保から独立させる | 妥当 | 重い接続準備が挨拶・確認質問まで止めるのを防ぐ。元runと隔離試験でも接続未確保のままQwenは応答した。ただし、初回応答の独立性だけでは最終回答の到達を保証しない。 |
| `begin_larm_voice_session_inner` の遅延claim | 条件付きで妥当 | ASR/TTSが共有接続を不要とする構成で、使わない5 Providerの先取りを避ける判断には根拠がある。反面、初handoffで初めて準備することと、64〜267秒級の準備実測・120秒の回答予算の組合せは整合しない。遅延claim単体の誤りではなく、未準備handoffの扱いが不足している。 |
| create→poll→claim→health→Provider要求 | 妥当 | 実行契約と短命credentialを得てから要求する正しい順序。catalogやallocation readyだけで生成へ進めないのは必要な制約。claim後のcapacity変化・queue待ちは別に扱う必要があり、接続成功を「今すぐ回答可能」と表現してはいけない。 |
| 接続・lease・生成で同じattempt予算を使う | 上限管理として妥当、対話設計は不足 | 各段階に120秒ずつ与えて総時間が膨らむのを防ぐ点は合理的。ただし118秒の接続待ちの後に残り約2秒で生成を始める実測があり、成立見込みの低い要求を送る。準備待ち中の短い応答期限、残り時間不足時の明示的な終了、別Ownerでの準備継続を定義すべき。 |
| Owner/OnceCellで初期化を共有し、lease keyを永続化する | 妥当 | 同一Ownerの重複createを抑え、再起動時の接続回収を可能にする。`Arc::ptr_eq` による世代確認も古い接続を採用しないために有効。global Ownerは単一会話資源を扱う現設計と整合するが、複数会話の独立利用を保証する実装ではない。 |
| 呼出側が取消になっても初期化workerを残す | 条件付きで妥当 | create直後など、単にfutureを破棄すると解放すべき接続を見失うため、cleanup責任を保持する設計は合理的。run終了後も準備を続けるか、いつOwnerを閉じるかという製品方針は別途必要。元runでは取消直後のrelease=204を確認しており、資源漏れとは判定しない。全ての取消タイミングについて保証したわけではない。 |
| 失効した接続だけ、出力前に1回再試行する | 条件付きで妥当 | `AllocationLost && !output_started` に限定するのは、二重回答や不明な実行結果の再送を抑える方針として良い。capacityエラーの無条件連続再試行をしない点も妥当。ただし残余予算と取消をcleanup・再接続にも引き継ぐ必要がある。 |
| 「少し考えます。」と待機中の相づち | 要改善（実測あり） | 接続も生成も未開始の段階で「考えます」だけを残すと、推論中か準備待ちか判別できない。相づちは約30秒で止まる一方、stepは120秒続く。状態に沿った説明と、短いユーザー応答期限の方が適切。 |
| 期限応答・失敗表示・監査 | 一部妥当、統一が必要（実測あり） | 120秒の期限応答を保存する経路は実際に動作した。しかし内部期限でもProviderはcancelled、定型応答完了はrun successとなり、モデル回答成功と区別しづらい。別のqueue timeoutではFailedイベントだけになった。処理終了・回答成功・準備失敗・ユーザー取消を別の結果として残し、どの失敗でも説明がユーザーに届くようにする。 |

### 期限に関する追加のコード確認

- `role_routing/executor.rs:125` の `permit_next_step` は、dispatch直前に永続root期限とstep経過時間を検査する。この入口の検査は妥当であり、期限検査が全くないという指摘は誤り。
- ただし `conversation_inputs_roles/role_dispatch.rs:160` は残余時間ではなくroot/stepの期間値をrouteへ渡す。`conversation_provider_route.d/01.rs:56` はそこから `now + duration` を作る。例えば180秒rootの開始から170秒後に次stepのdispatchが許可された場合、入口検査だけではその先の120秒I/Oをrootの残り10秒に制限できない。**これはコード上の整合性リスクで、今回の元runで発生した事実ではない。** 出口で不採用にする処理があっても、期限後のI/O自体を防ぐものではない。
- `providers/stream/larm_voice.rs:70–136` のAllocationLost再試行は、同じ期間値で新しいattempt期限を作る。一方、今回の音声handoff経路には外側の `wait_for_reasoner` があり、そのtick期限が全attemptを制限する。従って、今回の経路が再試行だけで必ず120秒を超えるとは断定しない。外側のwrapperがない呼出経路も含め、同じ絶対期限を渡す方が一貫している。
- `wait_for_reasoner` のtick期限と内側の `timeout_at` は独立して競争する。前者は定型応答を保存し、後者が先に失敗を返す場合は一般の失敗処理へ進む。今回B/Cでは前者を確認した。timeoutの種類や順序にかかわらず同じ説明・監査結果になることは未確認であり、境界試験が必要。

### 維持すべき点と、SAAAだけで直せる点

維持するのは、Qwen即応の独立、正式なAgent Connection契約、Ownerによる初期化共有、確実な解放責任、出力後に再生成しない原則である。これらを取り除いて待ち時間を短く見せる修正は合理的ではない。

SAAA側で先に直せるのは、準備待ち/実行枠待ち/生成中の区別、短いユーザー応答期限、残余予算の引継ぎ、取消・期限・capacityの結果分類、stage終端の監査である。**準備と生成の予算を分ける場合も、同じユーザーrunの絶対期限を超えて加算しない。** 長い準備を続けるなら、短いrunを説明付きで終えた上で、明示された会話Ownerの準備処理として継続する。再試行は準備済みOwnerを再利用し、終了済みrunへ回答を後付けしない。

一方、事前確保がQwenの独立要求を妨げない保証、probeの公平な実行枠確保、claim後queueの上限・優先順位はLARMとの契約にまたがる。現状の単一実測だけで「先に全Providerをclaimすれば解決」「並列数を増やせば解決」と決めるのは合理的ではない。

追加で必要な境界試験は、root残り10秒での新step、AllocationLost再試行中の期限、tickと内側timeoutの前後関係、Owner初期化中のrun取消と会話終了の違い、release失敗後の所有権維持である。既存の `reasoner_times_out_when_the_step_budget_elapses` は定型応答経路を確認するが、これら全部や実LARMの競合条件を代替するものではない。

## 優先する修正案

1. **接続待ちを短いユーザー応答期限から切り離す。** handoff直後は「詳しい回答に使う接続を準備しています」など確認済み状態を伝える。例えば5〜10秒で状態更新、15〜30秒で「まだ接続を確保できません。再試行できます」と一度明示的に終える。秒数は提案値。接続準備をバックグラウンドで続けるかreleaseするかはOwnerの方針として明示し、終了済みrunへ遅れて回答を書かない。
2. **実際のAgent Connectionを先行確保する。ただしQwenを待たせない。** アプリ起動/会話開始から独立workerでcreate→poll→claimを進める案を検証する。5 Provider全体の先行claimが独立Qwen要求と競合し得るため、LARM側でreasoner用確保とQwenの実行枠を分離する、または準備済み接続のbackchannelを初回応答が安全に再利用する契約が必要。カタログの先読みだけでは代替できない。単なるローカルプロセス確認や直接URL疎通も代替にならない。
3. **LARMのreadinessと実行待ちを分け、probeの飢餓を防ぐ。** 長時間の通常要求でprobeが延々と枠を得られない状態を可視化する。有効なruntime/release単位の検証cache、短いprobeに枠を渡す公平な順序、上限到達時の具体的なbusy理由を検討する。稼働中要求を勝手に停止したり、資源確認なしに並列数だけ増やしたりしない。claim後もqueueが長くなることを今回A/Dで確認したため、事前確保だけで十分とはしない。
4. **期限・取消・監査を一貫させる。** run→Owner→connection→allocation→requestを追跡できるIDと、create応答、poll状態変化、claim/health、queue、releaseの時刻を保存する。取消時も各stageのterminalを必ず残す。ユーザー取消と内部期限取消を区別し、定型期限応答を「モデル回答成功」に集計しない。Dのcapacity失敗は定型回答保存に入らずFailedイベントで終わったため、接続timeout・queue timeout・その他の準備失敗でもユーザーに短い説明が確実に届く経路を統一する。相づち「はい。」の反復より、実際に変わった待機状態を表示する。
5. **再試行は同じ絶対期限とOwnerで管理する。** `AllocationLost` の1回再試行は現状同じ `timeout_ms` で新しいdeadlineを作る。role routeもstepごとにrootの期間値からdeadlineを作るため、永続rootの残余時間を全await/再試行に引き継ぐ設計を確認・統一する。これは元runの原因ではなく、コードから分かる追加の回帰観点。二重createを防ぎ、close失敗時はcleanup責任とlease keyを保持して再試行する。

現在の作業ツリーには、目的語のない「発生してください。」を確認質問にする `guard_for_input` が既に存在する。今回の調査で加えた変更ではない。この入力が不要なhandoffに進むことは減らせても、正当なhandoffの接続待ち問題は残る。ASRが原発話のどこを失ったかは今回の証拠から特定できない。

## 必要な回帰試験と未確認事項

- cold/ready接続、runtime busy、semantic probe busy、claim後queue、health staleを区別した試験。Qwenの初回応答時間と最終回答到達時間を別計測する。
- 64/120/267秒の接続待ち、300秒startup失敗。短いユーザー期限で必ず説明と終端が出ること、期限後に元runへ回答を書かないこと。
- create応答前、poll中、claim中、生成queue中、出力後の取消。各stageのterminal、release=204またはcleanup保留、Owner世代・idempotency key・再起動時の所有権を検証する。
- AllocationLost再試行がroot絶対期限を延ばさないこと、出力後には再生成しないこと、同時turnがOwner/connectionを二重作成しないこと。
- 実LANで先行確保あり/なし、ornith busy時のQwen p50/p95を比較。今回の一度の成功や過去の一度の同時生成だけでは保証しない。
- 実LANと隔離DBを使う受入試験で、handoff→接続→生成→最終message保存→UI表示→音声完了を通す。DB上のcompletedだけではモデル回答成功と判定しない。
- 元runの正確なAgent Connection ID、poll回数・body、別coding要求の呼出元、当時の画面表示、起動バイナリと現在ソースの完全な一致は未確認。別要求の呼出元を別Codex chatなどと推定しない。
- 基本の対象テスト: `cargo test --manifest-path crates/larm-session/Cargo.toml`、`cargo test --manifest-path src-tauri/Cargo.toml butler_route_tests`、`bun test tests/larm-voice-owner.test.ts tests/larm-voice-drain.test.ts tests/larm-voice-lifetime.test.tsx`。これに上記の遅延・取消・競合条件を追加する。今回、これらを実装回帰の完了として実行したとは主張しない。
- 実装時に初回応答凍結域を変更するなら `bun run quality:check` とmacOSの `bun run desktop:smoke`、理由付き `freeze:accept:initial-response` が必要。ASRを変更するならAGENTS.md指定の3つのBunテストとRust `voice::streaming_asr`、ASR域だけのfreeze更新が必要。

調査中は製品実装・本番設定・本番DBを変更していない。`bun run freeze:check` は成功。既存の別Codex chatへの送信は行っていない。調査用ランナーの初回ビルドでは補助constructorのfeature不足があり、`quality-eval-harness` を付けて成功した。これは製品の失敗ではない。

## 保存した証拠

- `app-evidence.json`: 元run、session、監査、messages、step/root、不変policy。
- `larm-timeline.json`: 稼働LARMの18:19:50〜18:23:00のjournal JSON。
- `larm-*.ts`: 稼働releaseのreadiness/connection/execution gateのソース写し。
- `source-fingerprints.json`: 調査したSAAAコードのSHA-256とHEAD（未コミット変更を含む作業ツリー）。
- `live-api.json`: 今回のcreate/poll/claim/Provider要求/release実測。credential値は含めない。
- `runtime-default-budget.json`: 保存済み予算での隔離runtime試験。
- `runtime-connection-budget-exhausted.json`: 接続確保に118秒を費やした試行。
- `larm-live-validation.json`: 追加API/runtime試験のLARM journal。
- `runtime-extended-budget.json`, `larm-extended-validation.json`: 600/480秒policyでの実runtimeとLARM queue timeoutの対応。
- `diagnostic-runner.rs.txt`: 隔離ソースだけに追加した実runtimeランナー。`quality-eval-harness` は補助constructorを有効にするために使用し、`cfg(test)` やモデルmockは使っていない。
- 調査用ソース・DB・ビルドログ: `/tmp/saaa-handoff-investigation-20260926/`。DBには元DBのコピーが含まれるため公開用添付にしない。

運用ツール実行回数（比較調査追記を含む会話累計）: initial_instructions 1回、context_compile 3回、compile_eval 3回（対応runへの保存済み）。元調査の最終確認で本番の全settings_documentsが調査開始時の隔離コピーと一致し、対象製品ソースのSHA-256も不変であることを確認した。比較調査では本番DB・設定・製品ソースへの書込みを行っていない。
