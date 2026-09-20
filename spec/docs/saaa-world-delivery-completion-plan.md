# 現在の理解を全Provider・音声経路へ届ける実装計画

作成日: 2026-09-21。状態: 設計、実装未着手。担当想定: Terra。

## 1. 解消する弱点と完成状態

目的は「World Modelに状態があるのに、選んだProviderや音声では理解に使われない」を解消すること。ユーザーが仕事の対象を選び、テキストでも音声でも「今何を進めている」「さっきの仕事は終わった」と尋ねると、同じ正本と時点に基づく回答を得られるようにする。

全経路で同じ文章を生成する保証ではない。同じScope、必須条件、現在状態の根拠が、各adapterの実際の入力へ届き、不明・失効・取消を同じ規則で扱うことが完成条件である。単に `world_free_history` を取り除くだけでは完了しない。

上位は [Personal AI Concept](saaa-personal-ai-concept.md) §4・§5・§8、[World Model Concept](saaa-personal-world-model-concept.md)。[M3B計画](saaa-personal-world-model-m3b-plan.md)の限定投入・未接続を解消する後続計画。M3Bの「再生成0」は当時の段階制約であり、本書では新しいgeneration/attemptごとに送信直前のframeを構成する。ただし失効したframeのTTLを書き換えて復活させない。

## 2. 依存と担当範囲

必須の前提は [重要Context計画](saaa-required-context-completion-plan.md)のRC完了。[委任仕事計画](saaa-delegated-work-completion-plan.md)のDW-C1/C3/C5をTask状態sourceの契約とする。Worldの純粋型・adapter調査は先行できるが、Taskを含む統合受入はDW完了後。

本書はScope選択のUI、World source接続、frameの鮮度、全Providerへの変換、stateに関する回答の根拠を所有する。Goal/委任の変更と実行開始はDW、必須項目と予算はRC、経験からの選択変更は [学習計画](saaa-adaptive-improvement-completion-plan.md)が所有する。

## 3. 現状と全経路の棚卸し

2026-09-21の作業ツリーを確認した。以下は固定された現在仕様ではなく、WD-00で再確認する開始基準。

| 対象 | 現状と接続先 |
| --- | --- |
| 通常会話 | `runtime/conversation_turn.rs` でcompose後、非対応経路にはWorldを除いた履歴を渡す |
| frame要求 | `runtime/context/world/turn.rs` は明示ProjectとCoding参照が中心。`graph_request: None`、TTL 1,000ms |
| OpenAI互換 | `providers/chat_completions/world_body.rs` はrequestごとの再検査と除去を持つ |
| DynamicLan | `providers/stream/dynamic_lan.rs`。接続確保・allocation後の入力作成へ接続する |
| LARM共有音声 | `providers/stream/larm_voice.rs`。音声sessionからLLM利用権を得た後に同じ入力処理を使う |
| AgentSession | `providers/agent_session.rs` と `agent_session/sse/generation.rs`。session、follow-up、manifestを合わせる |
| Codex | `runtime/codex_*.rs` と実際の呼出元を追跡。CLI/SDK経路の現在の有効範囲を記録する |
| reasoning MCP | `runtime/conversation_controller/mod.rs`、`providers/reasoning_mcp/`、`crates/reasoning-contract/`、`services/reasoning-mcp/` |
| UI Scope | `src/lib/runtime.ts` にscopeRefs型がある。Chat/Codingから値を渡す実導線を確認・実装する |

Rustのpathは `src-tauri/src/` 相対。会議状態は専用Meeting実行機能が存在する前提にせず、現行Situationが観測した分類とCalendar観測の範囲を明示する。

## 4. 共通状態入力の契約

### WD-C1 Scopeをユーザー体験から解決する

Chatに現在のProject/Taskを示す小さな対象表示と切替を設け、Coding workspace登録時にresource→project→taskの関係を正本へ登録する。ユーザーの明示選択、採用済みTask参照、登録済みlinkからだけresolved Scopeを作る。

単一対象が特定できる「この仕事」「今のProject」は明示focusへ束縛する。複数候補なら選択UIまたは会話確認へ。文面類似度だけで別Projectへ切り替えない。セッションを作り直す操作を前提にせず、Scopeを変えたら以後のgenerationへ反映する。

Projectがなくてもuser Scopeで許可されたSituationは読める。task/resource状態には所有Scopeを要求する。現在状態を使うためだけに架空Projectを作らない。

### WD-C2 状態sourceと意味

新規予定 `runtime/context/world/inputs.rs` にtyped request、`WorldSourceSnapshot` を置く。既存World coreとFrameServiceを拡張し、別World DBを作らない。

| source | 正本・扱い |
| --- | --- |
| 現在の会話対象 | resolved Scopeと登録済みProject/resource metadata |
| 任された仕事・Goal | DWのTask/Goal/委任参照と既存coding/tool実行台帳。World内のGoalノードへ正本を複製しない |
| 現在の関わり方 | Situationのscene/attention/hold、観測時刻、signal health。分類は推定として表現 |
| 約束・期限 | schedule ledger。Calendarは観測または投影であり、委任の証明にしない |
| 記憶由来の現在理解 | 既存Worldの有効assertionと来歴。矛盾、失効、訂正、アクセス制約を維持 |

「会議アプリが前景」と「実際に会議中」、「ジョブがsettled」と「Goal達成」は区別する。観測できていないsourceは `unknown/unavailable` とする。confidenceをモデルの断定確率として作らない。

自然文からWorld候補を抽出する処理は既存Continuity抽出と別purpose・validatorにする。候補には実source ID/version、Scope、対象時点、proposed change、引用範囲を持たせ、pure reducerで採否を決める。モデルが述べたTask完了や権限は採用せず、専用Runtimeの正本を参照する。これにより「抽出対象外のまま」を最終状態にしない。

### WD-C3 Frameの寿命と送信順序

Provider接続/モデル能力の確保 → 最新source snapshot → RCのcompose → adapter serialize → source版/TTL再検査 → dispatch receiptの順。遅い接続確保の前に短命frameを作らない。

Frameには `frame_id / scope_digest / source_versions / observed_at / as_of / expires_at / omitted_reasons / digest`。SQLite sourceは一つのread transactionで読む。Situationなどメモリ上のsourceはsnapshot sequenceを保持し、DB snapshotとの組合せをdispatch前に再検査する。混在した時点を単一の原子的snapshotと偽らない。

1 attemptでcompose前の状態変化により作り直すのは高々1回。その後も変わり続ける場合、状態に依存しない要求はWorldを省略して理由を記録し、状態が必須の要求は `state_unstable` と具体的な確認応答を返す。RCの必須項目を省略して継続しない。

既存1,000ms TTLを便宜的に延長しない。source別freshness policyへ移す場合はWD-02で用途・観測周期・失効イベントを固定し、旧期限切れframeをCurrentへ戻さない。Tool follow-up、fallback、再開要求は新generationとして再構成し、TTL切れの旧frameを再利用しない。

### WD-C4 共通の送信インターフェース

新規予定 `runtime/context/dispatch.rs` の `PreparedDispatchContext` にRCの必須集合、World参照、Provider budget、scope digestを渡す。adapterはrender結果とsource manifestを返す。実wire bodyに載っていないsourceをselectedに記録しない。

Worldは独立したtyped blockとして扱い、結合文字列の完全一致だけで除去位置を決めない。system policyと引用データを分離する。Provider独自のroleやenvelopeへ変換しても、current instructionは一つ、World/Memory/Toolの命令権限はnoneを保つ。

共通receiptからgeneration依存と送信digestを記録し、送信直前の許可CASはRCへ委譲する。Providerの自動truncationに任せない。能力不足時は、同じ権限・送信許可内の対応Providerへ明示fallback、または対応不能を返す。

### WD-C5 session・音声・外部サービス

AgentSession/Codexは外部sessionが前のWorld本文を暗黙保持する可能性を扱う。完全入力を置換できる契約ならその機能を使用する。できなければ会話判断用sessionはgenerationごとに新規化し、Taskの進捗はhost正本から再投影する。UIの会話継続性と外部session IDを同一視しない。

継続Task用sessionを残す場合、最新制約・撤回を強制するtool boundaryと、古い入力を削除/無効化できるprotocolが必要。できないadapterを「同じ状態を理解」と表示しない。外部remote cacheの物理消去は別cleanup契約として扱う。

音声はASR確定後に現在Scopeと状態を取り直す。ASR開始時のWorldを発話終了後まで固定しない。TTS開始前にはSituation holdを再検査する。共有LARM sessionは維持しても、生成に渡す状態はrequestごとに独立させる。

reasoning MCPはContext/typed evidenceのschema版、request byte上限、必須領域をserviceと同時に更新する。旧版との接続ではhandshakeで判別し、必要な状態を削って旧版へ送る互換処理をしない。外部サービスの変更が必要ならadapter契約とrelease依存を成果物に含め、その経路の完了を保留する。

### WD-C6 状態に関する回答と失効後の結果

「今のTask」「現在の会議推定」「次の期限」など状態照会は、hostが検証できる `StateAnswer {claim, source_ref, as_of, status}` を内部形式にする。free textだけで現在性を保証しない。モデルに回答させた場合もclaimの状態値と出典を正本へ照合し、不一致部分を採用しない。

dispatch後に単にTTLが経過した結果は、送信時点の観測として区別する。関連source版・委任が変わった結果は現在状態の回答や次の操作に採用せず、最新状態の再取得へ。再生成は依頼あたり最大1回、予算不足/再変化ならhostの状態カードで不明または最新の検証可能な値を示す。

普通の会話全てを構造化回答に置換しない。外部操作は表示文面とは独立にRC/DWの現在状態検査を受ける。古い自然文応答を新しい権限の根拠にしない。

## 5. Terra向け作業カード

各カード最大5実装ファイルを目安に枝番化。Providerを一括で変更せず、共通契約を先に固め、一経路ずつwire受入を通す。

| ID | 対象と実装 | 合格条件 |
| --- | --- | --- |
| WD-00 | 全送信経路、session寿命、live接続条件、HEAD/dirtyのbaseline | reachable route一覧とadapterごとの入力/削除契約が確定 |
| WD-01 | ScopeのChat/Coding導線、project/resource/task link | 通常UIからresolved Project/Taskを実際に渡せる。曖昧Scopeの混入0 |
| WD-02 | WorldSourceSnapshot、source別freshness、typed block | source役割/時点/不明を表現。Task正本の二重化0 |
| WD-03 | Situation・DW Task・schedule sourceをFrameServiceへ | ProjectなしのSituation、Project付きTask、期限を正しく分離 |
| WD-04 | 自然文World候補抽出・validator・訂正/forget | 明示事実と推定を区別。モデル申告からTask完了/委任作成0 |
| WD-05 | just-in-time compose、bounded再構成、RC receipt | 接続確保がTTLより遅くても送信直前の状態を使う。無限再compose 0 |
| WD-06 | OpenAI互換のtyped renderingとfollow-up | body/manifest一致、既存tool loop維持、古いframeの再送0 |
| WD-07 | DynamicLanと共有LARM音声へ接続 | allocation/voice acquire後にcompose。text/voiceで同じ状態参照 |
| WD-08 | AgentSessionのcreate/resume/follow-upへ接続 | hidden historyから削除/訂正前状態が復活しない。必要ならsession新規化 |
| WD-09 | 現行Codex CLI/SDKの実送信口へ接続 | provider本文とmanifestを確認。Task進捗とsession stateを混同しない |
| WD-10 | reasoning contract/service/controller同時改訂 | handshake/サイズ/typed evidenceが一致。旧版へのsilent drop 0 |
| WD-11 | StateAnswer検証・現在性・TTS直前hold | 生成中の状態変化を現在の断定へ流用しない。誤ったclaimは表示しない |
| WD-12 | 設定で状態対応の可否と省略理由、source参照を提示 | Providerを替えたときの機能差を隠さない。本文をdiagnosticsへ出さない |
| WD-13 | 全経路matrix・実UI/実音声/実Provider受入 | §6を全て満たす。特定Providerだけの成功で完了しない |

## 6. 完全克服の受入条件

全経路matrixの行はOpenAI互換、DynamicLan text、共有LARM voice、AgentSession、現行Codex、reasoning MCP。各行で初回、Tool継続（機能がある経路）、fallback、Scope切替、訂正、忘却、session再開を確認する。Tool非対応は能力契約として明示し、存在する機能の試験をN/Aにしない。

| シナリオ | 合格 |
| --- | --- |
| Project Aの仕事実行→「今何を進めている」 | 正しいTask ID/状態/観測時点を根拠付きで回答 |
| Bへ切替→Aの結果到着 | Bの現在状態へAを混ぜず、Aの報告として表示 |
| 会議推定・不明・stale・終了へ変化 | 推定を確定事実にしない。TTS holdと説明が整合 |
| 接続待ち3秒、Tool実行5秒、その間にTask完了 | 旧1秒frameを再送しない。次generationは最新状態 |
| state変更/forgetがdispatch前後に発生 | 前なら送信拒否、後なら旧状態による新操作0。履歴としての観測時点を区別 |
| Providerと音声を切替 | 同一scope/source snapshot条件で同じ事実集合がwireに存在 |
| 同名Project・対象なし・二つのGoal | 推定だけで混ぜず、候補確認またはuser Scopeへ限定 |

状態照会の実モデル受入は各経路につき正常・不明・訂正・Scope切替・期限切れ・生成中更新を各5例、計30例。日本語表現と状態遷移fixtureをWD-00で凍結し、model/contract版を記録する。検証可能なstate claimの誤り0、source mismatch 0。状態カードへの縮退は区別して集計し、正常5例が全て縮退ならその経路は未達。

wire一致はstub server、判断・提示は実Provider/実UIで別々に確認。資格情報のない経路は未検証・未完了とする。クラウド禁止条件をlive確認のために緩めない。許可された経路だけを実行し、残る依存を報告する。

性能目標は同一環境で共通frame/serialize追加p95 30ms以内、既存TTFA p95悪化10%以内。接続確保時間、モデル生成時間、host overheadを分ける。値は本計画の設計目標であり実測済みではない。

検査: 新規Rust `wd_`（0件不可）、既存 `runtime::context` / `m3b_` とProvider対象、`bun run test:rust-packages`、`bun run ipc:check`、`bun run size:check`、`bun run check:local`、`bun run spec:check`。契約/IPC変更時は生成物を更新する。

## 7. 成果物と開始指示

`spec/evidence/world-delivery/{baseline,progress,route-matrix,results}.md` を作成。機能対応・body投入・manifest・失効処理・実モデル回答を別列で記録する。M3Bの未接続一覧を更新し、解消していない行を削除しない。

Terraへの開始指示例:

> WD-00から順に実装してください。RCの送信契約とDWのTask正本を使い、ScopeのUI入口から各Providerの実際のwire body、音声応答まで接続してください。World除去の単純削除やTTL延長で済ませず、session内の古い理解と生成中の状態変化を扱い、route-matrixの全行を受入してください。
