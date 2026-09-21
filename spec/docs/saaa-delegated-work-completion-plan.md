# 自然な依頼から実行・報告まで続く委任仕事 実装計画

作成日: 2026-09-21。状態: 独立監査後の残件修正・再受入待ち（2026-09-21 更新）。担当想定: Terra。

## 0. 現在の実装状況

残件の実施順・対象ファイル・回帰試験・完了条件は、後続の
[委任仕事の残件修正・統合受入計画（Terra向け）](saaa-delegated-work-repair-terra-plan.md)
を正本とする。本書の実行契約・最終受入条件は維持する。
独立監査は `spec/evidence/delegated-work/independent-audit-2026-09-21.md`。
既存の `progress.md` / `results.md` は部品試験の成功履歴として保持するが、
そこにある complete をカード全条件の達成として引き継がない。

| 状態 | カード | 実装済み／残作業 |
| --- | --- | --- |
| 完了済み部分を保持 | DW-00 | baselineと既存部品の成功履歴。後続DWR-00で現状を再採取する。 |
| 部分完了・再受入待ち | DW-01〜06、DW-08、DW-09、DW-11、DW-13 | source/権限、自然文plan、複数Goal再openと撤回、busy復帰、receipt/binding、report transaction、wake/Chat更新、forget/revokeを修正する。origin分離・single slot・既存cursor等は再利用する。 |
| 部分完了 | DW-07 | SDKの実readとwrite/network/scope拒否の証跡は保持。read/test別のtool境界、登録recipe、時間予算、再実行可能な拒否試験と実test受入が残る。 |
| 部分完了 | DW-10 | plan/dependency/replan/artifact参照の部品を保持。自然文経路のplan作成、結果根拠によるverifier、Goal全体の完了を接続する。 |
| 部分完了 | DW-12 | 実TTSのplayback_finished/delivery_unknownと再生抑止の証跡は保持。全入口の設定OFF/ON、配送・再接続・forgetとの組合せを再受入する。 |
| 未完了 | DW-14 | desktop smokeと個別fixtureは成功。実装修正と、実モデル・実profile・実UI・sleep/restartを含む最終統合受入の両方が残る。 |

既存の `bun run desktop:smoke` 成功は build、bundle、launch、IPC ready、cleanup の証明であり、業務操作の証明ではない。全体gateの対象外失敗と本テーマの未達を分け、steward自身のsize超過等は後続計画で解消する。

## 1. 解消する弱点と完成状態

目的は「決まった言葉で始め、次に話しかけるまで結果が届かない限定ループ」を、明示委任の範囲で続く仕事へ完成させること。ユーザーが自然文で調査・テスト確認を任せ、別の話題へ移っても実行が継続し、完了や判断待ちが適切なタイミングで一度届けられる。撤回後は新規操作しない。再起動で実施済み操作を繰り返さない。

上位は [Personal AI Concept](saaa-personal-ai-concept.md) §3・§4・§9・§15。[最小執事循環](saaa-minimal-loop-plan.md)と[Butler Schedule](saaa-butler-schedule-ledger-plan.md)の後続。本書は固定トリガ・一件制限・次turn依存を明示的に更新する計画であり、旧計画の「このStepでは見送る」を最終制約として引き継がない。

対象は既存のCoding/Pi実行先と登録済みCapabilityによる調査・検証仕事。任意アプリ操作や無制限の自動修正を追加せず、能力や委任が足りない仕事は具体的な判断待ちへ移す。未知の仕事を何でも完遂することを完了条件にしない。

## 2. 依存・所有・現状

[重要Context計画](saaa-required-context-completion-plan.md)のRC-C1〜C4を先に確定する。DW-00〜04の台帳・純粋reducerは先行可能だが、通常generationとTool実行の統合受入はRC完了後。[World供給](saaa-world-delivery-completion-plan.md)へTaskの正本参照を渡し、[学習計画](saaa-adaptive-improvement-completion-plan.md)へ判断と検証結果を渡す。World/Memoryを実行正本にしない。

| 現行接続点 | 調査時点の差 |
| --- | --- |
| `steward/reduce.rs` | 固定文言、ユーザーturnでstart/inspect/publish |
| `steward/schema.rs` / `repository.rs` | 会話内active Goal一件、固定request、成功条件は文字列 |
| `steward/report.rs` / `runtime/start_turn.rs` | 終端写像と保留報告の解放が次turnに依存 |
| `coding/service.rs` / `service_transactions.rs` | 既存user message/runを前提に受付。非会話起点には契約の拡張が必要 |
| `runtime/pi/runner.rs` / `coding/recovery.rs` | 実行状態・終端・process recoveryの正本として再利用 |
| `schedule/tick.rs` / `decide.rs` / `handle.rs` | 開発中。調査時のActは通知とID記録まで。委任文字列の存在だけでは実行許可にならない |
| `situation/tick.rs` / `speech.rs` | 状態変化とhold条件。通知解放の契機へ接続する |

pathは `src-tauri/src/` 相対。フロントは `src/features/coding/StewardPanel.tsx`、Chat履歴、Schedule設定。着手時の差分に実装があれば再利用し、schedule workerを二つ起動しない。

## 3. 実行契約

### DW-C1 Goal・委任・計画を分ける

既存steward表を加算migrationで拡張し、型付き `GoalSpec` / `DelegationSpec` / `TaskPlan` を新規予定 `steward/contracts.rs` に置く。

- Goal: ユーザー起点のsource、Scope、望む結果、成功条件とverifier、status/revision。
- 委任: Goal参照、対象resource、許可operation、送信先、金額/時間/回数上限、有効期限、通知条件、status/revision。
- 計画: Goal/委任版、有限step列、依存関係、能力参照、各stepの成功条件、次の判断、最大再計画回数。
- 実行: `coding_jobs` / `coding_runs` または既存tool invocationを参照。終了状態をstewardが独自に決め直さない。

初期実装上限は同時active Goal 8、Goalあたりstep 16、同時Coding実行1、再計画2回。上限は設定契約で検証し、超過時は明示的に受付を断る。単一Goal限定は撤廃するが、実行枠は増やさない。現在話題と背景Goalを別管理する。

成功条件は、例えば `test_report_obtained`（指定test runnerの結果と失敗一覧を取得）と `tests_pass`（指定対象が成功）を区別する。テストが失敗した調査は前者を満たし得るが後者を満たさない。自由文だけで検証不能なら `user_confirmation_required` を採用し、自動完了を宣言しない。

### DW-C2 自然文からの受付

共通Tool経路へ `work_propose` / `work_status` / `work_amend` / `work_withdraw` を追加する。モデルは `GoalProposal` とユーザーsourceの引用範囲を出すだけ。hostがsourceのrole、Scope、最新revision、引用と明示意図を検査し、既存委任の範囲内ならそのまま受付する。

モデルによる意味解釈だけで新しい外部操作権限を増やさない。既存委任を超える依頼は、対象・操作・予算・完了条件を揃えたUIで一度確認する。明確なユーザー操作による委任登録はそのまま権限正本とする。曖昧な「それをお願い」は候補を絞り確認し、引用、仮定、Tool本文からGoalを採用しない。

旧固定トリガは互換入力として同じ受付APIへ変換する。新しいユーザーメッセージを捏造せず、sourceの実IDを維持する。音声で受付する場合も同じ契約を使用し、未対応Providerにはhostの実行能力を正しく伝える。

### DW-C3 会話外起点と冪等な実行

`coding/service` の受付を共通domain serviceへ抽出する。起点は `UserTurn {message, run}` と `DelegatedEvent {goal, delegation_revision, event_id}` の型付きunion。後者は保存済みの明示委任とGoal sourceへ必ず辿れる。`StartTurnInput`やsource messageの偽造で既存APIを通さない。

既存 `coding_jobs.source_id UNIQUE` の意味を保つため、旧rowはUserTurn起点として移行する。非会話起点は別のorigin binding表または加算列で識別し、`(origin_kind, origin_id, operation_digest)` を一意にする。依存するcall cache、source forget、world snapshot、recoveryも同時更新する。

writer transaction内で委任の現行版、撤回、resource、権限、budget、dedupe、実行枠を再検査し、Taskとdispatch intent、予算予約を同時commit。外部I/Oはtransaction外。予約は `reserved / consumed / released` を区別し、後続requestが同じ予算を二重使用しない。

dispatch intentは `pending → dispatching → accepted | failed | outcome_unknown`。受領確認前のクラッシュは結果照会へ進む。相手が冪等keyを受け付けない場合、再送でexactly-onceを装わず `outcome_unknown` で止める。再起動時のprocess所有確認は既存recoveryだけが担当する。

### DW-C4 実行権限はプロンプトではなく境界で強制する

「修正しないで」というrequest本文や禁止語検出を権限境界にしない。read委任はrunnerにread-only tool集合とfilesystem権限を渡す。test実行は任意shellを許可せず、ユーザーが登録したtest recipe、作業copy/許可出力先、network/timeout/resource制限をhostが適用する。

既存Pi/Codex実行profileが必要な制限を強制できなければ、そのprofileでは当該委任をunsupportedにする。DW-07で対応profile/adapterを完成させ、少なくとも一つの実際のread/test実行先が制限下で動くまで本計画は未完了。ファイル変更後の差分確認だけを事前制限の代替にしない。

dispatch許可の線形化点は委任版検査とintent CASのcommit。先行撤回は新規dispatch 0。後行撤回はcancelを要求し、既に実行済みの効果は取り消せたと主張しない。遅延結果は監査と状態照会の証拠として保持できるが、新しいstepや成功扱いの根拠にはしない。

### DW-C5 イベント駆動で仕事を継続する

新規予定 `steward/events.rs` / `driver.rs` は単一writer上の永続イベントcursorを消費する。Task終端、期限到来、依存解決、委任変更、Situationのhold解除、再起動・復帰を入力にする。in-memory wakeupは高速化のみで、再起動時はDBから未処理分を回復する。

pure reducer → transitionとoutboxをcommit → effect実行の順。turnごとのinspectは補助に降格し、継続の唯一の契機にしない。新規daemon・別DB・LLMへの常時問いかけを作らない。期限は既存scheduleの45秒tickを使い、Task結果はcommit後に即時wakeする。

状態は `queued / dispatching / running / awaiting_dependency / awaiting_user / verifying / done / failed / cancelled / outcome_unknown` を明示。`done`は実行の終端かつGoal verifierの成功。失敗後の次stepは採用済み計画と委任内に限定し、回数/時間超過は判断待ちまたは失敗へ。

### DW-C6 報告を一度、適切な時点で届ける

既存steward_reportsをdelivery outboxへ拡張する。`report_id + task_revision + destination`で一意化し、会話メッセージの挿入とdelivery状態更新を同一transactionにする。commit後のUIイベントは再配送可能にし、UIはmessage IDで重複排除する。

`silent`は会話内表示だけ、`both/speak`でもSituationのTTS holdを必ず通す。foreground会話中に別Taskの結果を現在回答へ混ぜない。会議中の保留はhold解除イベントでまとめ、次の発話を待たない。mute、離席、通知設定はpolicyとして扱い、解除後に最新状態の要点だけを一通で届ける。

音声出力はDBと原子的に確定できないためexactly-onceを保証しない。`playback_started / finished / delivery_unknown` を区別し、再起動時に不明な音声を自動再生しない。会話内の永続報告は残す。OS通知はユーザーが有効にした場合だけ補助経路とする。

## 4. Terra向け作業カード

番号順。各カードは最大5実装ファイルを目安に枝番分割する。schema、IPC、生成型の所有者をprogressへ記録する。

| ID | 対象と実装 | 合格条件 |
| --- | --- | --- |
| DW-00 | 現行steward/schedule/Pi、dirty、schema、各権限境界のbaseline | 既存worker・新規予定・外部profileの不足を一覧化 |
| DW-01 | Goal/Delegation/Plan/Origin/Verifier型と純粋validator | 不明operation、曖昧完了、循環step、上限超過を拒否 |
| DW-02 | steward表の加算移行、複数Goal、origin binding、予算予約 | 既存Goal/jobを保持。重複起点と同時予約で二重Task 0 |
| DW-03 | 共通Toolの提案/照会/変更/撤回とユーザーsource binding | 自然文と旧トリガが同じ受付へ。引用からの委任0 |
| DW-04 | UIで対象/操作/予算/成功条件を登録・確認・変更 | 同一許可を繰り返し聞かない。二つのGoalを区別して停止できる |
| DW-05 | coding共通domain受付を抽出、UserTurnとDelegatedEventを接続 | message偽造0、既存tool受付の互換維持 |
| DW-06 | dispatch intent、CAS、budget reservation、recovery | commit前後/外部受領前後のcrashで再実行の捏造0 |
| DW-07 | runner profileのread/test権限制限とresource制御 | 許可外write/network/shellが実行境界で拒否。実profile一つ以上で成立 |
| DW-08 | 既存coding終端commitから永続event、driverとcursor | user turnなしで終端を処理。再起動後に取りこぼし0 |
| DW-09 | schedule Actを共通受付へ、委任実体/期限/権限を再検査 | receiptなしでStarted 0。架空・撤回済みdelegationはAct 0 |
| DW-10 | 依存step、verifier、有限再計画とTask成果物参照 | settledだけでGoal達成0。失敗/未知/検証不能を区別 |
| DW-11 | report outbox、Situation hold解除、Chatイベント | 次turnなしで報告。重複eventで会話報告1通 |
| DW-12 | TTS/通知配送状態、UI再接続、設定OFF/ON | playback不明時の二重発話0、無効時の新規仕事0 |
| DW-13 | 撤回・forget・Scope revokeをTask/dispatch/reportへ連動 | 遅延結果で次step開始0、削除本文の通知復活0 |
| DW-14 | 複数Goal、話題切替、実行競合、sleep/restartの統合受入 | §5の全条件。現行UIから操作できる |

DW-07でSDKやrunnerの変更が必要なら、既存拡張点に権限制御を実装する。第二のCoding Agentを新設しない。外部依存が不足する場合はprofile契約と残作業を明示し、後続のfake試験は進めても完成扱いしない。

## 5. 完全克服の受入条件

| シナリオ | 合格 |
| --- | --- |
| 「このプロジェクトの失敗テストを調べて、終わったら教えて」→別話題 | 明示委任内で実ジョブ開始、検証結果と報告をuser turnなしで保存・表示 |
| AとBを登録、一つの実行枠、Aだけ撤回 | Bが消えず、公平なqueue順で進む。Aの新規step 0 |
| 期限到来→会話中/会議中→解除 | 重い推論は会話を優先。通知は設定通り保留・集約し、解除後に届く |
| クラッシュをintent前/後・送信前/後・結果commit前/後へ注入 | duplicate effect 0、未知結果は照会へ。メモリ上のaction配列を成功証拠にしない |
| test終了code失敗、調査結果あり | 調査完了とtests passを混同しない。verifier出典を表示 |
| 否定・引用・曖昧対象・権限不足 | 自動採用せず、対象を絞った確認または対応不能の説明 |
| Task実行中の撤回・forgetと遅延結果 | 停止要求、後続禁止、削除済み内容の再表示0 |

受入データは20個以上の自然な依頼表現と10個以上の曖昧/否定/引用ケースを固定する。実モデル、実UI、破棄可能なGit workspaceで最低一つの実行profileを通す。通常完了・失敗・撤回・再起動・hold解除をそれぞれ実演し、fixtureだけの成功を全体完了にしない。

設計上の時間目標: awake状態でTask終端commitからdriver処理p95 2秒以内、hold解除確定から会話内報告p95 2秒以内、期限処理は既存tickの1周期+5秒以内。sleep中の処理は保証せず復帰時に照合する。Foreground優先と権限は時間目標より優先。

検査: 新規prefix `dw_` のRust対象試験（0件不可）、coding/recovery/steward/scheduleの既存試験、UI導線試験、`bun run ipc:generate` / `bun run ipc:check`、`bun run size:check`、`bun run check:local`、`bun run spec:check`。schema再open・移行は一時実DBで確認する。

## 6. 成果物と開始指示

`spec/evidence/delegated-work/{baseline,progress,results}.md` にカード、起点・委任・intent・receipt・verifier・reportの追跡表を残す。個人の原文、絶対workspace path、認証情報を証跡へ出さない。実装済み/自動受入/実運用導線/未完了を分ける。

初期は既存設定に従いOFFでもよいが、UIから有効化して上記シナリオを再現するまで完成ではない。既存権限内の継続をカードごとのユーザー承認待ちにしない。

Terraへの開始指示例:

> DW-00から順に実装してください。RCの必須Context契約を利用し、自然文受付、非会話起点の正当な実行、実ジョブの終端、会議後の自動報告まで閉じてください。開始通知やaction ID保存だけを実行完了にせず、実際の権限制御とverifierを受入に含めてください。
