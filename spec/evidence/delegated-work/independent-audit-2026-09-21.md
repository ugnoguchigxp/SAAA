# 委任仕事の独立監査（2026-09-21）

結論: 残件は実 UI 操作だけではない。自然文受付、複数 Goal の再起動・競合・撤回、報告の原子性と表示などに、UI がなくても確認できる実装上の欠落がある。DW-14 の最終 UI 受入より先に、これらを改善計画へ戻す必要がある。

対象は現行 working tree と実装計画・progress・results・対象試験。監査中にも他作業による変更があり、終了時 HEAD は `e8c79608b138324e51839ef15bff5914f4146771`。本監査では実装・既存計画・完了ステータスを変更していない。以下の行番号は監査時点。実 SDK/TTS の過去の成功記録は尊重するが、今回の再実行結果と区別する。

## 1. 完了判定を戻すべき具体的な問題

### A01 / P1: 複数 active Goal を保存すると再起動の schema 初期化が失敗する

- 根拠: `src-tauri/src/steward/schema.rs:53` は毎回 `steward_one_active_goal` UNIQUE index を作成し、同ファイル64行で削除する。初回の削除後に同じ会話へ A/B を作ると、次回は削除へ進む前の再作成で失敗する。`src-tauri/src/persistence/schema.rs:173` は起動時にこの migration を呼ぶ。
- 再現: 実ソースから最初の `execute_batch` の SQL を抽出。一時ファイル DB で初回作成→旧 index 削除→同一会話に active Goal を2件保存→close/reopen→同じ SQL を実行した結果、`UNIQUE constraint failed: steward_goals.conversation_id`。実アプリ再起動ではなく、実 migration SQL のファイル DB 再現である。
- 既存試験の穴: `dw_10_migration_backfills_a_plan_for_an_existing_goal` は1件。`dw_01_multiple_active_goals_are_allowed_and_no_delete` は複数件で再openしない。`ml_07_reopen_maps_outcome_unknown_without_rerun` も複数 Goal との組合せを保証しない。
- 対象カード: DW-02 / DW-14。必要な確認: active 2件・8件、完了済み Task を持つ Goal、旧DBからの移行それぞれで実初期化関数を通す再open。

### A02 / P1: 自然文の work_propose 経路で Goal plan が作成されない

- 根拠: `steward/repository.rs:101` の `persist_default_goal_plan` は直接登録でのみ呼ばれる。`propose`（173行）は Goal・Delegation・origin binding を挿入して終了する。`steward/tools.rs:56` 以降はその直後に `queue_task` を呼ぶが、`repository.rs:547` の `next_ready_plan_step` は plan がなければ `None` を返し、Task は0件となる。
- 影響: 自然文 tool は受付成功・`requiresConfirmation:false` を返しても、新しい仕事が開始されない。再起動 migration の backfill は同じ受付内の欠落を補わない。
- 既存試験の穴: `dw_03_proposal_binds_only_a_persisted_user_source_and_allows_multiple_goals` は Goal 件数と source 偽造拒否まで。DW-10 の依存試験は `register_goal` を使用し、自然文経路を通らない。
- 対象カード: DW-03 / DW-10。必要な確認: 自然文 tool→plan→step→reservation→intent→実 service receipt を一連で確認する。

### A03 / P1: busy で一度断られた Task が自動再開できない

- 根拠: `steward/reduce.rs:165` 付近のエラー経路は Task を `queued` に戻し、`settle_dispatch(..., true)` で intent を `failed` にする。`repository.rs:637` の claim は `pending` のみ許可する。`next_queued_work`（864行）は最古の queued Task を選び、intent 状態を条件に含めない。
- 影響: B が A と競合すると、空き枠が戻っても B の claim は通らない。同じ B が先頭に残り、後続も進めなくなる。設定 OFF についても、claim の後に coding_enabled を確認して return するため `dispatching` が残る経路がある。
- 既存試験の穴: `delegated_tasks_compete_for_one_production_coding_slot`（`coding/integration/tests.rs:181`）は `service::execute_delegated` を直接呼び、A 停止後にテスト自身が B を再呼び出しする。Steward の claim/失敗/retry を経由しない。
- 対象カード: DW-06 / DW-14。必要な確認: 実際の Steward dispatcher で busy→枠解放→自動開始、公平な後続進行、OFF→ON を検証する。

### A04 / P1: 状態同期が他 Goal の撤回を波及させ、撤回済み Task を上書きし得る

- 根拠: `steward/repository.rs:1022` の `sync_from_coding` は `latest_work` の撤回状態を一度だけ読み、会話内の全 Task に適用する。最新 B を撤回すると、active A の open Task まで cancelled になる。逆に最新 B が active のまま A を撤回すると、A の既存 Coding job 状態で cancelled を running/done 等へ上書きし得る。
- 経路: `list_steward_tasks` は一覧取得のたびにこの関数を呼ぶ。`apply_terminal_event` の撤回ガードだけでは防げない。
- 既存試験の穴: `dw_14_multiple_goals_keep_the_sibling_through_topic_switch_withdrawal_and_hold` は A を撤回し、B を残す片方向。A の実行中 job を伴う同期と、最新 B 撤回の逆順を検証しない。`dw_13_late_terminal_event_cannot_revive_cancelled_task` は別の終端適用関数だけを試験する。
- 対象カード: DW-02 / DW-13 / DW-14。必要な確認: A/B 両方向の撤回、遅延結果、その後の list/status/poll を含める。

### A05 / P1: source が user であることと、新しい委任権限を承認したことが区別されていない

- 根拠: `steward/repository.rs:183` 以降は source の存在・role・会話と workspace 登録等を確認するが、本文の引用範囲、否定、明示意図、既存委任への包含を検査しない。モデル指定の operations/budget を新規 active Delegation に保存し、常に `requiresConfirmation:false` を返す。`GoalProposal` に引用範囲や確認済み authority 参照もない。
- 影響: user 発言に引用・否定が含まれても、不適切な tool call が出れば host は権限レコードを作れる。現状は A02 が実行を止めるが、plan 作成だけ直すとこの問題が実行に届く。
- 既存試験の穴: `dw_01_proposal_rejects_ambiguous_or_unbounded_authority` は実際には空 operations と17回の上限違反だけ。`tests/delegated-work-cases.test.ts` は Markdown の20/10件と route ラベルを検査するだけで、文例をモデル・host に通していない。
- 対象カード: DW-01 / DW-03 / DW-04。必要な確認: 明示権限内の受付と新権限の確認を分離し、引用・否定・曖昧文に対する不正 tool call を host で拒否する試験。

### A06 / P1: report outbox と会話メッセージが同一 transaction で確定されない

- 根拠: `persistence/sqlite/writer.rs:77` の `write` は Mutex による直列化で、transaction を開始しない。`steward/report.rs:85` の通常配送 caller も transaction を作らない。`flush_unflushed`（192行）は message INSERT、conversation 更新、観測記録、`mark_flushed` を個別実行する。
- 影響: message INSERT 後・mark_flushed 前のクラッシュで、再起動後に同じ報告を別 message ID で再挿入し得る。逆に `claim_terminals` が `report_json` を更新した後、outbox 挿入前に落ちると、その Task が再claimされず報告が欠ける。Task・reservation・intent 作成も caller によって transaction がない。
- 既存試験の穴: `terminal_delivery_is_unique_per_task_revision_and_records_message_id` は outbox の重複挿入と通常 flush のみ。途中クラッシュを挿入しない。TTS の durable state テストはこの会話配送の原子性を証明しない。
- 対象カード: DW-02 / DW-06 / DW-11。必要な確認: 実書込み経路の各commit境界へ fault を入れ、欠落0・会話報告1通を再openで確認する。

### A07 / P1: 終端から配送・後続開始への自律 wake-up と Chat 更新が閉じていない

- 根拠: `runtime/pi/runner.rs:77` は終端と cursor を同一 transaction で更新するが、その後 report 配送を wake しない。`driver::consume` は outbox を作らない。`report::flush_all_held_reports`（103行）は既に未配送 report を持つ会話しか選ばないため、report がまだ0件の終端を拾えない。
- 現在の補助経路: `StewardPanel.tsx:58` の2秒 polling→`list_steward_tasks`→publish/flush/start。または新 user turn。schedule 側の flush は45秒 tick で、schedule OFF では実行されない。
- Chat 側: Steward panel は Task 状態だけを refresh する。report は UI event を emit せず、`useConversationTurn.ts:186` の `saaa:ui-history` 購読へ接続されていない。この event の発行元は generative UI 用 `features/chat/ui/api.ts:23`。DBへの保存と、開いている Chat の即時表示は別である。
- 既存試験の穴: combined test は `publish` と `flush_held_reports` を直接呼ぶ。UI unmount・会話切替・polling停止中の後続開始、Chat表示を確認しない。
- 対象カード: DW-08 / DW-11 / DW-14。必要な確認: UI が polling しない条件でも終端→outbox→配送→後続開始を進め、再接続で message ID 重複なく表示する。

### A08 / P1: verifier と報告内容が結果の根拠まで接続されていない

- 根拠: `repository.rs:659` の終端適用は `result_json`・終了code・test runner・結果出典を読まず、settled + test_report_obtained を done にする。tests_pass と user_confirmation_required は一律 awaiting_user。依存判定は step の verifier 判定結果ではなく done/awaiting_user を参照する。
- 報告は `claim_terminals`（1077行）の `task <id>: <state>` だけ。調査要約・失敗一覧・verifier 出典を含まず、awaiting_user は report 対象外。Task が done でも Goal は active のままで、schema も Goal の done/failed 状態を持たない。完了した仕事が active Goal 上限を占め続ける。
- 依頼対象: `reduce.rs:260` 付近の実行 request は固定の汎用文。Goal summary や選択された具体的 test target が runner request に反映されない。自然な対象指定を保持した完遂も未証明。
- 既存試験の穴: `dw_10_settled_read_step_durably_enqueues_one_dependent_test_step` は架空 job の settled 注入で成功。test結果なし、失敗codeあり、全成功、検証不能を実際の成果物と照合していない。
- 対象カード: DW-10 / DW-11。artifact reference 保存・有限 retry は部分として成立するが、Goal 達成判定の完成とは言えない。

### A09 / P1: read/test の許可差と時間予算が runner 境界へ渡らない

- 根拠: `scripts/pi-codex-sdk/index.ts:136` の read-only / no-network / never-approval 設定は実在する。ただし read と test_run は同じ実行設定で、登録済み test command の許可リストや操作別 tool 集合を渡していない。`reduce.rs` の read/test recipe は実行可能な制約ではなく英語 request。
- 旧 profile: `runtime/pi/process.rs:299` の delegated-read-test-macos-v1 は workspace 引数を使わず、`allow file-read*` と `allow process*`、一部 temp write を許す。SDK profile の workspace 外 read 拒否を、この別 profile に一般化できない。
- 時間: `repository.rs:919` は開始前に既存 run の経過を確認するだけ。`runtime/pi/runner.rs:150` は固定1800秒で、delegation の budget_ms を参照しない。例えば1秒/60秒の登録予算が現在の run をその時間で止める保証はない。
- 証跡の限界: 実 SDK の README read、write/network/scope 拒否は既存 results に記載。保存された `delegated_sdk_profile_completes_a_read_only_prompt` は README read と不変性のみで、拒否3種を同テストで再現しない。実 SDK の test recipe 実行は確認できない。
- 対象カード: DW-07。「read sandbox の実証」と「read/test/recipe/resource 制限の完成」を分ける必要がある。

### A10 / P1: 設定 OFF、forget、Scope revoke の全経路保証が不足する

- Memory OFF: `memory_enabled` の確認は `reduce::on_user_message` / foreground inspection にあるが、model tool、`start_queued_for_conversation`、schedule dispatch にはない。既存 queued Task は UI refresh 経路から進められる。`ml_03_memory_off_or_no_goal_is_noop` は固定トリガ入口だけを試験する。
- Coding OFF: 新規 process は service の enabled 確認で止まる。ただし A03 の claim 消費により、ONへ戻した際の継続が壊れる。
- forget: `repository.rs:371` は open Task の取消と予約解放を行うが、既存 held report、pending speech、Goal summary を失効させない。配送時の SELECT も source の有効性を再検査しない。現行 digest は本文を含まないため、削除原文の漏出を今回実証したわけではない。しかし forget 後の stale report 抑止・派生本文削除の保証はない。
- Scope revoke: `runtime/context/scope.rs:433` は scope state と epoch を更新する。delegated dispatch と runner の `source_valid` は context_scopes を照合しない。Steward 側の Scope revoke 接続・試験は確認できない。
- 撤回停止の補足: commands/tools は撤回後に cancel を呼び、authorize が active lineage を要求するため cancel エラーを捨て得る。ただし runner の `stopping` は source_valid 不成立を検出して abort する。従って「撤回しても絶対に止まらない」とは判定しない。即時 cancel receipt の保証と実 process 停止の組合せが未検証である。
- 対象カード: DW-12 / DW-13。実 TTS 正常・renderer 失敗の成功は有効だが、DW-12 全条件の完了根拠には不足。

### A11 / P2: 直接登録した Goal が Task 作成前はパネルに表示されない

- 根拠: `commands::register_steward_goal` は Goal/Delegation/plan を保存するだけ。`repository::list`（304行）は steward_tasks を起点に JOIN するため、未開始 Goal は0行。`StewardPanel` はこの一覧だけから Goal ごとの撤回ボタンを作る。
- 影響: 明示登録後、固定トリガ等で Task を作る前に A/B を見分けて撤回する UI が成立しない。これはクリック受入がないだけでなく、host の返却契約の欠落。
- 既存試験の穴: `confirmed registration sends only the selected bounded Goal scope` は正確な IPC payload を確認するだけで、登録後の実 host 一覧を接続していない。multiple Goals の UI 試験は最初から Task を2行 mock している。
- 対象カード: DW-04 / DW-14。

### A12 / 要追加実験: service receipt と Steward job binding の間のクラッシュ・高速終端

- 根拠: `coding/service.rs:193` 以降で Coding origin/run を commit し runner thread を開始する。その戻り後に `reduce.rs` が Task.coding_job_id と intent receipt を保存する。終端 consumer は Task.coding_job_id で検索する。
- 懸念: binding前に終端が commit される順序では consumer が Task を見つけず cursor だけ進める。後の sync は状態を写すが、同じ artifact/依存/replan 適用を行わない。receipt前クラッシュも unknown で止めるだけで、起動済み job と Task の再照合範囲が不足する可能性がある。
- 判定: 順序上の窓はコードで確認。今回、実 runner に fault を入れた再現はしていない。A01〜A11 の確認済みコード欠落と区別して次回実験対象にする。
- 対象カード: DW-05 / DW-06 / DW-08 / DW-10。

## 2. 既存自動受入で証明できている範囲

| 試験・証跡 | 証明する範囲 | 証明しない範囲 |
| --- | --- | --- |
| dw_01 / dw_02 / dw_03 | 型・数値上限、user source存在、直接登録、予約・重複抑止の個別操作 | 自然文意味、承認包含、複数Goal再open、受付から実行まで |
| dw_06_restart_marks_unreceived_dispatch_unknown_without_reclaiming | dispatching→unknown、同じintent再claim拒否 | 全送信/commit境界、既存job再照合、busy復旧 |
| dw_10 の依存・migration・replan試験 | 直接登録由来のplan、依存Task enqueue、再計画版数上限、job参照 | 自然文由来plan、実test結果verifier、成果物内容、Goal完了 |
| dw_14_multiple_goals_keep_the_sibling_through_topic_switch_withdrawal_and_hold | DB fixtureでA撤回/B維持、話題切替、手動publish/hold解除 | B撤回の逆順、実dispatcher競合、UI表示、sleep/restart |
| delegated_tasks_compete_for_one_production_coding_slot | 本番service/runnerとscripted Piで実slot競合、明示再呼出し後のB完了 | Steward自動retry、公平性、実LLMによるA/B仕事 |
| coding_restart_never_resends_and_blocks_live_or_ambiguous_process | process不明時に再送しない安全側回復 | 複数Goalのアプリ再起動、ユーザーへ判断待ちを自動配送 |
| report uniqueness / speech restart試験 | outboxの一意キー、speech claimの再生抑止 | message挿入との原子性、hold中forget、Chat更新 |
| opt-in real TTS 2件（既存実行記録） | 本物のrenderer/playerの終了callbackと失敗callbackのdurable記録 | 全設定OFF/ON、実UIの一連の配送、他音声と競合する場面 |
| opt-in real SDK（既存実行記録） | 少なくともread成功、報告されたwrite/network/scope拒否 | 登録test recipe、read/test差、時間予算、SAAA自然文からの一連の経路 |
| jsdomパネル2件 | 選択したGoal ID、確認済み登録payload | 実IPC・実DBとの一致、登録直後の表示、Chat再接続 |
| packaged desktop smoke（既存実行記録） | build/bundle/launch/IPC-ready/cleanup | 業務操作・報告内容・複数Goal再open・sleep/wake |

## 3. 今回の再検証

- `bun test tests/delegated-work-cases.test.ts tests/steward-panel.test.tsx tests/coding-steward.test.ts tests/pi-codex-sdk.test.ts`: 8 pass。
- `cargo test --lib steward:: -- --test-threads=1`: 36 pass / 2 ignored（実TTS）。
- `cargo test --lib coding:: -- --test-threads=1`: 13 pass / 3 ignored（実認証等）。競合・delegated origin・forgetのproduction fixtureを含む。
- `cargo test --lib schedule::tests -- --test-threads=1`: 18 pass。
- `bun run ipc:check`: 4 + 1 pass。`bun run typecheck`: pass。
- `bun run spec:check`: role-routing と world の文書参照 REF003 が2件。委任仕事の実装欠陥とは別。
- `bun run check:local`: world評価script・role-routing/worldテスト計3ファイルの整形で停止。lint/checkへ進んだと扱わない。
- `bun run size:check`: fail。対象外だけでなく steward/repository、schema、report、reduce、commands、StewardPanel の超過、contracts/driver/tools 等のbaseline不足を含む。「すべて無関係な既存超過」として免責できない。
- A01 の実 migration SQL を一時ファイルDBで再openして再現。
- 実SDK・実TTS・packaged smokeは今回再実行していない。既存results記載を今回の実測と混同しない。

results には「認証SDK turn未完了」「app駆動音声未確認」という古い記述と、後段の完了記述が併存する。最新版を時系列・profile別に整理する必要がある。SDK拒否3種は、再実行可能なケースと結果に対応付ける。完全な source→delegation revision→plan/step→intent→Coding receipt/result→verifier→report/message/speech の追跡表も不足している。

## 4. native UI 自動操作の代替手段

このセッションでは CUA を実際に照会し、`apps: []`、Codex In-app Browser のみを確認した。native API はセッションの tool 定義でも無効。既存SAAAのネイティブ画面を今すぐ操作できる接続はない。

ただし「macOSで代替手段が存在しない」という結論は誤り。

| 手段 | 調査結果 | 必要条件・残る確認 |
| --- | --- | --- |
| WebdriverIO + embedded WebDriver | 現行Tauri公式資料でmacOS対応。外部tauri-driver不要 | SAAAにtest用 `tauri-plugin-wdio-webdriver` とWDIO設定を追加し再build。必要に応じtauri-plugin-wdioも追加。現行依存・設定にはない。まず互換性の短い実証が必要 |
| Xcode XCUIAutomation / XCTest | Xcode 26.3、xcodebuild/xcrun が存在。Appleの公式UIテスト方式 | macOS UI test harness、SAAAのAX要素可視性、Xcode Helper等のAccessibility許可、ログイン済みGUI sessionを確認。今回harnessの作成・操作はしていない |
| CUA native surfaceを利用できる実行環境 | 製品版bundleをそのまま操作する選択肢 | native surface有効化と必要OS権限。現在のapps空を改善する外部条件が必要 |
| AppleScript / AX / CGEvent | osascriptは存在。代替自動化技術として候補 | Accessibility/Automation許可と専用harnessが必要。今回のCUA利用規則では非CUA方式による操作に明示指定が必要で、調査のみ。実操作可とは判定しない |
| SafariDriver単体 | safaridriverは存在 | 存在だけではTauri WKWebViewを操作できない。外部tauri-driver単体のmacOS非対応問題を解消しない |
| 通常browser + IPC mock | UI部品検証を追加できる | 実Tauri/WKWebView/OS lifecycleの受入代替にはならない |

参照: [Tauri WebDriver公式資料](https://v2.tauri.app/develop/tests/webdriver/)、[WebdriverIO Plugin Setup](https://webdriver.io/docs/desktop-testing/tauri/plugin-setup/)、[Apple XCUIAutomation](https://developer.apple.com/documentation/xcuiautomation)、[Apple UI automation recording](https://developer.apple.com/documentation/XCUIAutomation/recording-ui-automation-for-testing)。Tauri英語資料は2026-06-29更新と表示され、古い説明や翻訳とは内容差がある。

embedded方式は実アプリのWebViewを操作するが、instrumented buildである。配布版bundleそのものの検証とは区別する。mockで仕事を完了させず、実IPC・実DB・実profileへ接続する。OS sleep/wake、native dialog、外部audioについては別途OS側の制御・観測が必要であり、WebDriver導入だけで全条件を満たすとは断定しない。

## 5. 次の改善計画に渡すべき範囲

計画を「UI受入環境の用意」だけにしない。まず A01〜A11 の実装契約の穴を塞ぎ、A12 の境界を実験で確定する。各指摘を再現する試験を追加し、既存の部品試験と経路全体の試験を分ける。

再判定の目安: DW-02/03/04/06/07/08/10/11/12/13 は部分完了へ戻す根拠がある。DW-05のorigin分離、DW-09のreceipt後Started等は部分的に確認できるが、共有dispatcherの欠陥を直した後に再受入する。DW-00のbaselineやDW-01の既存validator等の成果を破棄する必要はない。ただしDW-01には計画の引用/source/authority/期限など、実際の型と一致しない契約が残る。DW-14は実装修正と統合受入の両方が未完了。

外部条件が必要なのは、実モデル認証と対応profile、破棄可能なGit workspaceと隔離DB、GUI操作接続またはtest harness、音声出力、制御可能なsleep/wake/restart環境。実装欠落やhostのfault試験はこれらを理由に先送りする必要がない。

最終受入では、20の明示表現・10の非採用/確認表現を実際の受付経路に入力する。通常完了・失敗・撤回・判断待ち・再起動・hold解除を実モデル/実profile/実UIで記録し、A/B両方向の撤回と自動queue継続、設定OFF/ON、forget/revoke、成果物とverifier出典、Chatと音声配送を追跡する。

時間目標も残件: 終端→driver p95 2秒、hold解除→会話報告 p95 2秒、期限処理1tick+5秒、復帰時照合の統合計測は確認できない。既存schedule reducer/tickのミリ秒単位性能試験は、このアプリ全体の遅延証明ではない。
