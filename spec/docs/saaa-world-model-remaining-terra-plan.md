# World Model 実装残件専用計画 — Terra作業カード

作成日: 2026-09-21。状態: 未着手の作業指示。対象は現在の作業ツリーからの差分実装。

## 1. 目的とこの計画の使い方

[全経路への投入計画](saaa-world-delivery-completion-plan.md) §0の実装不足を解消する。上位仕様は [World Model Concept](saaa-personal-world-model-concept.md)。五要素の既存graphを維持し、Situation・委任仕事・期限の正本を同じFrameから参照できるようにする。次に自然文からの候補作成、全Providerへの投入、現在状態の回答検証、UI表示を接続する。

本書は既存実装の作り直しを指示しない。M4A/G1のgraph・失効検査・通信runner、MCP v2の厳格handshake、Codexのmetadata receiptは再利用する。TTL延長、Worldの削除だけ、hostカードへの全面置換で完了扱いにしない。

カードは記載順に一つずつ実装する。各カードを「実装→指定の回帰→証跡更新」で閉じ、未合格なら次の依存カードへ進まない。独立カードは進めてよいが、並行エージェントや別タスクへの依頼は本書では指示しない。カード内で実装ファイルが5個を超える場合、型・実装・接続に枝番分割し、先に証跡へ分割案を書いてから実装する。テスト・mod宣言・生成物は別枠で数える。

パスはリポジトリ相対。以下では `R = src-tauri/src/`、`C = crates/personal-state-core/src/` とする。「新規」は予定パスであり、既存APIと同名の機能が見つかった場合は再利用して実際のパスを証跡へ記録する。

### 完了を二段階に分ける

- **実装残件完了**: T00〜T24の実装とoffline受入が合格し、共通Frameの全source・全adapter・回答検証・UI導線がつながった状態。
- **製品受入完了**: 実装残件完了に加え、親計画WD-13の実モデル・実UI・実音声・性能・全体品質ゲートが合格した状態。

実モデル接続がなくてもT00〜T24を進める。資格情報や接続の不備はlive項目だけを未検証にし、実装残件の停止理由にしない。size/Clippyの既存違反はT00で記録し、担当ファイルの違反と今回増やした違反は解消する。全体の既存違反を残した場合は製品受入未完了とする。

## 2. 今回固定する契約

### 2.1 正本と許可

| 情報 | 読取り元 | Frameへ渡す値 | 禁止する扱い |
| --- | --- | --- | --- |
| 対象 | `R/runtime/context/scope.rs` の保存済みresolved Scope | focus、許可scope、epoch、digest | 文面類似度による別Projectの混入、読取り時の自動登録 |
| Situation | `R/situation/mod.rs` のRuntimeInnerをlock中にコピーする専用API | scene、attention、hold、signal health、観測時刻、sequence | ウィンドウ名・画面内容の投入、会議推定を確定事実にすること |
| Coding | `coding_jobs` と必要な現在run | ID、正本状態、revision、owner digest | settledをGoal達成へ変換 |
| 委任 | `steward_goals` / `steward_delegations` / `steward_tasks` と所有関係 | 各ID、status、loop_state、revisionが存在すればrevision、状態digest | World側への権限・Goal達成状態の複製、別conversationの暗黙読取り |
| 期限 | `schedule_entries` | ID、scope_ref、status、due_at、revision | Calendar観測から委任・実行権限を作成 |
| 五要素graph | 既存Worldの検証済み投影・来歴 | 既存WorldSliceV2の有効部分 | 新しいWorld DB、runtime状態をgraphへ重複保存 |

DWの列名・所有関係はT00で現行schemaから確定する。conversation ID一致だけでProject横断を許可しない。登録済みworkspace/resource/project linkと許可Scopeへjoinする。明示的に許可されたconversation由来のuser-scope委任がある場合だけその契約を使う。所有関係不明はunavailable、権限不一致はdenied。World用の架空linkや権限を追加しない。

### 2.2 Frameとsource型

新しいwireは **WorldFrame schema_version=2** とする。既存graph/runtime型はできる限り保持し、`project_scope`の代わりに `scope {focus_scope_key, allowed_scope_keys, digest}`、`sources`、source別stampを導入する。Projectがない場合は、保存済みuser Scopeで許可されたSituationだけを扱える。graphは既存のProject検証を通った場合のみ投入し、user Scopeから全Projectを探索しない。

`WorldSourceEntry` は `kind / source_id / owner_scope_key / availability / observed_at_ms / as_of_ms / version / digest / payload`。availabilityは `available / unknown / unavailable / stale / denied` の列挙。availableだけがtyped payloadを持ち、それ以外は固定reason codeを持つ。deniedのID・payloadはproviderへ出さず、host側の省略理由だけに残す。空リストとunavailableを区別するsource group statusも保持する。

versionがない正本に偽のrevisionを付けない。既存revisionと、意味のある全状態列のcanonical digestを併用する。digest対象から読取り時刻だけを除き、同じrevisionの状態書換え、選択集合への追加・削除も検知する。source別stampには正本の種類・所有scope・query条件・集合digestを含める。

初期上限はFrame全体8,192 UTF-8 bytes、graph上限6,144 bytes、Coding/委任Taskは合計8件、期限8件、Situation1件。これらの上限は加算して全量を保証するものではない。必須状態→現在focus→期限昇順（同時刻はID順）→その他→graphの順で予算配分する。必須状態が入らなければRCの送信拒否とする。旧runtime用2,048 bytes上限との矛盾をcore側で解消し、必須情報を黙って切らない。追加状態はgraphの五要素ノード件数へ数えない。

### 2.3 時点と再検証

- DBの全sourceは既存SqliteReadersの一つのread transactionで読む。Situationのコピー中にDB lockを取らず、DB読取り中にもSituation lockを取らない。
- `Situation before → DB snapshot → Situation after` のsequenceを比較する。不一致ならsnapshot全体を高々1回取得し直す。まだ変わる場合はstate_unstable。
- Frameの有効期間は従来どおり最大1,000ms。元stampの期限を検査し、再検証で期限を延長しない。Situationの観測鮮度はmonitorの既存設定・signal healthを使用し、取得時刻を観測時刻に置換しない。T00で既存の鮮度基準がなければT03で `max(3 × sample_interval_ms, 1,000ms)` を明示的に実装・境界試験する。
- Situation sequenceはFrameへ出す意味的な状態、health、monitor有効状態の変更時に進む。変化のないtickや監査ログ追加では進めず、観測時刻だけを更新する。時計逆行・monitor停止・snapshot不能はcurrent扱いにしない。
- provider接続確保→snapshot→RC compose→serialize→正本版・Scope・TTL再検証→generationのdispatch CAS→送信。同じgenerationのbodyをreceipt記録後に書き換えない。
- 再検査時の変化は送信前に新generationで高々1回再構成する。snapshot内retryとこのretryは別に数え、さらに外側で無限retryしない。2回目の変化は必須ならhostのstate_unstable応答、任意ならWorldなしの新body・新receiptにする。
- 回答受理時はsource変更とTTL経過を分離する。TTLのみ経過した回答は送信時点の観測として扱う。Scope/権限/関連source変更は現在状態の断定・次操作に使わない。DBとSituationを原子的だったと記録しない。Situationは受理・audio queue境界でもsequenceを再確認する。

### 2.4 自然文抽出

既存Continuityとは別の `world-extraction` purposeとstrict schemaを作る。対象は許可された現在のuser messageだけ。assistant回答・Tool出力・過去履歴の引用をuserの申告へ昇格させない。既存schedulerの重複抑止・キャンセル・資源制限を再利用し、専用の常駐pollerを追加しない。

候補は `source_message_id / source_version / scope_key / quote_start_byte / quote_end_byte / target_time / proposed_change / epistemic_status`。hostがmessage ID・version・scopeを付与し、モデルの値で上書きさせない。UTF-8境界・引用一致・現存source・Scope・許可されたpayload種別を検証する。引用は根拠の所在であり真実性の保証ではない。user_reportedとinferredを区別し、因果推定は仮説として保持する。

五要素は既存の型とvalidatorに従う。モデルから実行権限、委任作成、runtime Task完了、Goal達成の確定変更は受け付けない。訂正は対象sourceと既存assertionを同定できる場合だけ遷移へ変換し、曖昧ならpending。冪等キーはsource ID/version・scope・候補canonical digestからhostで作る。commit直前に再検証し、forget/編集後に古い候補を復活させない。

### 2.5 現在状態の回答

既存host StateAnswerを広げつつ、一般会話は維持する。現在状態を答える経路は `StateClaim {kind, source_ref, source_version_or_digest, value, as_of_ms}` を構造化出力として受け、hostが正本へ照合して表示文を生成する。モデルが返した自由文に検証済みバッジを付ける方式は禁止。

現在状態の回答モードでは検証前の本文・音声を配信しない。モデルの自由文は回答の正本にせず、検証済みclaimから本文を組み立てる。構造化出力非対応はhostカードへ縮退し、理由を表示・集計する。一般会話をすべてbufferしない。状態照会分類はモデルに権限を与えず、複合依頼でTool実行までhostカードで握り潰さない。

Scope切替後の結果は元Scopeの報告として残すか拒否し、現在Scopeの回答へ付け替えない。再生成は一つのuser依頼で最大1回。Tool実行の再許可はRC/DWが所有し、World claimの成功を許可の代わりにしない。

## 3. 作業カード

各カードの試験名は指定prefixを含める。テスト配置は近接する既存test moduleまたは同名の新規 `*_tests.rs`。以下の「合格」は正常系だけでなく、指定した反例が失敗側へ分岐することをassertする。

### T00 — 現行正本と前提の固定（WD-00）

- 変更: `spec/evidence/world-delivery/remaining-baseline.md`（新規）。コード変更なし。
- 読む: `R/steward/{schema,contracts,repository}.rs`、`R/runtime/context/{scope,required,generation}.rs`、`R/situation/{mod,contracts}.rs`、`R/schedule/` のschema定義。
- 作業: HEAD/dirty、正本の実列名・所有join・revision欠如、Situation鮮度、全Provider呼出元、自然文抽出の既存scheduler入口を表にする。未接続・既存ゲート失敗も記録。
- 合格: 各sourceに「どの値を、どのScopeで、何を再検証するか」が一意。DW契約不備があれば該当sourceカードのみblockedとし、型・Situation・期限は続ける。

### T01 — 共通Frame v2の純粋型（WD-02）

- 依存: T00。変更: `C/world/runtime_frame.rs`、新規 `C/world/frame_sources.rs`、coreのmodule宣言。
- 作業: §2.2の型・canonical digest・上限を実装。新規runtime状態をCodingOwnerStateへ押し込めず、source payloadの列挙として追加する。
- 合格 `wr_t01_`: user/project scopeのserialize、unknownと空集合の区別、不正availability/payload組合せ拒否、決定的digest、UTF-8全体byte上限。v1 fixtureを黙ってv2扱いにしない。

### T02 — Scope認可をuser/projectへ分離（WD-01/03）

- 依存: T01。変更: `R/memory/personal_state/world/runtime_scope.rs`、`R/runtime/context/world/turn.rs`、必要時 `R/runtime/context/scope.rs`。
- 作業: Project必須の判定をsource別に分ける。user ScopeのSituationを許可し、Task/期限/graphは所有Scopeを検査。既存principal・classification・policy・input epoch検査を残す。
- 合格 `wr_t02_`: ProjectなしSituation成功、同名の他Project拒否、二候補focus未解決、epoch変更、低classification、読取りでlink/Scopeが増えない。

### T03 — Situationの読取りAPI（WD-03）

- 依存: T01。変更: 新規 `R/situation/world_snapshot.rs`、`R/situation/mod.rs`、`R/situation/tick.rs`。
- 作業: RuntimeInnerから§2.1の値だけをコピーするAPIと意味的sequenceを追加。disabled/stale/poisonedをavailableにしない。既存next_revisionの意味が一致しなければ専用sequenceを使う。
- 合格 `wr_t03_`: scene/hold/health/有効状態変更でsequence進行、同一tickで不変、stale境界、時計逆行、停止、推定ラベル、本文やwindow titleがserializeされない。

### T04 — 委任source adapter（WD-03）

- 依存: T00/T02。変更: 新規 `R/memory/personal_state/world/runtime_delegation.rs`、`R/runtime/context/world/inputs.rs`。
- 作業: 許可Task→delegation→Goalを正本からjoin。Coding Taskとの関係はIDで保持して二重計上しない。Goal・委任・Taskをそれぞれの状態として投影し、report本文・実行権限は渡さない。
- 合格 `wr_t04_`: withdrawn/paused/failed/done、同revision状態変更、Goal更新、削除、同conversation別Project拒否、権限不明でdeny、settled≠Goal達成。

### T05 — 期限source adapter（WD-03）

- 依存: T02。変更: 新規 `R/memory/personal_state/world/runtime_schedule.rs`、`R/runtime/context/world/inputs.rs`。
- 作業: 全許可Scopeのscheduled/firingを決定的に上限8件へ。status変更、削除、より早い期限の追加も集合stampへ含める。個別source stampだけにしない。
- 合格 `wr_t05_`: 別Scopeの最早期限、同時刻のID順、取消・再予定・追加・削除、同名他Scope非開示、未観測Calendarから期限を作らない。

### T06 — snapshot取得とFrame組立て（WD-02/03）

- 依存: T03/T04/T05。変更: `R/memory/personal_state/world/runtime_frame.rs`、新規同directory `runtime_sources.rs`、`R/runtime/context/world/inputs.rs`。
- 作業: §2.3のsnapshot順序と1回retryを実装。Situation依存は注入可能なread interfaceにしてcoreへAppStateを持ち込まない。graph/runtime/sourceを同一予算で組立てる。
- 合格 `wr_t06_`: DB一read transaction、Situation中途変更、二回変化、user-only frame、五要素graph併存、必須予算不足、各groupの省略理由。取得中にDBへ状態コピーを書かない。

### T07 — stamp再検証と受理（WD-05/11）

- 依存: T06。変更: `R/memory/personal_state/world/runtime_frame.rs`、`R/runtime/context/world/live.rs`、`R/runtime/context/generation.rs`。
- 作業: 元TTL、Scope、policy、source版/集合digest、Situationを再検証。DB正本検査とCASを同transactionに置き、Situationの観測sequenceは別と明記。終了時TTLのみとsource変更を区別する。
- 合格 `wr_t07_`: 999/1000ms境界、時計逆行、revision据置変更、取消/forget前後、完了時TTLのみ、完了前source変更、failed/cancelled receiptを残せる。

### T08 — 抽出候補schemaとpure validator（WD-04）

- 依存: T01/T02。変更: 新規 `C/world/extraction.rs`、新規 `R/memory/personal_state/world/extraction_validation.rs`、既存 `world/validation_v2.rs` の接続点。
- 作業: §2.4をstrict schemaとhost検証へ分ける。モデルの候補を既存Worldのpatch型へ変換するpure関数を作り、DB書込みはまだ行わない。
- 合格 `wr_t08_`: 五要素の許可候補、因果推定ラベル、存在しないsource、偽引用、UTF-8途中、Scope偽装、Task完了/権限payload、過大出力、未知field拒否。

### T09 — 抽出scheduler・commit接続（WD-04）

- 依存: T08。変更: 新規 `R/memory/personal_state/world/extraction.rs`、`R/memory/personal_state/scheduler.rs`、`product_extract.rs` の既存入口、既存storeのcommit接続点。
- 作業: 保存済みuser messageから専用purposeを起動。既存設定・同意・local/cloud送信制約に従う。上限1抽出/source version、失敗は保存済み会話を失敗にしない。commit時の再認可と冪等性を実装。
- 合格 `wr_t09_`: userから候補→既存ledger→Frameまで到達、assistant/Tool除外、重複実行、抽出中の編集/forget、キャンセル、不正JSON、Memory offで抽出なし。モデルにはfixture応答を使う。

### T10 — 訂正・forgetの抽出回帰（WD-04）

- 依存: T09。変更: 抽出validator/commit接続の必要な修正と新規 `extraction_tests.rs`。
- 作業: 対象が明確な訂正だけ既存遷移へ接続。曖昧訂正はpendingとする。原文削除後の再起動・再抽出で旧候補が復活しないことを保証。
- 合格 `wr_t10_`: 明示訂正、二候補曖昧、同文別source、source version更新、replay、forget後rebuild、異なるScopeの同名ノードを変更しない。

### T11 — 共通dispatch準備（WD-05）

- 依存: T07。変更: 新規 `R/runtime/context/world/dispatch.rs`、`world/turn.rs`、`R/runtime/conversation_turn.rs`、`world/render.rs`。
- 作業: snapshot/compose/render/manifestを一つのprepared objectで渡す。§2.3のretry回数と必須/任意の失敗分岐をここだけに実装。current instructionは常に1件、Worldはinstruction authorityなし。
- 合格 `wr_t11_`: 接続3秒後の新Frame、変化1回/2回、任意省略のbodyとreceipt一致、必須状態送信拒否、provider wrapperを含む予算、似た文字列を含む通常履歴を消さない。

### T12 — OpenAI互換のTool継続・fallback（WD-06）

- 依存: T11。変更: `R/providers/chat_completions/world_body.rs`、同directoryのloop送信点、対応するgeneration作成点（T00で実pathを記録）。
- 作業: 各実リクエストに新prepared objectを要求。Tool結果後は現sourceを取得して新generationへ渡す。旧Frameを履歴から除く際はtyped provenanceを使い、Tool call/resultの対応を保持。
- 合格 `wr_t12_`: 初回→5秒Tool→Task変更→新Frame、source忘却、二重Tool実行なし、primary失敗後fallback新Frame、本文/manifest/digest一致。旧follow-up World-freeテストは「旧Frameなし・新Frameあり」に更新する。

### T13 — DynamicLan・共有LARM（WD-07）

- 依存: T12。変更: `R/providers/stream/dynamic_lan.rs`、`larm_voice.rs`、必要な共通dispatch callback型。
- 作業: allocation/lease取得後にT11を呼ぶ。呼出元で先に生成したFrameを渡さない。ASR確定後の現在Scopeを利用し、音声sessionの継続とFrame寿命を分離。
- 合格 `wr_t13_`: allocation/lease fixtureを3秒待たせた実HTTP、待ち中Scope変更、lease失敗cleanup、同snapshotのtext/voice事実集合一致。共通HTTP試験だけでこのカードを完了にしない。

### T14 — AgentSessionの履歴隔離（WD-08）

- 依存: T11。変更: `R/providers/agent_session/sse.rs`、`sse/generation.rs`、session作成を所有する既存module。
- 作業: 安全な入力置換が契約で証明されない限り、判断generationごとにremote sessionを新規化する。Tool継続にはhostで保持する許可済み会話・Tool対応・最新Frameを再構成。remote hidden historyへ依存しない。
- 合格 `wr_t14_`: fixtureが旧session本文を保持する設定で、訂正/forget/Scope切替後は旧sessionが使われない。Tool結果とcall IDが保たれる。serverが再構成継続非対応なら能力unsupportedを返し、この機能は未完了として残す。

### T15 — Codexの共通Frame投入（WD-09）

- 依存: T11。変更: `R/runtime/codex_context.rs`、`codex_turn.rs`、`codex_process.rs`、既存Codex fixture tests。
- 作業: 独自metadata snapshotを共通Frameのtyped renderへ置換。既存fresh thread・actual wire digest・dispatch/finish検査を保持。role-routingなど別の有効Codex判断経路もT00の一覧に従って接続し、必要ならT15bへ分割。
- 合格 `wr_t15_`: 五要素graph＋Situation/Task/期限が実thread/startへ届く、World二重投入なし、turn instruction1件、Scope変更時拒否、Tool/権限境界維持。Coding実行session自体の再開を無条件に廃止しない。

### T16 — MCPのFrame v2対応（WD-10差分のみ）

- 依存: T11。変更: `crates/reasoning-contract/src/{lib,schema,world}.rs`、`R/runtime/conversation_controller/payload.rs`、必要なservice側validator。
- 作業: project_scope必須のv2契約では新Frameを表せないため `reasoning-answer-v3` / `world-evidence-v2` へ同時更新。schema比較・budget・body digest・authority noneは維持。旧schemaへのsilent fallbackを追加しない。
- 合格 `wr_t16_`: user Scope、全source payload、graph、旧v2 server拒否、metadata改竄、byte境界、初期化待ち失効。service/client fixtureと配備依存を更新。

### T17 — 状態claimの型とvalidator（WD-11）

- 依存: T07。変更: `R/runtime/context/state_answer.rs`、新規 `R/runtime/context/world/state_claim.rs`、新規core側pure claim型（必要時）。
- 作業: §2.5の検証関数とhost表示を作る。kindごとに比較列を固定し、自由文の真偽判定をLLMへ丸投げしない。Task状態、Goal状態、meeting推定、期限を区別する。
- 合格 `wr_t17_`: 正常/偽state/偽source/古いversion/別Scope/unknown、settledを成功とするclaim拒否、元時点だけの有効回答。

### T18 — claim受理・表示・再取得（WD-11）

- 依存: T12〜T17。変更: `R/runtime/conversation_turn.rs`、controllerのresponse受理点、Provider共通response型。5ファイル超過はadapter別枝番に分ける。
- 作業: 状態回答モードを既存分類へ接続し、structured response→validator→host render→保存/配信とする。source変更時は再生成1回、再変化はhostカード。各Providerの対応/縮退を明示。
- 合格 `wr_t18_`: 生成中Task終了、withdraw、Scope A→B、偽claimがstream/UI/TTSに出ない、複合依頼のTool処理維持、一般会話のstream維持。全正常例をhost縮退にして合格にしない。

### T19 — 通常音声の最終hold（WD-11）

- 依存: T03/T18。変更: `R/situation/speech.rs`、ack以外を含むTTS合成入口・audio queue入口（T00で実pathを記録）。
- 作業: 合成開始前とqueue直前で同じSituation契約を確認。合成中にholdへ変化した音声をqueueへ入れない。現在状態回答はT18の検証済み本文だけを使用。
- 合格 `wr_t19_`: ASR後/合成中/queue直前のMEETING hold、unknown、hold解除、cancel後に音声が復活しない。既存ack試験だけでは完了にしない。

### T20 — Scope選択・結果の所属表示（WD-01）

- 依存: T02/T18。変更: `src/features/chat/useConversationTurn.ts`、対象表示用新規component、`src/lib/runtime.ts`、既存Scope取得API。
- 作業: 現在対象と切替、複数候補の選択を表示。登録済み対象だけ選べる。途中でScopeを替えた旧結果は元対象を表示し、現在対象の結果へ変更しない。
- 合格 `wr_t20_`: Projectなし、同名二Project、二Task、A→B切替後A結果、対象削除、backendが不正scopeRefsを拒否。UI操作のcomponent試験を追加。

### T21 — 能力・省略理由の表示（WD-12）

- 依存: T12〜T16/T20。変更: 新規 `R/runtime/context/world/capabilities.rs`、既存IPC契約、settingsのProvider表示component、Chatのsource表示component。
- 作業: backend由来の `state input / graph / fresh tool continuation / verified state answer` 能力を区別。configured capabilityと直近実送信・省略理由を分ける。UIでProvider名から能力を推測しない。
- 合格 `wr_t21_`: Provider切替、対応不能、World省略、host縮退を正しく表示。diagnosticsに本文・引用・秘密が入らない。IPC生成物とreceiver fixtureが一致。

### T22 — offline残件runner（WD-13実装側）

- 依存: T09〜T21。変更: `scripts/world-eval.ts`、Provider別World fixture tests、`spec/evidence/world-delivery/remaining-report.json`（新規生成物）。
- 作業: 既存33ケースを保持・必要な意味変更を明記し、wr_t01〜wr_t21と6経路×7遷移のmatrixを収集。機能のないToolだけ能力根拠付きN/Aにできる。live未検証をpassへ換算しない。
- 合格 `wr_t22_`: テスト0件・case欠落・重複・古いreport・子プロセス失敗でrunner失敗。current/stale/denied/unstableとreceipt一致を実送信fixtureでassert。開始時に前回成功reportを無効化。

### T23 — 性能と担当コードの品質（WD-13実装側）

- 依存: T22。変更: 既存Frame性能fixture、担当moduleの必要な分割・最適化。
- 作業: 同一build profile・端末・DB・warmup/sample条件で旧baselineと比較。100投影/2,000ledgerと上限sourceを使用。freshness検査を外して高速化しない。担当ファイルのsize/Clippyを解消。
- 合格: 共通Frame/serialize追加p95 30ms以内を同一測定範囲で判定し、件数・raw sample・build条件を保存。未達なら未完了。実Provider TTFAはlive側に残し、fixture時間で代用しない。

### T24 — 実装残件の締めとlive引継ぎ（WD-13）

- 依存: T00〜T23。変更: 本書、親計画§0、`spec/evidence/world-delivery/remaining-{progress,results}.md`（新規）、既存route-matrix。
- 作業: 各カードの変更path、合格test、未検証を記録。§4の必須コマンドを実行。v3 MCPの同時配備手順・旧版拒否・戻す場合のclient/service一組でのrollbackを記録。
- 合格: sourceが投入されるだけでなく、自然文→既存ledger→Frame→各wire→検証済み回答/UIまでfixtureで接続。全体ゲート失敗とlive依存が残れば「実装残件完了／製品受入未完了」と区別する。未達カードをdoneにしない。

## 4. 試験の実行規則

カードT01〜T21のRust試験prefixは `wr_tNN_`。core/contract/serviceに追加した試験はそれぞれのmanifestで実行する。以下の `wr_` がdesktop以外の試験まで実行したとは扱わない。各filterの実行件数を確認し、0件は不合格。

```sh
cargo test --manifest-path src-tauri/Cargo.toml --lib wr_ -- --test-threads=4
cargo test --manifest-path src-tauri/Cargo.toml --lib runtime::context -- --test-threads=4
cargo test --manifest-path src-tauri/Cargo.toml --lib memory::personal_state::world -- --test-threads=4
bun run world:eval
bun run test:rust-packages
bun run ipc:check
bun run test:frontend
bun run typecheck
bun run size:check
cargo fmt --check --manifest-path src-tauri/Cargo.toml
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
bun run check:local
bun run spec:check
```

各カードではそのprefixと影響先の既存試験に絞る。T22〜T24で全体を実行する。strict deadlineを検証する試験とprotocol内容を検証する試験を分け、時間延長で本来のtimeout検査を消さない。テストの都合でlive TTL・ACL・body上限・size閾値を緩めない。

reportには `schema_version / code_revision / dirty_diff_digest / generated_at / case_id / route / source_kinds / frame_digest / wire_digest / expected / actual / pass / verification_level / omission_reason` を残す。本文・token・個人情報は保存しない。実測とfixture、正常回答とhost縮退を別集計にする。

## 5. この計画の外に残す受入作業

親計画WD-13の6経路それぞれ正常・不明・訂正・Scope切替・期限切れ・生成中更新を各5例、計30例を実モデルで行う。モデル名・契約版・接続先識別子・実行日を記録し、state claim誤り0、source mismatch 0。正常5例が全てhost縮退した経路は未達。

実UIのScope切替、実音声のASR→現在Scope→TTS hold、TTFA p95悪化10%以内を測定する。稼働接続先・認証が必要な時点で不足情報だけを求める。以前の接続失敗を現在の障害と決めつけず再確認するが、接続復旧のために送信許可を広げない。

プロジェクト全体の既存size/Clippy修正は、World担当外の大規模改修なら別の作業範囲として記録する。担当外だから製品全体が完成したとは報告しない。

## 6. Terraへの開始指示

> この計画のT00から順に進めてください。最初の到達点はT07までの「正本に基づく共通Frameと失効検査」です。各カードの実装・指定試験・証跡更新を終えてから次へ進み、T24まで継続してください。既存実装は再利用し、未接続sourceを架空データで埋めたり、旧Frame除去だけで最新状態投入を完了にしたりしないでください。実接続不能はlive受入だけの依存として記録し、独立した実装・offline検証を続けてください。成果物には実装残件完了と製品受入完了のどちらに到達したかを明記してください。
