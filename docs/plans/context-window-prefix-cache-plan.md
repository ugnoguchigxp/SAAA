# 長期メモリ第1段階 Context安定化と初回実動作確認の実装計画

状態: 2026年10月1日、C0〜C3のコードと隔離回帰試験を実施。C4の実モデル試験は両modeともJSON契約不一致で途中終了し、20ターン受入は未合格。実cacheと物理音声も未確認。結果は[初回試験の記録](../../spec/evidence/memory-context-first-trial/trial-notes.md)を参照。既定modeはlegacyのまま。

製品コンセプトの唯一の正本は[SpaceのSAAAの全体コンセプト 第5章](https://chatgpt.com/space/page_9fc5877949748191b556705128f6a2f5)。本書は最初の実装範囲、送信契約、検証、観察して一度止める地点を定める技術計画であり、コンセプトを複製する文書ではない。

## 1. 最初に試すテーマと止める地点

最初に試すテーマは、**同じ話題の文字会話とTool往復で固定Prefixを維持し、通常の応答・訂正・根拠検証が崩れないか**とする。まず「SAAAのメモリ実装をどの順に進めるか」を継続して相談する20ターン程度の会話で確かめる。

実装は、固定System Promptと提示Tool定義の安定化、可変情報の末尾配置、現行queue経路での計測、同条件の比較に限る。初回の実動作確認を終えたら、結果と不足をまとめて**この段階で一度止める**。Snapshot／公開Epoch、Checkpoint＋Tail、Profile、Episodeの本格実装へ自動で進まない。

長期メモリ全体を最後まで実現する方針は維持する。この区切りは、モデルへ届くContextと実際の会話を先に確認し、次の実装判断に使うためのもの。初回で確かめるのはContext基盤であり、日をまたぐ本人理解や長期記憶の完成ではない。

### 初回で分けて判定する三つの結果

| 判定 | 成立条件 |
| --- | --- |
| Context構造 | 同じPolicy・同じ提示Tool集合なら固定Prefixが同一。日時、Tool残り回数、辞書保留状態で変化しない |
| 会話と整合性 | 現在入力、訂正、最新の必要な状態、Tool結果、根拠失効、容量制限、取消・先行TTSに回帰がない |
| 実cacheと応答時間 | 実Providerのusageまたは診断で再利用を確認し、同条件の遅延を比較できる。計測不能なら未確認とする |

最初の二つが成立しても三つ目が不明なら「Context基盤は成立、cache効果は未検証」と報告する。Prefix一致をcache hitへ読み替えない。cacheが効かないことだけで長期メモリ機能を縮小しない。

## 2. 現在の実装と既存計画の継承

確認対象はHEAD `979cf9780c128aba860df916a007e9cf663d3d37` と2026年10月1日の未コミット変更を含む作業ツリー。現行会話はOrnith単一queue経路。今回、実行中build、実効Provider設定、本番DB、実cache能力は確認していない。実装開始時に対象コードと差分を再確認する。

| 対象 | 確認した動作 | この段階で行うこと |
| --- | --- | --- |
| `runtime/conversation_check/queue_context.rs` | 固定指示の途中へ現在日時を追加し、その後ろにContext Policyを結合。履歴・Memory・Worldを投影し、応答後と保存前に再検証する | 固定指示と可変Runtime情報を分離。根拠検証は保持する |
| `runtime/conversation_check/queue_runtime.rs` | Tool定義、辞書の固定指示とpending_contextをSystem Promptへ追加し、Tool往復ごとに残り回数を追加する | 現在提示されるローカルToolの全定義を正規化し、辞書保留状態と残り回数を動的領域へ移す |
| `runtime/conversation_check.rs` | LARM lease、容量検査、履歴調整、先頭systemと現在user入力、SSEからJSONへの条件付きfallbackを持つ | 最終送信構造を型付きで組み、削ってはいけない要素を予算検査で保護。論理要求と実HTTP試行を識別する |
| `providers/chat_completions/mod.rs` | messages全体を送信し、返り値は本文String。JSON応答のusageをqueueへ返す契約はない | 既存呼出しを壊さない内部の観測契約を追加する |
| `providers/chat_completions/chunks.rs` と `stream_response.rs` | SSE usageをCompletionへ格納するが、stream結果は本文Stringで返る | usage、初回content、完了時刻を本文から独立してqueueの計測へ渡す |
| `runtime/context/usage.rs` | usage parser、timings、generation_usageを持つ | parser・型を再利用。存在しないcontext_generation IDでusage表へ書かない |
| `memory/context_window/` | Memory、古い抜粋、直近履歴を毎回投影する | 今回は動的な参照資料として継承。固定Checkpointや追記Tailとして完成したとは扱わない |
| `runtime/context/segment/` | 固定領域、entries、manifest等の部品が残る | 新機構の参考と後続の再利用候補。今回はbuilderをそのままつながず、役割・根拠・予算契約を先に整える |
| `app_state.rs` | `SAAA_CONTEXT_SEGMENTS` は既定OFFで、現行queueはSegmentBuilderを呼ばない | 既存フラグの意味を変更しない |
| `ConversationAudit` | correlation ID付きのaudit_eventsを書き、通常は本文digestを残す | 既存監査へ小さな構造化計測を追加する |

本ファイルの2026年9月27日版を本改訂で置き換える。旧版のQwen入口・Qwen最終発話、backchannel、役割ごとの先読み、旧Prompt配置や数値採用基準は今回の実装対象にしない。接続・leaseとモデル側の会話状態やKV保持は別であり、Chat Completionsは後続要求もmessages全体を送る。

`spec/docs/context-window-records-memory-implementation-plan.md` は既存部品の技術資料として参照する。削除されたconversation_prepare等の旧入口、旧「完了」記録、履歴をassistant発言として置く方式を現行queueへ無条件に復活させない。

作業ツリーでは読み上げ辞書ToolとWorldの過去会話選定等が変更されている。実装中に提示Toolやpending_contextが変わった場合はこの基準を更新し、他の未コミット変更を保持する。別Codexタスクへの送信は行わない。

## 3. 実装する範囲

1. 固定指示、Context Policy、提示Tool定義、辞書の固定指示を決定的に組み立てる。
2. 日時、Tool残り回数、辞書保留状態、現在のScope・必要な参照状態を固定Prefixから分離する。
3. 現行会話の最終messagesで現在入力を一度だけ配置し、必要な動的情報を予算削減から保護する。
4. request単位で固定Prefix、提示Tool集合、送信量、usage、時間、結果を観測する。
5. 隔離fixtureと既存回帰を通し、実Providerと実アプリでbaselineとcandidateを比較する。
6. 初回結果を整理し、Snapshot／EpochとCheckpointへ進む前に一度止める。

### 後続段階へ残す範囲

- User Profile・Episodeの生成、独立Write Gateと昇格、Pending採用の拡充。
- 永続Snapshot、Context公開Epoch、Topic Memory Set、固定Checkpointとappend-only Tail。
- Temporal期間の自然文抽出、Hybrid Retrievalの追加signal、階層検索、linking、reflection。
- ContextStillの契約変更、個人会話の移動、新Vector DB・Graph DB。
- 起動時prewarm、ProviderキャッシュAPI、差分送信、Provider／モデル構成の変更。
- raw保存・抽出・応答利用の設定変更、DB容量enforcement、ASR／AEC／TTS再生方式の改修。

即時失効、出典、Scope、現在の制約、設定、取消・音声の整合は後回しにしない。今回の送信構造でこれらを維持する。新しい長期メモリの型や表を先行して追加しない。

## 4. Contextの契約

### 4.1 要求の組立て

```text
固定System Prompt
  Ornith固定指示
  Context Policy
  提示可能なローカルTool定義
  辞書等の固定操作契約

参照資料と会話
  現行のMemory・継続抜粋・直近履歴
  当要求のAssistant actionとTool結果

動的Runtime領域
  現在日時・timezone
  Scopeと今回必要なWorld
  読み上げ辞書の保留状態
  Tool残り回数・実行状態

現在のユーザー入力 一度だけ
```

Providerが要求するroleと履歴契約を維持し、先頭systemは一件にまとめる。過去会話、Memory、World、Tool結果、辞書の提案本文は非命令の参照資料。Runtime領域はhostが作る構造化状態として別に識別し、引用文が同じ見出しを含んでもRuntime値や操作権限にならない。

日時の値とTool残り回数の値だけを動的領域へ置き、「Runtime日時を使う」「残り0回ではToolを呼ばない」等の不変の動作契約は固定指示に残す。最大回数・許可Tool・辞書変更の確定は既存host側で強制する。モデルへの位置変更を権限の緩和にしない。

### 4.2 固定Prefix

- 同じ固定指示版・serializer版・提示Tool集合なら、同じsystem本文とTool順序を生成する。
- 認可・設定・現行offerで提示可能なToolだけを入れる。利用不可Toolをcache維持のために残さない。
- Tool定義リストをnamespace/name等の安定keyで並べ、objectのkey順を正規化する。enumや手順等の意味を持つ配列は勝手に並べ替えない。
- 同名で異なるschema、定義欠損、重複は黙って片方を採用せずcontract errorとして扱う。
- 時刻、run ID、Tool残り回数、World stamp、pending本文を固定部分へ混ぜない。
- `fixed_prefix_digest` と `tool_set_digest` を別に持ち、変更時にはPolicy／Tool／serializer等の理由を残す。

この段階の固定対象はsystemと定義であり、Profile SnapshotやTopic Snapshotではない。会話履歴・Memoryの先頭が変われば、それより後ろの再利用が短くなることを許容し、変化を観測する。

### 4.3 Tool往復と動的状態

一つのjob内では固定部分を一度作り、各stepで同じ本文を使う。Assistant actionとTool結果は既存の参照履歴へ追記し、前stepのRuntime領域を履歴へコピーしない。次stepでは新しい残り回数と必要な最新状態を末尾へ一件配置する。

現在のユーザー入力をsystemや履歴から重複生成しない。HTTP要求ごとに現在入力が一件存在することと、Tool結果が命令へ昇格しないことを最終payloadで検証する。元のTool名、引数、結果、エラーと操作の冪等性を保つ。

辞書保留状態は必要な操作の前提として扱う。保留状態が変わった後に古いproposalを確定しないよう、辞書serviceの既存の再検証を維持する。Tool集合が処理中に失効した場合、dispatch時に拒否し、必要に応じて新しい固定構成で再計画する。

### 4.4 予算と失効

現行LARMのcontextWindow、出力予約、ローカル上限、最終JSONのwire量を検査する。byteとnative tokenは別の値。tokenizerが使えなければ既存の保守的上限を継承し、実token量が測定済みと称さない。

組立て要素にRequired／Optionalと出典を持たせる。固定Policy、現在入力、今回のRuntime前提、未完了操作とTool結果、今回参照する状態の必須部分をRequiredとして保護する。古い履歴を落とせても、Runtime領域を単なる履歴として先に削らない。必須集合が入らなければ送信前にrequired_context_overflowで停止する。

現在の投影を先に予算削減し、別の場所でsystemや動的本文を後付けする方式を残さない。固定・参照・動的・現在入力を含む最終構造のbudget ownerを一つにする。既存検査を廃止する意味ではなく、入力予約と最終wire制限を同じ送信構造へ適用する。

`QueueContext.validate_result`、`validate_commit`、World frameのsource／Scope／世代検査、speechのfingerprint検査を保持する。日時・残り回数の変更をsourceの失効と混同しない。送った参照資料と検証manifestを対応させ、省略したWorldを参照済みと扱わない。

即時訂正、忘却、根拠変更、Scope／認可変更が起きた場合は古い根拠の出力を採用しない。今回はSnapshotをpinしないため、単なる新しいMemory投影も現行の再検証で再実行になる場合がある。これを回避するために検査を弱めず、後続の公開Epochで整理する。

## 5. モジュールと内部interface

以下の新規名称は実装予定。現存APIと区別する。

| 場所 | 変更内容 |
| --- | --- |
| `runtime/conversation_check/context_compiler.rs` 新規 | FixedContext、DynamicContext、ContextEntry、CompiledRequestの型、固定serialization、動的render、最終配列の構築と予算選択 |
| `runtime/conversation_check/queue_context.rs` | 保存済みScopeでsource projectionを作り、固定／参照／動的へ渡す。source manifestと検証は維持 |
| `runtime/conversation_check/queue_runtime.rs` | 現行offerを渡し、step別の残り回数・辞書保留状態を動的へ供給。旧systemの後付けを除く |
| `runtime/conversation_check.rs` | LARM容量とrequest correlationを渡す。Compilerが選択した配列を送る。既存probe・診断呼出しはwrapperで互換維持 |
| `providers/chat_completions/observation.rs` 新規 | 送信確定時と終了時の観測型・sink。本文を含まない計測receiptを返す |
| `providers/chat_completions/{mod,stream_response,chunks}.rs` | 最終body、usage、初回content、完了／失敗の情報を観測する。SSE／JSONと既存返り値の互換wrapperを維持 |
| `runtime/conversation_check/context_metrics.rs` 新規 | audit_eventsへの集約、比較keyと一時的なmessage-prefix比較、欠測・失敗の記録 |
| `src/conversation_queue_e2e.rs` と対応fixture | 実queueを通る送信payload取得、複数turn／Tool往復／訂正／usage／fallbackのfixture拡張 |
| `src-tauri/tests/conversation_context_stability.rs` 新規 | Compiler単体だけでなく最終送信・保存・取消を確認する公開harness入口からの試験 |
| `scripts/conversation-context-report.ts` 新規 | 既存監査のexportを読み、baseline／candidateを比較するoffline report。計測欠落を表示する |

型の責務は次のとおり。

- `FixedContext`: fixed instruction、tool definitions、policy／serializer version、digests。利用許可の正本ではない。
- `DynamicContext`: host日時、timezone、Tool残り回数、Scope、World／辞書等の参照状態と要件。
- `ContextEntry`: role、body、Required／Optional、source refs、安定順序。実行許可を含めない。
- `CompiledRequest`: 最終messages、固定digest、selected／omitted source、入力bytes、比較metadata。
- `RequestObservation`: correlation、logical request、HTTP attempt、mode、model、usage status、時間、結果。

実装順はlegacyへの観測sink・transport計測とbaseline fixture → typed Compiler → stableのqueue接続 → 回帰fixture拡張 → report。同じ計測を先に現行経路へ入れ、構造変更前のbaselineを採る。既存の長大なconversation_check.rsへSQL・serialization・計測を積み増さず、局所moduleへ分ける。予算計算とserialized messagesを別々の実装で再現しない。

## 6. 計測と保存

### 6.1 計測の正本

第1段階は既存`audit_events`へ、schema版付きの小さなeventを追加する。専用DB table、保存設定、公開IPCは追加しない。`generation_usage`はcontext_generationsへ結び付くため、queueがそのGenerationHandleを持たない状態で架空IDを挿入しない。既存usage parserとtiming型は再利用できる。

計測eventは`ConversationAudit`の2,048 bytes上限へ収める。本文・認証header・secret・生のusageオブジェクトを追加保存せず、許可した数値・digest・固定reasonだけにする。prefix比較に必要な前回messagesは小さく上限を設けた一時メモリに保持し、終了・構成変更・失効で破棄する。既存wire_prefixesを使う場合もsecretを含むHTTP bodyを格納しない。

Auditがbest effortで失敗した場合、通常会話を新たに落とさない。一方、比較reportは欠落をmissingとし、計測が揃った成功へ変えない。

### 6.2 記録する値

| 値 | 用途と欠測の扱い |
| --- | --- |
| correlation_id／logical_request_id／http_attempt_id／step | 一つの依頼、Tool step、SSE・JSON試行を区別する。再送を一件へ潰さない |
| mode／fixed_prefix_digest／tool_set_digest／serializer_version | baselineとstableの構成を識別する |
| provider／requested model／response model／connection identity | モデルalias、実モデル、接続再作成を可能な範囲で区別。返されない世代はunknown |
| fixed_prefix_bytes／messages_bytes／wire_bytes | 固定部分、messages、最終HTTP bodyのサイズを分離する |
| message-prefix一致bytes／prefix change reason | 同じ比較keyの要求を診断する。HTTP JSONの先頭一致をモデルtokenの一致と呼ばない |
| input_tokens／cache_read_tokens／output_tokens | Providerが返した値のみ。未返却はnull、明示0は0 |
| usage status | provider、missing、unsupported、disconnected等。unknownをunsupportedへ決めつけない |
| first_content_ms／first_visible_ms／completed_ms | 初回本文delta、初回公開本文、完了を分離。JSON fallbackの完了時間をSSEのTTFTにしない |
| outcome／fallback／omission_count | 成功だけでなく失敗・取消・省略・fallbackを比較する |

モデル側のnative tokenizerやchat templateが確認できる場合だけstable prefix token lengthを記録する。確認できなければnullとbytesを併記する。今回のSnapshot／Epoch／Deep RetrievalのKPIは対象外または未実装と記し、架空の0回で埋めない。

### 6.3 usageとfallback

SSEのusage-only chunkとJSON応答のusageを同じ観測型へ変換する。usageが通常chunkに同居するProviderの仕様は実契約を確認して対応する。本文のfinish、DONE、partial-output、内部思考非公開の検査を維持する。

`stream_options.include_usage`等を互換API名だけで強制しない。現在の実Provider契約が支持する場合だけ使い、支持が不明なら元のrequest形状を保つ。usage取得のための追加モデル呼出しや無限再試行を作らない。

SSE拒否後の既存JSON fallbackは「公開output未開始」の条件を維持する。同じlogical requestの別HTTP attemptとして記録し、二度目の失敗も失敗として残す。ユーザーに一度公開した本文・音声をfallbackで二重生成・二重再生しない。

## 7. 段階と実装完了条件

C0〜C3の実装と対象回帰は完了。広いgateには既存・並行作業の不整合が残る。C4は実モデルと実アプリ起動まで実行したが、会話受入は未合格。コード存在、fixture成功、実Provider確認、実アプリ受入を別に記録する。

| 段階 | 作業 | 完了条件 |
| --- | --- | --- |
| C0 現状固定 | 変更対象の差分、build、実効設定、fixture初期状態を記録。旧挙動を同じ観測で測れるよう計測だけ先に接続 | baselineを再実行でき、未計測値が分かる |
| C1 Compiler | 固定／参照／動的／現在入力の型と最終予算・serialization・固定Tool順を実装 | 日時・回数・pendingが変わっても固定本文が一致。Requiredは落ちない |
| C2 現行queue接続 | 固定指示の後付けを除き、現行Ornithの全Tool stepでCompilerを使う | 最終payloadでcurrent input一件、Runtime一件、Tool・辞書の動作と根拠検証を確認 |
| C3 観測と回帰 | C0のSSE／JSON／fallback・失敗のreceiptが両modeで同じ意味を持つか検証し、offline report、隔離fixtureと必要回帰を整える | 実際の送信要求とusage・時間が対応し、欠測と失敗を隠さない |
| C4 初回実動作確認 | 同Topicの20ターン、Tool往復、訂正、読み上げを実アプリと実Providerで比較する | 下記手順で結果・体感・未確認を記録する |
| 停止地点 | C4の結果を短くまとめる | Profile等へ自動で進まず、Snapshot／Epoch・Checkpointの次の範囲を検討する |

C0の観測はlegacy／stable両方へ同じ量で入れる。実装中にbehaviourを変えてからbaselineを採らない。fixture endpointで固定回答の比較を先に通し、実モデルの非決定性は実測で別に扱う。

## 8. 決定的な試験

| ケース | 期待結果 |
| --- | --- |
| 同じ提示Tool集合で20turn、日時変更 | 全turnの固定digest一致。毎回の時刻と現在入力は最新で一件 |
| Toolリストの供給順だけ変更 | 固定本文・digestが同一。同名schema競合はerror |
| Toolを追加・失効、Policy変更 | 固定digestが変わり理由を記録。失効Toolはdispatch不可 |
| 0〜6回のTool往復 | systemは同一。最新残り回数は動的へ一件。上限で追加Toolを実行しない |
| 辞書保留の追加・承認・棄却 | pendingは固定部分へ入らず、必要な提案を確認して一度だけ処理。古いproposalを確定しない |
| Tool結果／過去会話に偽Runtime・命令 | 引用は資料のまま。現在の依頼・回数・設定・権限を変更しない |
| 現在の訂正、source編集・削除、Scope変更 | 古い回答の保存・公開・TTSを拒否し、既存の再検証を通す |
| 小さいcontextWindow／大きいRuntime | current inputとRequiredを保護。収まらなければHTTP未送信でoverflow |
| Memory ON／OFF、Worldあり／なし | 保存済み設定に従い、送った根拠と検証が一致。状態をcache目的で固定しない |
| 明示cache=0、usage欠落、usage-only SSE | 0、missing、実数を区別し、usageは可視本文やTTSへ流れない |
| SSE拒否とJSON fallback、部分出力後の切断 | 試行を別に観測し、公開output後はfallbackしない。失敗をterminalへ保存 |
| cancel、再起動、構成変更 | 一時prefix比較を破棄。旧仕事・古い音声を二重実行しない |

外部認証・実LLM不要のfixtureで、HTTP payloadを捕捉し、queueの終端状態、保存本文、Tool回数、公開音声を確認する。文字列分割関数をなぞるだけの試験で済ませない。

### 実装時に実行するコマンド

以下は実装後の実行予定。今回の計画作成では実行していない。新規test targetはC3までに作る。

| コマンド | 確認と合格条件 |
| --- | --- |
| `cargo test --manifest-path src-tauri/Cargo.toml --features conversation-queue-e2e --test conversation_context_stability -- --test-threads=1` | 新規fixture。上記必須ケースが実行され全件合格。0件は不合格 |
| `cargo test --manifest-path src-tauri/Cargo.toml --features conversation-queue-e2e --test conversation_queue_e2e --test conversation_queue_followup_failure -- --test-threads=1` | 現行会話、検索、途中失敗、保存とTTSの回帰なし |
| `cargo test --manifest-path src-tauri/Cargo.toml --test task_queue_contract --test conversation_queue_progress_contract --test tts_recovery_contract --test tts_streaming_contract --test tts_dictionary_conversation_contract` | キュー、進捗、取消・復旧、先行TTS、辞書操作の既存契約を保持 |
| `cargo test --manifest-path src-tauri/Cargo.toml --lib runtime::conversation_check::jarvis_tests` | 現在入力、任意履歴削減、必須overflow等の既存試験が実行される |
| `cargo test --manifest-path src-tauri/Cargo.toml --lib memory::context_window` | projection・現在入力・出典の既存回帰を保持 |
| `bun test tests/conversation-queue-page.test.tsx tests/conversation-asr-continuous.test.ts tests/qwen-realtime-asr-ipc.test.ts tests/tts-dictionary-page.test.tsx` | 画面とASR受付、辞書の既存回帰なし |
| `cargo fmt --check --manifest-path src-tauri/Cargo.toml` | 変更Rustの形式を確認。他の作業の形式変更をまとめて上書きしない |
| `bun run size:check` と `bun run clippy:check` | moduleサイズと既存のlint基準を満たす |
| `bun run check` | C3で広い既存gateを確認。既知のbaseline失敗と今回の失敗を分け、今回起因の失敗は残さない |

Cargo fixtureのglobal環境変数・mock serverを同時に変更する試験は直列化する。前提不足・外部依存・既存不具合で試験が止まった場合、件数・失敗点・理由を残し、skipや部分実行を全体合格へ変えない。失敗は原因を修正して同じ試験を再実行し、観測対象を緩めて通さない。

## 9. 初回実動作確認の手順

### 9.1 比較条件

現行設定を維持したまま、baselineとcandidateの同じ初期状態を用意する。live DBを直接複製操作するのではなく、既存backup／隔離データの仕組みを使い、両方に同じ試験用の会話・状態を配置する。稼働アプリと混ざらないことを確認する。

baselineとcandidateは同じ観測コードを持つbuildで、context modeだけ切り替える。各試験開始時の保存履歴・World・辞書pending・Scopeを同じ状態へ戻すのは隔離fixtureだけ。本番のユーザー設定・会話を消さない。Memory ON／OFFを比較する場合も隔離コピーごとに条件を固定する。

記録する条件はbuildと作業差分digest、要求したモデルと返されたモデル、Provider構成fingerprint、input／output予約、推論設定、提示Tool集合、Memory設定、初期DB fixture、試行順、時刻。tokenやsecretはreportへ含めない。

キャッシュのcold／warm状態を制御できなければ「cold」と断定しない。baseline→candidateの順序だけでcache温度の効果を混ぜず、比較順を交互にする。キャッシュのクリアAPIを仮定せず、観測可能な状態を記録する。

### 9.2 会話シナリオ

| シナリオ | 操作と観察 |
| --- | --- |
| 同Topic継続 | 「SAAAのメモリ実装はContextから始めたい。初回は文字会話で確認したい」から、理由、対象、後回しの機能、検証、結果の整理を20turn程度継続する |
| 現在の訂正 | 中盤に「初回は音声も短く確認する。ただしProfile実装はまだ含めない」と訂正し、後続回答が新条件を使うか確認する |
| Tool往復 | 固定fixtureでは検索→本文取得→回答を再現。実環境では許可された小さなWeb検索、会話recall等を使い、残り回数と出典を確認する |
| 辞書保留 | 試験用の語で提案→確認→承認または棄却を行い、保留状態の表示と重複・旧版操作を確認する |
| 根拠失効 | fixtureで応答中のsource削除・Scope変更を発生させ、古い本文の保存とTTSを拒否する。通常会話では自然文の訂正を確認する |
| 音声smoke | 普通の短い回答とTool後の回答を読み上げ、先行音声、取消、重複再生が変わらないことを確認する |
| 障害 | usageを返さないfixture、SSE拒否、Tool後のProvider失敗で、missingと説明付きterminalが一致するか確認する |

live Webの内容が変わる試験は固定fixtureの品質比較と分ける。20turnは通常の会話継続として比較し、途中で「記憶している」と自己申告しただけでは合格にしない。対象の判断・条件を実際の回答と原典参照で確認する。

ASR／音声logicは今回変更しない。変更が必要になった場合は範囲を再評価し、macOSでTTSのみからASR発話が作られないことと、TTS中の人の発話がASRへ届くことを両方確認する。再生中gateを導入しない。

### 9.3 停止時に残す証拠

`spec/evidence/memory-context-first-trial/` を実装・試験時に作り、次を残す。今回、空の結果や仮の合格記録は作らない。

- `baseline.md`: 対象build・初期条件・既知の失敗・実行した試験。
- `requests.jsonl`: 許可した数値・digest・reasonのみのrequest／attempt計測。
- `comparison.md`: Prefix変化数、usageの有無、cache、初回本文と公開本文の時間、完了時間、失敗・取消。
- `trial-notes.md`: 実際の会話の期待と観察、継続・訂正・Tool・音声の体感、問題の再現条件、次の判断。

本文の確認が必要な場合は既存の原典IDへ戻る。audit本文記録を無断で有効にしたり、secretや個人会話全文を計測exportへ複製したりしない。

## 10. 合格・保留・失敗と次の判断

| 結果 | 判定と対応 |
| --- | --- |
| 固定Prefix一致、必須情報・訂正・Tool・保存・音声が成立 | 初回構造確認は合格。実cacheと長期Memoryの完成は別に判定する |
| Prefixは一致するがusageなし・モデル内部cache不明 | cache効果は保留。Provider計測の残件を明示し、token値を推定しない |
| cache利用あり、会話の回帰なし | 効果と費用・遅延を記録し、限定試験の次を検討する |
| 固定部分が変化する | 最初の差分とreasonを調べ、可変情報・Tool順・serializer・設定差を修正する |
| 訂正・根拠・容量・取消・音声の回帰あり | 不合格。candidateを戻し、同じシナリオで修正・再確認する |
| Prefix以降でMemoryや履歴が頻繁に変わる | この段階の限界として記録。Snapshot／EpochとCheckpointの優先課題へつなぐ |

初回の20turn×baseline／candidateは動作を観察する試験であり、p95や10%改善を統計的に実証する試験ではない。件数・欠測・失敗を併記し、中央値と個別値を中心に見る。p95等を出す場合は標本数と暫定性を示す。

性能の許容劣化幅と本採用に必要な標本数はC0でbaselineのばらつき・本人の体感目標を見て、candidate結果を見る前に定める。旧版の10%改善・5%非劣性を根拠なく流用しない。初回試験後の正式採用や既定化は別の判断とし、効果未確認を完成へ変えない。

C4後に相談するのは、固定Prefixのまま試験を続けるか、計測を補うか、次にSnapshot／公開EpochとCheckpointを一体で実装するか、である。Profile等をどこまで次に含めるかも、この会話と送信Contextの証拠を見て決める。

## 11. 限定有効化と切り戻し

内部フラグ `SAAA_CONVERSATION_PREFIX_MODE=legacy|stable` を実装した。既定はlegacy。計測は両modeで同じにする。既存`SAAA_CONTEXT_SEGMENTS`や保存済みMemory／Provider設定へ相乗りしない。

modeはjob開始時に読み、そのjobのTool往復中は固定する。進行中の処理へ無造作に切り替えない。modeを切り戻しても会話の正本、受理済みjob、辞書、設定、原典を保持し、失敗・取消・結果不明を既存queueの契約で終端させる。実施済みTool操作を自動で逆実行・再実行しない。

新規schemaは予定しない。もし保存表やIPCが必要になった場合はこの計画の差分を先に示し、コピー／隔離DBでmigration・旧版復帰を検証する。起動失敗を保存設定の初期化やProvider交換で回避しない。

計画本文の予定・未実施表現は作成時点のもの。2026年10月1日の実装・試験結果は以下と証拠ディレクトリを正とする。


## 12. 実装結果と停止時点（2026年10月1日）

| 段階 | 結果 |
| --- | --- |
| C0 | 同一の観測コードでlegacy／stableを比較できる隔離fixtureを実装。実設定は読み取りのみ。本番DB・Provider設定を変更していない |
| C1 | 固定Tool定義を名前とobject keyで正規化。Runtime、Scope、Worldを非命令の参照領域へ配置。最終messagesの予算検査とRequired保護を実装 |
| C2 | 単一Ornith queueの全stepに接続。job内mode固定、最新の残り回数・pending、一度だけの現在入力を確認。根拠失効時の表示・保存・TTSを拒否 |
| C3 | Memory OFF／ONで各mode20ターンのfixtureが合格。SSE／JSON usage、fallback、部分回答後の再生成禁止、Tool上限6、取消、辞書、復旧、ASR受付を確認。全体gateは別の不整合で未合格 |
| C4 | stableのmacOS bundle起動・画面・IPC確認は合格。実モデルはlegacy14ターン、stable8ターン成功後にJSON契約不一致で停止。20ターンの実会話受入、実cache、物理音声は未合格／未確認 |

新規ファイルの最終構成は `context_compiler.rs`、`context_metrics.rs`、`queue_runtime/conversation_answer.rs`、transportの `observation.rs` と `json_response.rs`、fixtureの `context_trial.rs`／`context_live.rs`、offline report。既存のqueueとfixtureは責務ごとの実submoduleへ移し、旧機能を保つ。DB schemaとIPCの追加はない。

試験中、失効したLARM接続の再利用と、Memory ON時のTool上限案内の読み上げ拒否を修正した。公開本文または音声が始まった後は、通信失敗・不正形式でも別回答を作って置き換えない。制御文はchunk境界をまたいでも公開しない。Tool後の案内文も根拠を確認した同じstreaming speech経路を使い、閉じたrunのWorldアクセスを再許可しない。

構造の結果は[comparison.md](../../spec/evidence/memory-context-first-trial/comparison.md)、条件は[baseline.md](../../spec/evidence/memory-context-first-trial/baseline.md)、試験と広いgateの制約は[validation.md](../../spec/evidence/memory-context-first-trial/validation.md)。安定化のコードは試せる状態だが、既定化やC4合格は宣言しない。まず実モデルのJSON出力契約を解決して同条件の20ターンを再実施する。Snapshot／公開Epoch・Checkpoint＋Tail・Profile・Episodeへ自動で進まない。
