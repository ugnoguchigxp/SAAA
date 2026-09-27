# 重要な記憶・制約を欠落させない共通Context 実装計画

作成日: 2026-09-21。実装状況更新: 2026-09-21。状態: 一部実装・自動検証済み。実モデルおよびネイティブ UI の受入は未完了。担当想定: Terra。

## 1. 解消する弱点と完成状態

目的は「記憶を保存したのに、次の判断では重要な条件が抜ける」を解消すること。単に `Should` を `Must` に変更するだけでは完了しない。現在Scopeに有効な目的・制約・明示訂正・未完了操作の対応関係を、予算配分、Provider変換、Tool継続、fallback、訂正・忘却競合を通して保持する。

完成時、ユーザーが「この仕事では外部送信しない」「先ほどの採用は取り消し」と伝えた後、長い会話、話題切替、再起動、モデル変更を挟んでも、その条件に反する実行へ進まない。必要な条件だけでもモデルに収まらない場合は、条件を落とさず、対象を絞るための説明を返す。

上位は [Personal AI Concept](saaa-personal-ai-concept.md) §5.1・§6・§15。[共通Runtime計画](context-memory-unified-runtime-implementation-plan.md)を継承する。本書は新しいMemory正本を作らず、既存投影の必須性と送信契約を完成させる後続計画である。

## 2. 四計画の依存と所有境界

推奨実装順は **本書 → [委任の継続](saaa-delegated-work-completion-plan.md) → [全経路への状態供給](saaa-world-delivery-completion-plan.md) → [経験による改善](saaa-adaptive-improvement-completion-plan.md)**。別々に完了報告する。

| 所有者 | 所有する変更 | 本書との契約 |
| --- | --- | --- |
| 本書 RC | 必須項目判定、Context予算、送信前検証、欠落時の応答 | `RequiredContextSet` / `DispatchContextReceipt` を定義 |
| DW | Goal/委任/実行の正本、非同期報告 | 正本から必須条件と参照を供給。Memoryから権限を作らない |
| WD | World source、ScopeのUI導線、各Providerの変換 | 本書の必須項目検証を全adapterで使用。予算規則を複製しない |
| AI | 選択・計画・通知の適応 | 必須条件より学習結果を優先しない |

本書は既存の全送信経路で「必須項目が失われた送信を拒否する」まで所有する。WDは同じAPIでWorldも送信可能にする。RCの完了をWDの未完成で代替せず、WDの未対応を通常成功扱いしない。

## 3. 着手時に読む現行実装

確認基準は2026-09-21の作業ツリー。並行更新があるためRC-00で再照合する。

| ファイル（repository rootから） | 確認した状態 |
| --- | --- |
| `src-tauri/src/memory/personal_state/projection.rs` | 抽出済みassertionは一律 `Should`。未処理sourceは `Must` |
| `src-tauri/src/memory/context_window.rs` | 履歴・継続情報を先に組み立て、固定byte予算を使う |
| `src-tauri/src/runtime/context/broker.rs` | baseの使用量にcandidateを加算。Shouldは予算超過時に省略可能 |
| `src-tauri/src/runtime/context/{source,generation,generation_inputs}.rs` | requirement、来歴、dispatch/完了検査の基盤 |
| `src-tauri/src/runtime/conversation_controller/mod.rs` | reasoning向けに件数制限と先頭削除で再縮小している |
| `src-tauri/src/providers/chat_completions/world_body.rs` | Broker後にhost文面を追加。最終wireサイズの再評価が必要 |
| `src-tauri/src/memory/personal_state/{worker,journal,projection}.rs` | 意味抽出、訂正・忘却、未処理sourceの保持 |

計画中の型・ファイル名には「新規予定」と付す。既存関数が移動していても同じ責務へ接続し、古いpathの復活をしない。既存dirty差分を巻き戻さず、schema番号は着手時の最新値+1とする。

## 4. 必須Contextの契約

### RC-C1 必須性と命令権限を分離する

新規予定 `runtime/context/required.rs` に純粋判定を置く。`required` は送信に必須という意味であり、system命令への昇格ではない。抽出文と引用は `UntrustedData` のまま。ホストが所有する委任・送信禁止はモデルの解釈に依存せず実行境界でも強制する。

| 項目 | 扱い |
| --- | --- |
| 現在のユーザー入力 | 既存の唯一の現在入力として必須。source本文の複写で二重化しない |
| 現在Scopeのactive objective / constraint / decision / pending_decision / open_loop / active_referent / progress_ref | 現在状態の最小表現を必須。根拠全文は任意取得へ分離 |
| 明示訂正・取消 | 現在値と訂正元参照、適用Scopeを必須。旧値を独立した有効条件として残さない |
| disputed / candidate | 現在の目的・制約・対象の解釈を左右する場合、不確実性を含めて必須。未採用を採用済みにしない |
| 未処理の関連source | 意味抽出が完了するまで原文を必須として保持。抽出失敗を理由に省略しない |
| 未完了Tool call/result、実行Task、委任参照 | 継続に必要なID・状態・制約・対応関係を必須。Tool本文は別予算 |
| Scope外、削除済み、失効済み、superseded | 送信対象外。過去履歴から復活させない |
| それ以外の過去の経験・関連資料 | Should/May。必須領域を消費する前に選ばない |

「現在状態の最小表現」はID、kind、semantic key、現在値、status、scope、source/version、訂正関係。値の意味を削る要約は禁止。単に頻出であることを必須判定に使わない。scope未解決なら混ぜず、確認用応答へ進む。

### RC-C2 型と正本

新規予定の型は既存Candidateへのラッパーとして定義する。

- `RequiredContextSet`: set revision、scope digest、policy/input epoch、ordered item IDs、必要byte数、状態。
- `RequiredItem`: candidate/source参照、required reason、dependency refs、state version。本文の新しい永続コピーを作らない。
- `ProviderInputBudget`: model input上限、transport byte上限、出力予約、schema/host wrapper予算、token見積方式。
- `DispatchContextReceipt`: generation/attempt ID、required-set digest、実際の送信body digest、source version集合、omission理由。

既存 `context_generations` / input記録へ必要列を加算する。JSON全文を監査やdiagnosticsへ複写しない。予算がbyteのみでしか確認できないProviderは、その限界と保守的換算方法を明示し、byte数をtoken数と呼ばない。

### RC-C3 予算の順序

1. 接続先modelの能力と上限を解決する。fallback先は別budget。
2. 現在入力、host policy、必須項目、未完了Tool対応、実際のtool schema、出力予約を先に確保する。
3. 残りへ直近履歴・継続履歴・任意Memory・Worldを収容する。既存baseを無条件で先取りさせない。
4. adapterで実際のwire形式へserializeし、追加wrapperを含むサイズと必須項目の存在を再検査する。
5. 超過なら任意項目だけ減らして再構成を高々2回。必須項目だけで超過なら `required_context_overflow`。LLMによる必須条件の自動要約で迂回しない。

同じ根拠の重複はsource ID/versionと項目IDで除去し、異なるScopeの同じ文を同一視しない。必須件数上限を超えた場合も明示的なoverflowとする。全送信が常に成功する保証ではなく、必要情報を捨てて成功に見せない保証である。

### RC-C4 世代・訂正・忘却

composeはread snapshot上のsource版に束縛する。dispatch直前に同じwriterでdependency、Scope、委任参照、policy epochを検査し、generation状態をCAS更新する。このcommitを送信許可の線形化点とする。

訂正が先なら旧generationは送信しない。dispatchが先なら送信済みbytesを取り戻せるとは主張せず、取消要求を出して遅延結果の採用と後続Toolを禁止する。新入力の影響Scopeに属するrunだけを失効させ、無関係Taskまで止めない。

Tool follow-up、Provider fallback、音声reasoning、Codex/AgentSessionの次generationでも新しいreceiptを作る。adapter内部の先頭削除で必須項目を消さない。resumeされた外部sessionの履歴に訂正前の記憶が残る場合は、新しいContextを適用できるsession契約へ切り替えるか、対応不能を明示する。

### RC-C5 応答と操作導線

overflow、Scope競合、source失効は別reason code。UIに「どの対象の条件が収まらないか」と「対象を絞る／原文を確認する／記憶を訂正する」を示す。自動削除、勝手なクラウド移送、予算上限の自動緩和はしない。

意味抽出が曖昧な間は原文を保持し、明示確認で解決できる。忘却は既存journalへ接続。モデルが自然文を必ず正解するという保証に置き換えない。

## 5. Terra向け作業カード

番号順に実装。各カードは最大5実装ファイルを目安に、超える場合は枝番へ分割する。test、生成型、module登録は付随変更として記録する。

| ID | 対象・手順 | 必須試験と合格条件 |
| --- | --- | --- |
| RC-00 | 上記接続点、全Provider送信口、HEAD/dirty/schemaを `spec/evidence/required-context/` に記録 | 会話初回・follow-up・fallback・音声・外部sessionの一覧に漏れがない |
| RC-01 | 新規 `required.rs`、既存source型。C1の判定表を純粋関数化 | active/candidate/disputed/失効/Scope別の表形式試験。任意資料がMustへ無差別昇格しない |
| RC-02 | `personal_state/projection.rs` とworker結果の参照。訂正・未処理sourceを必須化 | 抽出成功前後とも条件が残る。訂正前後の二重採用0 |
| RC-03 | `context_window.rs`を責務別に分割し必須予約付きcomposeへ | 長い履歴より必須条件が優先される。現在入力1件、UTF-8境界を壊さない |
| RC-04 | Brokerの二段階配分とProviderInputBudget | 1 byte不足、wrapper増加、tool schema増加、小さいfallbackでsilent drop 0 |
| RC-05 | generation/input記録、schema加算、送信許可CAS | 訂正/forget/Scope revokeの競合順序双方を実DBで確認。古い結果の採用0 |
| RC-06 | chat-completions/OpenAI互換の初回とTool継続にreceiptを接続 | wire bodyに必須項目、記録digest一致。巨大Tool結果でも条件維持 |
| RC-07 | DynamicLan・共有音声経路へ同じ検査を接続 | 接続確保中の訂正、voice/text切替で欠落0。LAN専用の予算複製0 |
| RC-08 | AgentSession・Codex本文変換へ接続 | 新規/再開session、follow-up、失効後の応答で契約維持。未対応は成功扱いしない |
| RC-09 | reasoning controllerとreasoning契約へ必須領域を接続 | 32件超過、`fit_context`相当の縮小でも必須削除0。必要ならcontract版とserviceを同時更新 |
| RC-10 | Settings/Chatの不足・訂正導線とIPC型を更新 | 原因が区別できる。対象限定後に再送成功。記憶削除なしでも回復可能 |
| RC-11 | 全体シナリオ、移行、性能、実モデルでの意味確認 | §6を全て満たし、結果を作成。新flag OFFだけで完了としない |

RC-08/09の外部protocolに不足があれば、必要なcontract差分とadapterをこのカードで実装する。外部実装待ちならその経路は未完了と報告し、stubを最終成果にしない。

### 5.1 実装状況（2026-09-21）

以下はカードの設計上の完了ではなく、作業ツリーと自動検証に照らした実装状況である。**全Provider・実モデル・ネイティブUIを含む受入はまだ完了していないため、本計画全体を「弱点克服完了」とは扱わない。** 詳細なコマンド結果と再現条件は `spec/evidence/required-context/` を正本とする。

#### 実装済み

- **RC-01〜RC-03、RC-09:** 必須項目の分類、訂正・未処理sourceの保持、必須領域を先に予約するBroker、reasoning縮小時の必須項目保持を実装し、自動試験済み。
- **RC-04:** OpenAI互換・DynamicLan・AgentSessionの具体的な初期予算予約、最終wire検査、巨大Tool follow-upでの任意履歴だけを除く再構成を実装済み。
- **RC-05:** receipt/CAS、訂正・forget・Scope・Task/委任失効、Tool開始前の再検証、遅延応答の拒否を実装済み。AgentSessionの完了時も型付きContext失効を保持する。
- **RC-06〜RC-08:** OpenAI互換、DynamicLan、共有音声、AgentSessionの初回・Tool継続を共通の必須wire検査へ接続済み。
- **RC-10:** 3種類のContext拒否を区別するIPC型、表示文言、対象を絞る・原文を確認する・訂正する回復操作を実装済み。
- **RC-11のローカル検証:** provider設定とreceiptのmigration、候補1/100/512件の性能計測、Context試験を実施済み。

#### 未完了

| 区分 | 対象 | 未完了の内容 | 完了に必要なもの |
| --- | --- | --- | --- |
| 実装準備・実モデル評価 | RC-00 | 日本語固定60ケースは定義済みだが、設定済みモデルに実行していない | 応答可能な設定済み実モデルでケースを実行し結果を保存する |
| Provider契約 | RC-04 | Agent Connectionのprofileがモデル別入力上限・tokenizer・wrapper予算を返さない | Provider側がその能力契約を公開し、adapterで利用できること |
| 実通信受入 | RC-05〜RC-08 | local fixtureでは検証済みだが、実Providerで訂正・Scope変更・Tool継続・再開sessionの競合を通していない | ready状態のProviderと実通信での受入結果 |
| ネイティブ実機受入 | RC-10 | UIは実装済みだが、Tauri画面で3拒否状態と回復操作を観測していない | ネイティブ画面を操作できる自動化面、またはユーザーによる操作記録 |
| 総合実モデル受入 | RC-11 | §6の全シナリオ、全Provider、日本語60ケースを実モデルで完走していない | 上記Provider・ネイティブUI条件を満たした総合実行 |

| ID | 状況 | 実装済み | 残作業・未充足の受入条件 |
| --- | --- | --- | --- |
| RC-00 | 実装準備完了・実モデル評価待ち | Provider送信口の棚卸しと日本語固定60ケースを記録した | 設定済み実モデルでの60ケース実行結果を記録する |
| RC-01 | 実装済み・自動検証済み | `required.rs` がactive/candidate/disputedを分類し、失効・Scope外を除外する | 実モデル受入時にも分類結果を検証する |
| RC-02 | 実装済み・自動検証済み | 訂正と未処理sourceを必須として残し、古いassertion版のdispatchを無効化する | 実モデル受入時にも訂正の意味評価を行う |
| RC-03 | 実装済み・自動検証済み | Brokerが必須項目を先に予約し、収まらなければ欠落でなくoverflowを返す | 全体シナリオでの確認をRC-11で行う |
| RC-04 | Adapter実装完了・Provider能力契約待ち | concrete adapterごとの初期予約、最終wire検査、巨大Tool follow-upでの任意履歴だけを除く再構成を実装した | 接続先modelが入力上限・tokenizer・wrapper予算を返す契約と、その実機受入が必要 |
| RC-05 | 実装済み・実通信競合受入待ち | receipt/CAS、訂正・forget・Scope・Task/委任失効、Tool開始前再検証、Chat/AgentSessionの遅延結果拒否を実装・試験した。AgentSessionの完了時も型付きScope失効を内部エラーへ落とさない | 全adapterの実通信中競合を同じ条件で受入する |
| RC-06 | 実装済み・実Provider受入待ち | OpenAI互換/DynamicLanの初回・Tool継続で最終wire検査と任意履歴再構成を実装した | 実Providerでの初回・Tool継続・巨大結果の受入が未実施 |
| RC-07 | 実装済み・実通信受入待ち | DynamicLanと共有音声を共通検査経路へ接続した | 接続確保中の訂正とvoice/text切替の実通信競合受入が未実施 |
| RC-08 | 実装済み・実外部session受入待ち | AgentSessionの初回・Tool roundで検査と再検証を実装した | 再開した外部sessionと遅延応答の実通信受入が未実施 |
| RC-09 | 実装済み・自動検証済み | reasoningの縮小処理でも必須項目を保持し、必須のみの超過を拒否する | 全体シナリオでの確認をRC-11で行う |
| RC-10 | 実装済み・ネイティブ実機受入の観測手段待ち | overflow/Scope変更/source失効を区別するUI文言・IPC型・回復操作を実装した | ネイティブデスクトップで3拒否状態と回復操作を実際に通す。これはUI機能の追加実装ではなく、ネイティブ画面を操作・観測する手段が必要な受入条件 |
| RC-11 | ローカル検証完了・総合実受入待ち | provider設定migration、Required Context receiptの加算migration、候補1/100/512件のp95 20ms以内を自動確認した | §6全シナリオ、設定済み実モデルの意味評価、全Provider実受入を完了する |

現時点の残条件はすべて外部契約または実機受入である。ネイティブSAAA画面を操作できる自動化面（ブラウザのVite画面にはTauri IPCがない）、設定済み実モデルの応答完了、ならびにRC-04で必要なmodel入力上限・tokenizer・wrapper予算を返すProvider契約が必要になる。これらが提供されるまで、RC-05/07/08の実通信競合、RC-10の3拒否状態、RC-11の日本語60ケースと全Provider総合シナリオを成功として記録できない。

RC-10については、回復UI、IPCのfailure code、失敗時に自動再送しない制御は実装・自動試験済みである。追加のdebug IPCや強制失敗UIを製品へ入れて受入を代替しない。状態遷移の統合試験は追加できるが、Tauriネイティブ画面の表示・ボタン操作・遷移を確認する実機受入とは別の証跡として扱う。残る受入には、ネイティブ画面を操作可能な自動化面、またはユーザーによる同画面での操作記録が必要である。

## 6. 完全克服の受入条件

| シナリオ | 合格 |
| --- | --- |
| Aの禁止条件→長文100ターン→Bへ切替→Aへ復帰→再起動 | Aの有効条件が全対象generationに存在し、Bへ漏れない |
| 「採用」→「未決へ戻す」→古い抽出結果到着 | 現在は未決。古い決定の復活0 |
| 未抽出・抽出失敗・複数競合 | 原文/不確実性を保持。採用済み事実の捏造0 |
| 必須のみで容量超過 | dispatch 0、具体的回復導線あり。単にタイムアウトしない |
| dispatch前後のforget/委任撤回 | 先行撤回は送信0。後行撤回は以後の結果採用/Tool開始0 |
| 全Provider、音声、Tool loop、fallback | required-setとwire bodyが一致。省略経路がない |

自然文については、明示条件・訂正・引用・仮定・否定・Scope切替各10例以上、計60例以上の日本語固定ケースをRC-00で凍結する。各ケースに期待状態と禁止行動を人手で記載する。実際の設定モデルで重大な禁止違反0、不確実なケースは確認または保持へ。モデルの自己採点を使わない。

新しい予算処理の追加CPU時間は、1/100/512候補で基準比と絶対値を測り、同一環境でp95 20ms以内を目標とする。未達は機能完了と性能未達を分離し、上限を事後変更しない。モデルの待ち時間とは別計測。

基本検査: `cargo test --locked --manifest-path src-tauri/Cargo.toml --lib rc_`（新規prefix、0件不可）、既存 `runtime::context` / Personal State対象、`bun run ipc:check`（型変更時は先にgenerate）、`bun run size:check`、`bun run check:local`、`bun run spec:check`。reasoning契約変更時は独立crate/serviceも `bun run test:rust-packages`。

## 7. 完了報告と実装開始指示

成果物は `spec/evidence/required-context/{baseline,progress,results}.md`。カード別commitまたはdiff参照、実装済み/自動受入済み/実モデル確認済み/未完了を分ける。全経路の受入を満たすまで「弱点克服完了」と書かない。実接続が用意されていない経路は、必要条件を残して未完了とする。

Terraへの開始指示例:

> 本書のRC-00から番号順に実装してください。既存の並行変更を保存し、まずRequiredContextSetの契約と予算予約を完成させてください。Mustへの置換だけで完了せず、全Providerの最終送信と訂正・忘却競合まで受入を通してください。機能コードと証跡を更新し、未完了経路を隠さないでください。
