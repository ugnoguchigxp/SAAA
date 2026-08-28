# MVP 2.5 実装契約 — Agent Run Supervisor and Input Activity Signal

## 0. 文書の役割

- Status: Ready for implementation
- Reviewed: 2026-08-27
- 実装前提: [`mvp-2-implementation-plan.md`](./mvp-2-implementation-plan.md) のCore実装が完了していること
- 引き継ぐ未完了事項: [`mvp-2-release-evidence.md`](./mvp-2-release-evidence.md) のmanual desktop evidence。MVP 2.5はこれを完了扱いにしない
- shipping platform: macOS desktop
- 読者: 実装担当、レビュー担当、QA担当
- 完了の定義: `M25-00`〜`M25-10`を順に完了し、本文の受け入れ条件を証跡付きで満たすこと

この文書はロードマップではなく実装契約である。本文に選択肢が書かれていない限り、実装担当は別方式を選ばない。契約どおり実装できない場合は推測で進めず、Section 16のStop conditionとして計画を改訂する。

MVP 2.5は新しい自律機能を増やさない。MVP 0〜2のAgent Runを確実に停止・回収できるようにし、Situationへprivacy-boundedな弱い入力活動信号を一つ追加する。

参照するOpenClaw実装:

- Agent Run Supervisorの概念: `../openclaw/extensions/codex/src/app-server/attempt-turn-watches.ts`
- Input Activity APIの概念: `../openclaw/apps/macos/Sources/OpenClaw/SystemPresenceInfo.swift`

OpenClawのコード、型、event名は移植しない。SAAAの既存route、SQLite lifecycle、Shadow policyに合わせて実装する。

## 1. レビュー結果と確定した修正

初版には実装担当の判断が必要な箇所が残っていた。レビュー後は次のように固定する。

| 初版の問題 | 確定した修正 |
|---|---|
| OpenClaw linkが存在しない`../open-claw`を指していた | 実在する`../openclaw`へ修正した |
| macOS bindingをP0で選ぶことになっていた | `objc2-core-graphics 0.3.2`とfeatureを固定した |
| Input Activityがshipping adapterかunsupported fallbackか未決定だった | macOS adapterを実装する。予期しないpermission promptが出た場合だけStop conditionとする |
| `active_runs`を`ActiveRunHandle`へ置換し、全routeへ影響する案だった | `active_runs: HashMap<String, Arc<RunCancellation>>`を維持し、SupervisorはCodex worker内だけに置く |
| 成功時の`MessageCompleted`が`runtime_runs`確定前に送られる現行フローとの不整合があった | Codex success/failureはSQLite transaction commit後にterminal eventを送る |
| `turn/completed`以外のcompletion監視条件が曖昧だった | `agentMessage`完了かつactive item 0件のときだけterminal-gap watchを開始する |
| active item/requestの扱いが未定義だった | item idをadapter内の`HashSet`で追跡する。server requestはapproval policy違反として即時失敗させる |
| 新しいactive calibration profileを自動生成する案はrollback semanticsを壊す | profile rowと`RULE_VERSION`を変更しない。新fieldはserde defaultで補い、policy versionだけを上げる |
| Input Activityを`user_attention`へ反映する案はscene stateを変え得た | state、scene、confidence、evidence、user_attentionを一切変えず、Shadow policyだけを変更する |
| Calibration UIでは新thresholdを編集しない記述だった | 現行のcandidate editorへ2 fieldを追加する。新しいSettings画面は作らない |
| Shadow policyがactive calibrationのconfidence thresholdを使わず`45`/`70`をhardcodeしていた | `shadow_policy`へparametersを渡し、既存fieldをsource of truthにする |
| request timeoutとhard timeoutの起点が曖昧だった | thread start/resumeとturn startは各20秒。hard timeoutはturn/start response受信時から開始する |

## 2. 変更前baseline

実装前に次をbaselineとして記録する。挙動を変更すると本文に明記した箇所以外は、このbaselineを維持する。

| 領域 | 現在のsource of truth | 維持条件 |
|---|---|---|
| active run | `AppState.active_runs: Mutex<HashMap<String, Arc<RunCancellation>>>` | map型、重複run id拒否、window close時の全cancelを維持 |
| Codex process | `run_codex_turn_process`と`ProcessGuard` | process-per-run、read-only config、Drop時kill/waitを維持 |
| Codex success | non-empty assistant textを伴う`turn/completed: completed` | 成功条件を緩めない |
| Codex RPC | initialize送信、thread start/resume response、turn start response | method、request id `1/2/3`、20秒上限を維持 |
| Codex cancellation | `RunCancellation`のatomic flag | `cancel_run`をidempotentなまま維持 |
| runtime persistence | `runtime_runs` | statusのCHECK値を増やさない |
| restart recovery | `running -> interrupted` | 起動時reconcileを維持 |
| Situation runtime | Rustの`SituationRuntime` | React stateをsource of truthにしない |
| Situation sample | 既存2秒tick | sample intervalとworker lifecycleを維持 |
| Situation persistence | transition/decision/heartbeatだけ | 毎tickのsnapshotをSQLiteへ保存しない |
| Situation classification | foreground/conversation/microphone/audio/calendar | Input Activityでscene scoreを変更しない |
| Shadow safety | `actualExecution = NONE`, `actualPresentation = SILENT` | 自動介入を追加しない |
| calibration | active `CalibrationProfile`のparameters | 既存profile rowとrule versionを自動書換えしない |

実装開始時に次を実行し、結果をM25-10のrelease evidenceへ転記する。

```bash
git status --short
bun run typecheck
cargo test --manifest-path src-tauri/Cargo.toml
```

baselineが失敗する場合は、MVP 2.5と無関係な失敗かを切り分けて記録する。無関係な既存変更を修正しない。

## 3. 今回達成すること

### 3.1 Agent Run Supervisor

Codex app-serverによる1回のAgent Runについて、request待機、進捗、assistant出力完了、terminal event、取消、child終了を一つのstate machineで監督する。

保証すること:

- thread start/resumeとturn startのrequestが各20秒を超えない
- turn開始後、meaningful progressが60秒止まったrunを回収する
- assistant item完了後、`turn/completed`が10秒来ないrunを回収する
- routing hard timeoutを超えたrunを回収する
- cancel/timeout/policy violationで`turn/interrupt`を最大1回だけ送る
- interrupt後3秒でterminal eventが来なければchildをkill/waitする
- terminal reasonをbounded codeで永続化する
- prompt、tool argument、raw app-server eventをSupervisor diagnosticsへ複製しない

### 3.2 Input Activity Signal

macOSのCoreGraphics APIから「最後のkeyboard/mouse/tablet入力からの経過時間」を取得し、同じRust call stack内で`active | recent | idle | unknown`へ変換する。

使用目的は次だけである。

- Situation Snapshotへcategoryとhealthを追加する
- `idle`かつ明示SAAA interactionがないとき、Shadowの`SUGGEST`を`OBSERVE`へ抑制する
- calibration candidateでcategory境界を比較する
- replay fixtureでpolicy回帰を検証する

### 3.3 不変の安全境界

```text
automaticModelCallFromSituation = false
automaticMeetingStart = false
automaticTTSFromSituation = false
automaticNotification = false
automaticApplicationAction = false
rawInputEventCollection = false
rawInputIdleDurationPersistence = false
inputActivityMeansSafeToInterrupt = false
actualExecution = NONE
actualPresentation = SILENT
```

Codex routeはread-only、approval `never`、network/Web Search/write-capable MCP無効を維持する。

## 4. Scope

### 4.1 必ずshipする

- Codex `coding.assist` routeだけにAgent Run Supervisorを導入
- request/progress/terminal-gap/hard timeout
- idempotent Cancel、one-shot interrupt、3秒drain、kill/wait
- structured failure code、Supervisor version、last progress timestamp
- macOS Input Activity adapter
- Input Activity DTO、Situation Snapshot、health、UI、calibration、replay
- idleによるShadow `SUGGEST`抑制
- SQLite/settings schema v7
- pure unit test、fake app-server test、migration test、privacy test

### 4.2 行わない

- conversation provider、voice STT/TTS、Meeting segmentのSupervisor移行
- `active_runs` mapの型変更
- Agent Runの自動retryまたはprovider fallback追加
- timeout値のSettings UI
- adaptive timeout
- exact idle seconds、last input timestamp、key、button、cursor positionの保存・送信・表示
- 人物の在席・離席推定
- idleを根拠にしたMeeting、通知、TTS、model call、application action
- Accessibility/Input Monitoring permission要求UI
- Windows/Linux Input Activity実装
- OpenClaw/HermesAgent互換層
- Situation scene scoreまたはevidence weightの変更

## 5. 変更ファイル一覧

### 5.1 新規作成

```text
src-tauri/src/run_supervisor.rs
src-tauri/fixtures/situation/mvp2.5-v2.json
spec/docs/spikes/mvp-2.5-macos-input-activity.md
spec/docs/adr/0003-input-activity-signal-privacy.md
spec/docs/mvp-2.5-release-evidence.md
```

### 5.2 更新

```text
src-tauri/Cargo.toml
src-tauri/Cargo.lock
src-tauri/src/lib.rs
src-tauri/src/situation/contracts.rs
src-tauri/src/situation/classifier.rs
src-tauri/src/situation/mod.rs
src-tauri/src/situation/calibration.rs
src-tauri/src/situation/platform/mod.rs
src-tauri/src/situation/platform/macos.rs
src-tauri/src/situation/platform/unsupported.rs
src/lib/contracts.ts
src/lib/schemas.ts
src/features/settings/SettingsPage.tsx
src/features/situation/SituationPage.tsx
src/features/situation/review/SituationReview.tsx
tests/settings.test.ts
```

`src/lib/runtime.ts`、`src/App.css`、Meeting filesは契約変更が不要なので変更しない。Situationの既存`signal-health-grid`をそのまま使い、新しいCSS classは作らない。

## 6. Agent Run Supervisor — Rust contract

### 6.1 モジュール境界

新規fileは一つだけとする。

```text
src-tauri/src/run_supervisor.rs
```

責務:

- phaseとwatch timestampの保持
- signalを受けた決定論的遷移
- `SendInterrupt`または`Finish` effectの返却
- failure code、status、last progress metadataの生成

禁止する依存:

- Tauri
- SQLite/rusqlite
- `std::process::Child`
- JSON/serde_json
- wall clock取得
- sleep/timer生成

時刻は`u64`のmonotonic millisecondsをcallerから受ける。`lib.rs`側では`Instant`をrun開始時に一つ作り、`origin.elapsed().as_millis()`を`u64`へsaturating変換して渡す。

Codex JSON-RPC、process、Tauri Channel、SQLite orchestrationは現行どおり`lib.rs`に残す。MVP 2.5でCodex helper全体のfile移動は行わない。

### 6.2 固定する型

`run_supervisor.rs`へ次のshapeで定義する。field名とenum値を変更しない。

```rust
pub(crate) const SUPERVISOR_VERSION: &str = "mvp2.5-supervisor-v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RunPhase {
    Starting,
    Running,
    Draining,
    Interrupting,
    Terminal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RunFailureCode {
    UserCancelled,
    AppRestarted,
    ConfigurationError,
    ChildStartFailed,
    RequestTimeout,
    ProgressTimeout,
    TerminalTimeout,
    HardTimeout,
    ChildExited,
    ProtocolError,
    PolicyViolation,
    ProviderError,
    ResponseTooLarge,
    InternalError,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RunTerminalStatus {
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RunTerminal {
    pub status: RunTerminalStatus,
    pub failure_code: Option<RunFailureCode>,
    pub last_progress_elapsed_ms: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SupervisorEffect {
    SendInterrupt,
    Finish(RunTerminal),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RunSupervisionPolicy {
    pub request_timeout_ms: u64,
    pub progress_idle_timeout_ms: u64,
    pub terminal_gap_timeout_ms: u64,
    pub interrupt_grace_ms: u64,
    pub hard_timeout_ms: u64,
}
```

`RunFailureCode::as_str()`は次を返す。

```text
user-cancelled
app-restarted
configuration-error
child-start-failed
request-timeout
progress-timeout
terminal-timeout
hard-timeout
child-exited
protocol-error
policy-violation
provider-error
response-too-large
internal-error
```

production policy:

```rust
RunSupervisionPolicy {
    request_timeout_ms: 20_000,
    progress_idle_timeout_ms: 60_000,
    terminal_gap_timeout_ms: 10_000,
    interrupt_grace_ms: 3_000,
    hard_timeout_ms: routing.coding_assist.timeout_ms,
}
```

testは同じ型へ短い値を渡す。production constantをtest都合で変更しない。

### 6.3 Supervisor内部field

```rust
pub(crate) struct RunSupervisor {
    policy: RunSupervisionPolicy,
    phase: RunPhase,
    request_deadline_ms: Option<u64>,
    hard_deadline_ms: Option<u64>,
    last_progress_ms: Option<u64>,
    terminal_gap_started_ms: Option<u64>,
    interrupt_started_ms: Option<u64>,
    pending_failure: Option<RunFailureCode>,
    interrupt_sent: bool,
    terminal: Option<RunTerminal>,
}
```

timestamp加算は全て`saturating_add`を使う。deadline比較は`now_ms >= deadline_ms`とする。

### 6.4 公開methodと意味

```rust
pub(crate) fn new(policy: RunSupervisionPolicy) -> Self;
pub(crate) fn begin_request(&mut self, now_ms: u64);
pub(crate) fn complete_request(&mut self);
pub(crate) fn mark_turn_started(&mut self, now_ms: u64);
pub(crate) fn record_progress(&mut self, now_ms: u64);
pub(crate) fn record_assistant_output_completed(&mut self, now_ms: u64);
pub(crate) fn cancel(&mut self, now_ms: u64, interruptable: bool) -> Option<SupervisorEffect>;
pub(crate) fn fail(
    &mut self,
    now_ms: u64,
    code: RunFailureCode,
    interruptable: bool,
) -> Option<SupervisorEffect>;
pub(crate) fn terminal(
    &mut self,
    provider_status: &str,
    has_nonempty_content: bool,
) -> Option<SupervisorEffect>;
pub(crate) fn next_deadline_ms(&self) -> Option<u64>;
pub(crate) fn finish_pending(&mut self) -> Option<SupervisorEffect>;
pub(crate) fn tick(&mut self, now_ms: u64) -> Option<SupervisorEffect>;
```

method contract:

- `begin_request`: phaseをStartingに維持し、`request_deadline = now + 20_000`にする。
- `complete_request`: request deadlineをclearする。
- `mark_turn_started`: request deadlineをclearし、phaseをRunning、`last_progress = now`、`hard_deadline = now + hard_timeout`にする。
- `record_progress`: Running/Drainingだけで有効。phaseをRunningへ戻し、terminal-gapをclearし、last progressを更新する。Starting/Interrupting/Terminalではno-op。
- `record_assistant_output_completed`: RunningのときだけphaseをDrainingにし、terminal-gap開始時刻とlast progressをnowへ更新する。
- `cancel`: Terminalならno-op。`pending_failure = UserCancelled`。interruptable=falseなら即`Finish(Cancelled)`、trueならInterruptingへ移り`SendInterrupt`を一度だけ返す。
- `fail`: Terminalならno-op。通常はfailure codeを保持し、interruptable=falseなら即`Finish(Failed)`、trueならInterruptingへ移り`SendInterrupt`を一度だけ返す。既にInterruptingならreasonを上書きせず、interruptable=falseのときだけ`finish_pending()`と同じeffectを返す。
- `terminal`: pending failureがあればprovider statusよりpending failureを優先する。pendingがなければSection 6.8の表で確定する。
- `next_deadline_ms`: 現phaseで有効なdeadlineの最小値を返す。deadlineが無ければ`None`。
- `finish_pending`: Interruptingのときだけpending failureをterminalへ確定して`Finish`を返す。interrupt write失敗とchild exitで使う。pendingが無い状態では`InternalError`として確定する。
- `tick`: Section 6.6の優先順でwatchを評価する。

`Finish`を返すと同時にphaseをTerminalへ変更し、同じ`RunTerminal`を保持する。その後のmethod callは全て`None`を返す。

最初に確定した終了理由を上書きしない。Interrupting中の`cancel`/`fail`は`pending_failure`を変更しない。Interrupting中にchild exitまたはinterrupt write失敗が起きた場合は`finish_pending()`で最初の理由を確定する。

`next_deadline_ms`のphase別計算:

| phase | deadline |
|---|---|
| Starting | `request_deadline_ms` |
| Running | `min(hard_deadline_ms, last_progress_ms + progress_idle_timeout_ms)` |
| Draining | `min(hard_deadline_ms, terminal_gap_started_ms + terminal_gap_timeout_ms)` |
| Interrupting | `interrupt_started_ms + interrupt_grace_ms` |
| Terminal | `None` |

表の`Option`値が無い項目はmin候補から除く。加算は`saturating_add`を使う。

### 6.5 状態遷移

| From | Input | To | Effect |
|---|---|---|---|
| Starting | request response | Starting | deadline clear |
| Starting | turn/start response | Running | hard/progress clock開始 |
| Starting | request timeout | Terminal | Failed/request-timeout。turn idがないためinterruptしない |
| Starting | Cancel | Terminal | Cancelled/user-cancelled。childはcallerがkill |
| Running | meaningful progress | Running | last progress更新 |
| Running | agentMessage completed、active item 0 | Draining | terminal-gap開始 |
| Draining | meaningful progress | Running | terminal-gap解除 |
| Running/Draining | Cancel | Interrupting | SendInterrupt |
| Running | progress timeout | Interrupting | SendInterrupt、pending progress-timeout |
| Draining | terminal timeout | Interrupting | SendInterrupt、pending terminal-timeout |
| Running/Draining | hard timeout | Interrupting | SendInterrupt、pending hard-timeout |
| 非Terminal | policy violation | Interrupting | turn idがあればSendInterrupt、なければ即Failed |
| Interrupting | provider terminal | Terminal | pending resultを維持 |
| Interrupting | 3秒経過 | Terminal | pending resultを維持。callerがkill |
| Running/Draining | provider terminal | Terminal | provider statusから確定 |
| Starting/Running/Draining | stdout disconnect/child exit | Terminal | Failed/child-exited |
| Interrupting | stdout disconnect/child exit | Terminal | pending resultを維持 |

### 6.6 watch評価順

Codex loopでは次の順を守る。

1. cancellation flagを確認する。まだ受理していなければ`cancel()`する。
2. receiverを「次に到達するdeadlineまで、最大100ms」で待つ。
3. messageを受信した場合はmessageを先に処理する。
4. message処理後にTerminalでなければ、必ず`tick(now)`を一度呼ぶ。recv timeout時も`tick(now)`を呼ぶ。これによりforeign/unknown messageの連続投入でwatchが飢餓しない。
5. disconnect時は`fail(ChildExited, interruptable = false)`を適用する。既にInterruptingなら最初のreasonが維持される。
6. `tick`内部では次の順に判定する。
   1. 既存Interruptingの3秒grace
   2. Startingのrequest deadline
   3. hard deadline
   4. Drainingのterminal-gap deadline
   5. Runningのprogress-idle deadline

deadlineと同時刻にreceiver queueへ既に入っていた`turn/completed`は、step 3によりterminalとして先に処理する。cancel flagが同じiterationで立っていた場合はcancelが優先される。message処理と`tick`の両方がeffectを返した場合、message側effectを先に適用し、Terminalへ入った後の`tick`は`None`になる。

### 6.7 meaningful progress

progress clockを更新するnotification:

| notification | 条件 |
|---|---|
| `turn/started` | current thread/turnと一致 |
| `item/agentMessage/delta` | current thread/turnと一致し、deltaが空でない |
| `item/started` | current thread/turnと一致し、item typeが`commandExecution | reasoning | plan | agentMessage` |
| `item/completed` | current thread/turnと一致し、item typeが上記allowlist |

更新しないもの:

- 100ms poll
- empty delta
- unknown method
- `userMessage`
- current thread/turnと一致しないnotification
- JSON parse error
- 同じitem idのduplicate start/completion

forbidden item:

```text
fileChange
mcpToolCall
dynamicToolCall
webSearch
```

forbidden itemを受けたら`policy-violation`としてinterruptする。unknown item typeは既存のbounded Activity表示を維持してよいが、progress clockを更新しない。

### 6.8 provider terminal mapping

`turn/completed`はcurrent thread/turnと一致する場合だけ処理する。

| provider status/content | terminal |
|---|---|
| `completed`かつtrim後contentあり | Completed、failure codeなし |
| `completed`かつcontent空 | Failed/`provider-error` |
| `interrupted`かつpending UserCancelled | Cancelled/`user-cancelled` |
| `interrupted`かつpending timeout/policy | pending Failedを維持 |
| `interrupted`かつpendingなし | Failed/`provider-error` |
| `failed`またはunknown status | Failed/`provider-error` |

`method = error`は、thread/turn idが現在と一致するか、idが全く無いprocess-global errorの場合だけFailed/`provider-error`とする。明示的に別thread/turnを指すerrorは無視する。

### 6.9 notification correlationとactive item tracking

`lib.rs`のCodex adapter内に次を追加する。

```rust
let mut active_item_ids = HashSet::<String>::new();
let mut assistant_output_completed = false;
```

correlation path:

```text
/params/threadId
/params/turnId
```

terminal payloadで上記が無い場合だけ次も確認する。

```text
/params/turn/threadId
/params/turn/id
```

現在の`thread_id`と`turn_id`の両方が一致したnotificationだけをcurrentとする。どちらかが存在して不一致ならforeignとして無視する。両方とも無いdelta/item/terminalはprotocol progressとして扱わず無視する。

item idは`/params/item/id`から読み、既存identifierと同じ文字allowlist、最大160文字で検証する。

- valid `item/started`: setへinsert。既存idならduplicateとしてprogress更新しない。
- valid `item/completed`: setからremove。存在しなければduplicate/out-of-orderとしてprogress更新しない。
- valid `agentMessage` startでは`assistant_output_completed = false`。
- valid `agentMessage` completionでは`assistant_output_completed = true`。
- valid completion後、`assistant_output_completed == true`かつsetが空なら`record_assistant_output_completed`を呼ぶ。agentMessageより後に別itemが完了する順序でもterminal-gapを開始できる。
- Draining中に新しいvalid itemが始まったら`record_progress`によりRunningへ戻る。
- missing/invalid item idはUI projectionだけ行い、active setとwatchを変更しない。

assistant contentは既存どおりdeltaを連結し、deltaが一件も無い場合だけcompleted agentMessageの`text`をfallbackに使う。いずれも合計64,000文字を超える前に`response-too-large`で失敗させる。

serverから`id`と`method`を持つrequestが来た場合は、現在と同じくnever-approval routeへの要求とみなし`policy-violation`にする。MVP 2.5ではactive request countを持たない。

### 6.10 request waitの置換

既存`receive_codex_result`を次の責務へ拡張し、thread response id=2とturn response id=3だけに使う。

```rust
fn receive_supervised_codex_result(
    receiver: &mpsc::Receiver<CodexReaderMessage>,
    request_id: u64,
    supervisor: &mut RunSupervisor,
    origin: &Instant,
    cancellation: &RunCancellation,
) -> Result<Value, RuntimeFailure>;
```

処理:

1. `begin_request(elapsed_ms)`。
2. 最大100msごとにcancellationとrequest deadlineを確認。
3. cancellation時はStarting/interruptable=falseでCancelledを返す。
4. 受信JSONが`id`と`method`を両方持つ場合は、response idを判定する前にserver requestとして`policy-violation`にする。
5. expected response受信時に`complete_request`。
6. response errorは`provider-error`。
7. invalid JSONは`protocol-error`。
8. channel disconnectは`child-exited`。
9. request deadline到達は`request-timeout`。
10. expected id以外のresponse/notificationは捨てる。

initialize request id=1は現行どおりresponseを待たず、`initialized`を送る。request timeout watchの対象はid=2とid=3だけである。

readerとreceiverの型は次に固定する。

```rust
const MAX_CODEX_STDOUT_BYTES: u64 = 4 * 1_024 * 1_024;

enum CodexReaderMessage {
    Message(Value),
    Failed {
        code: RunFailureCode,
        message: &'static str,
    },
}
```

stdout readerは`stdout.take(MAX_CODEX_STDOUT_BYTES + 1)`を`BufReader`で読み、改行単位でJSONをdecodeする。総read byteが4 MiBを超えた場合は`ResponseTooLarge`、I/O errorまたはJSON decode errorは`ProtocolError`を一度送って終了する。error messageは固定文字列とし、raw lineやparser inputを含めない。`receive_supervised_codex_result`とturn loopは同じtyped messageを処理する。

### 6.11 `run_codex_turn_process` loop疑似コード

```rust
spawn child or ChildStartFailed
take stdin/stdout or ChildStartFailed
create sync_channel with capacity 256
spawn stdout_reader JoinHandle that sends CodexReaderMessage
let origin = Instant::now()
let supervisor = RunSupervisor::new(production_policy)

write initialize + initialized
write thread/start or thread/resume
receive_supervised_codex_result(id=2)
resolve thread_id

write turn/start
receive_supervised_codex_result(id=3)
resolve turn_id
supervisor.mark_turn_started(elapsed_ms)
last_progress_at = Some(now_iso())

loop {
    if cancellation newly observed:
        apply supervisor.cancel(interruptable=true)

    recv up to min(100ms, next supervisor deadline)

    on current non-empty delta:
        enforce total 64_000 chars
        assistant_output_completed = false
        supervisor.record_progress(elapsed_ms)
        last_progress_at = Some(now_iso())
        send RuntimeEvent::Delta

    on current valid item start/completion:
        update active_item_ids
        if started agentMessage: assistant_output_completed = false
        if completed agentMessage: assistant_output_completed = true
        supervisor.record_progress(elapsed_ms)
        last_progress_at = Some(now_iso())
        project existing bounded Activity
        if assistant_output_completed and active_item_ids empty:
            supervisor.record_assistant_output_completed(elapsed_ms)

    on current turn/completed:
        apply supervisor.terminal(status, has_content)

    on forbidden item/server request:
        apply supervisor.fail(PolicyViolation, interruptable=true)

    on recv timeout:
        apply supervisor.tick(elapsed_ms)

    after any non-terminal received message:
        apply supervisor.tick(elapsed_ms)

    on recv disconnect after queue is empty:
        apply supervisor.fail(ChildExited, interruptable=false)

    on SendInterrupt:
        write turn/interrupt with request id=4 exactly once
        if write fails, apply supervisor.finish_pending()

    on Finish:
        break with typed outcome
}

drop stdin
drop receiver
kill/wait child through ProcessGuard::terminate()
join stdout_reader
return typed outcome with thread_id/content/last_progress_at
```

reader joinがpanicした場合、予定outcomeがCompletedならFailed/`internal-error`へ置換する。予定outcomeが既にFailedまたはCancelledなら最初の終了理由を維持する。

`turn/interrupt` response id=4は待たない。terminal notificationだけを待つ。

### 6.12 process cleanup

`ProcessGuard`へidempotentなmethodを追加する。`terminated`はconstructorで`false`にする。

```rust
struct ProcessGuard {
    child: Child,
    terminated: bool,
}

pub(crate) fn terminate(&mut self) {
    if self.terminated {
        return;
    }
    let _ = self.child.kill();
    let _ = self.child.wait();
    self.terminated = true;
}
```

`Drop`も`terminate()`を呼ぶ。二重呼出しを安全にする。stdout readerは`map_while(Result::ok)`をやめ、read errorを`CodexReaderMessage::Failed`として一度送って終了する。

cleanup順序:

```text
Supervisor terminal確定
  -> 新しいUI delta/activityを停止
  -> stdin close
  -> receiver drop
  -> child kill/wait
  -> stdout reader join
  -> SQLite transaction commit
  -> terminal Tauri event
  -> `execute_turn` return
  -> active_runsからremove
```

`start_turn`のfinally相当で行う`active_runs.remove`は維持する。process cleanupは`execute_codex_turn`がreturnする前に完了していること。

## 7. Runtime persistence and event ordering

### 7.1 typed result

`lib.rs`へ次を追加する。

```rust
struct RunSupervisionMetadata {
    supervisor_version: &'static str,
    last_progress_at: Option<String>,
}

struct RuntimeFailure {
    code: RunFailureCode,
    message: String,
    recovery: String,
    supervision: RunSupervisionMetadata,
}

struct CodexTurnOutcome {
    thread_id: String,
    content: String,
    supervision: RunSupervisionMetadata,
}

struct CodexTurnFailure {
    thread_id: Option<String>,
    failure: RuntimeFailure,
}
```

`RuntimeFailure`はCodex route内だけで使う。全messageは`redact_runtime_text`を通す。`recovery`は固定文字列だけを使い、provider errorを連結しない。thread/turn request前のconfiguration/child start失敗も`last_progress_at = None`を持つ同じ型へ変換する。

### 7.2 coding runのtransaction

`execute_codex_turn`はprocess結果だけでreturnしない。次のtransactionをcommitしてからcallerへ返す。

success transaction:

1. `codex_threads`をupsert。
2. assistant `conversation_messages`をinsert。
3. `conversations.updated_at`を更新。
4. `runtime_runs`を`completed`へ更新し、failure codeをNULL、Supervisor metadataを保存。
5. update対象runtime rowが1件であることを確認。
6. commit。
7. commit後に`ConversationMessage`をreturn。

failure/cancel transaction:

1. thread idが確定済みなら`codex_threads`をupsert。
2. `runtime_runs`を`failed`または`cancelled`へ更新。
3. failure code、redacted error、Supervisor metadata、completed_atを保存。
4. update対象が1件であることを確認。
5. commit。
6. commit後にtyped failureをreturn。

`persist_codex_thread`と`persist_assistant_message`を順番に個別commitする実装は使わない。新しいtransaction helperを作る。

### 7.3 Tauri terminal event

Codex route:

- success commit後に`RuntimeEvent::MessageCompleted`
- cancel commit後に`RuntimeEvent::Cancelled`
- failure commit後に`RuntimeEvent::Failed`

terminal eventはrunごとに1件だけ送る。`start_turn`にあるgeneric `runtime_error`の送信を削除し、`execute_turn`が全terminal eventを送る。`prepare_runtime_run`自体が失敗した場合も、`execute_turn`がredacted `runtime_error`を1件送ってからerrorを返す。この場合はruntime rowが作成されていないため永続化は行わない。

conversation routeはSupervisor対象外であり、既存streaming/persistence順序を変更しない。conversation failure/cancel eventも既存`execute_turn`内の処理を維持する。

frontend-onlyなrun status storeは追加しない。UI eventはcommit済みSQLite stateの通知であり、永続source of truthは`runtime_runs`である。

finalize transactionが失敗した場合は`MessageCompleted`を送らない。別transactionで同じruntime rowをFailed/`internal-error`へ確定できた場合だけ`Failed` eventを送る。fallback transactionも失敗した場合はterminal eventを送らずcommand errorを返し、rowは次回起動時の`app-restarted` reconcile対象にする。Channel send失敗はcommit済みDBを巻き戻さず、event retryもしない。

### 7.4 failure codeとrecovery

| code | status | 固定recovery |
|---|---|---|
| `configuration-error` | failed | Check the Codex workspace and Settings, then retry. |
| `child-start-failed` | failed | Check the Codex installation and bundled runtime, then retry. |
| `request-timeout` | failed | Restart the Codex runtime and retry. |
| `progress-timeout` | failed | Retry the run. If the task is expected to be long, review the coding timeout. |
| `terminal-timeout` | failed | Restart the Codex runtime and retry. |
| `hard-timeout` | failed | Increase the coding timeout only if the task requires it, then retry. |
| `child-exited` | failed | Check the Codex runtime status and retry. |
| `protocol-error` | failed | Update or reinstall the Codex runtime, then retry. |
| `policy-violation` | failed | This read-only route cannot perform the requested operation. |
| `provider-error` | failed | Review the bounded error and retry. |
| `response-too-large` | failed | Ask for a shorter response and retry. |
| `internal-error` | failed | Restart SAAA and retry. |
| `user-cancelled` | cancelled | なし。Failed eventを出さない |
| `app-restarted` | interrupted | 次回起動時eventを出さない |

非Codex routeの既存`runtime_error`はTypeScript unionへ残す。

## 8. Input Activity Signal — Rust contract

### 8.1 DTO

`situation/contracts.rs`へ追加する。

```rust
#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum InputActivityState {
    Active,
    Recent,
    Idle,
    Unknown,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct InputActivitySignal {
    pub state: InputActivityState,
    pub health: SignalHealth,
}

pub fn unsupported_input_activity() -> InputActivitySignal {
    InputActivitySignal {
        state: InputActivityState::Unknown,
        health: SignalHealth::Unsupported,
    }
}
```

`SignalSnapshot`へ次を追加する。

```rust
#[serde(default = "unsupported_input_activity")]
pub input_activity: InputActivitySignal,
```

field位置は`foreground`の直後とする。JSON名は`inputActivity`。

`initial_signals`ではdefaultを使わず、monitoring停止を表す次を明示する。

```rust
InputActivitySignal {
    state: InputActivityState::Unknown,
    health: SignalHealth::Disabled,
}
```

### 8.2 CalibrationParameters

既存struct末尾へ追加する。

```rust
#[serde(default = "default_input_active_max_ms")]
pub input_active_max_ms: u64,

#[serde(default = "default_input_recent_max_ms")]
pub input_recent_max_ms: u64,
```

default function:

```rust
const fn default_input_active_max_ms() -> u64 { 30_000 }
const fn default_input_recent_max_ms() -> u64 { 300_000 }
```

validation:

```text
5_000 <= inputActiveMaxMs <= 120_000
60_000 <= inputRecentMaxMs <= 1_800_000
inputActiveMaxMs < inputRecentMaxMs
```

既存profile JSONはfield欠落をserde defaultで読む。profile row、rule version、status、parameters JSONをmigrationで書き換えない。

### 8.3 macOS dependency

`src-tauri/Cargo.toml`のmacOS target dependencyへ次を追加する。

```toml
objc2-core-graphics = { version = "0.3.2", default-features = false, features = ["std", "CGEventSource", "CGEventTypes"] }
```

既存`objc2-app-kit 0.3.2`とversionを合わせる。

Appleは`CGEventSourceSecondsSinceLastEventType`に`kCGAnyInputEventType`を渡すとkeyboard、mouse、tabletを含む直前のinput eventからの経過秒数を返すと定義している。実装はこのAPIだけを使い、event tapを作らない。[Apple API](https://developer.apple.com/documentation/coregraphics/cgeventsource/secondssincelasteventtype%28_%3Aeventtype%3A%29?language=objc)

`objc2-core-graphics 0.3.2`には`CGEventSource::seconds_since_last_event_type`があり、`CGEventSource`と`CGEventTypes` featureで利用できる。[Rust API](https://docs.rs/objc2-core-graphics/0.3.2/objc2_core_graphics/struct.CGEventSource.html#method.seconds_since_last_event_type)

### 8.4 macOS実装

新しいfileを増やさず、`situation/platform/macos.rs`へ追加する。

```rust
use super::{CalibrationParameters, InputActivitySignal};
use objc2_core_graphics::{CGEventSource, CGEventSourceStateID, CGEventType};

pub fn input_activity_signal(parameters: &CalibrationParameters) -> InputActivitySignal {
    let seconds = CGEventSource::seconds_since_last_event_type(
        CGEventSourceStateID::CombinedSessionState,
        CGEventType(u32::MAX), // kCGAnyInputEventType
    );
    super::classify_input_activity_seconds(seconds, parameters)
}
```

`classify_input_activity_seconds`は`platform/mod.rs`のpure functionとし、visibilityを`pub(super)`にせずprivateのままにする。child moduleの`macos.rs`から`super::classify_input_activity_seconds`で参照し、`platform` module外へ公開しない。

```rust
fn classify_input_activity_seconds(
    seconds: f64,
    parameters: &CalibrationParameters,
) -> InputActivitySignal
```

処理順:

1. `seconds.is_finite()`を確認。
2. `seconds >= 0.0`を確認。
3. `seconds <= u64::MAX as f64 / 1000.0`を確認。
4. invalidなら`Unknown + Degraded`。
5. `(seconds * 1000.0).floor() as u64`でms化。
6. Section 8.6の境界でcategory化。
7. validならhealthはReady。

raw `f64`はこのfunction callからreturnしない。raw observation用structは作らない。`Serialize`可能な型、error message、log、eventへraw値を入れない。

### 8.5 unsupported実装

`situation/platform/unsupported.rs`へ追加する。

```rust
pub fn input_activity_signal(_: &CalibrationParameters) -> InputActivitySignal {
    unsupported_input_activity()
}
```

`platform/mod.rs`はcfgでmacOS/unsupportedへdispatchする。

```rust
pub fn input_activity_signal(
    parameters: &CalibrationParameters,
) -> InputActivitySignal
```

### 8.6 category境界

| elapsed ms | state |
|---|---|
| `0 <= elapsed <= inputActiveMaxMs` | active |
| `inputActiveMaxMs < elapsed <= inputRecentMaxMs` | recent |
| `inputRecentMaxMs < elapsed` | idle |

境界値はinclusive/exclusiveを変更しない。roundではなくfloorを使う。

### 8.7 health mapping

| condition | state | health |
|---|---|---|
| macOS APIがfinite non-negativeを返す | thresholdで分類 | ready |
| NaN、Infinity、負値、overflow | unknown | degraded |
| non-macOS | unknown | unsupported |
| monitoring disabledのinitial snapshot | unknown | disabled |

Input Activityでは`permission-denied`を生成しない。使用APIからpermission拒否を判別するcontractがないためである。実機で予期しないpermission promptが出た場合はdegradedへ推測せずStop conditionとする。

## 9. Situation integration

### 9.1 sampling

`SituationSample`へ追加する。

```rust
input_activity: InputActivitySignal,
```

`situation/mod.rs`の`contracts` importへ`ForegroundCategory`、`ForegroundSignal`、`InputActivitySignal`、`InputActivityState`、`TimeBucket`を追加する。以下のdisabled returnが未修飾名でcompileできる状態にする。

`sample_platform`は一度のlockで次をcloneする。

```rust
let (enabled, calendar_enabled, parameters) = {
    let inner = self.inner.lock()?;
    (
        inner.settings.enabled,
        inner.settings.calendar_enabled,
        inner.calibration_parameters.clone(),
    )
};
```

`enabled == false`ならplatform APIを呼ばず、次をreturnする。worker loopから通常このpathへ来ないが、直接呼出しでもOS APIを呼ばないこと。

```rust
return Ok(SituationSample {
    foreground: ForegroundSignal {
        category: ForegroundCategory::Unknown,
        health: SignalHealth::Disabled,
    },
    input_activity: InputActivitySignal {
        state: InputActivityState::Unknown,
        health: SignalHealth::Disabled,
    },
    calendar: CalendarSignal {
        state: CalendarState::Unavailable,
        time_bucket: TimeBucket::None,
        health: SignalHealth::Disabled,
    },
    observed_at: crate::now_iso(),
    observed_ms: epoch_millis(),
});
```

enabled時だけ次を取得する。

```rust
input_activity: platform::input_activity_signal(&parameters)
```

`tick_sampled`はsample値を`SignalSnapshot.input_activity`へコピーする。SQLiteへraw snapshotを追加しない。

### 9.2 signal health

`signal_health()`へ次を追加する。

```rust
SignalHealthEntry {
    source: "input-activity".to_string(),
    health: signals.input_activity.health.clone(),
}
```

既存repositoryの最大8 entries制約内である。health変化eventは他signalと同じ差分検出を使い、重複発行しない。

### 9.3 classifier

`classify_with_parameters`のscore、scene、confidence、evidence、user_attention、audio environmentをInput Activityで変更しない。Input Activity参照をclassifierのscore計算へ追加しない。

これにより同じ既存signalでInput Activityだけを変えた場合、`Candidate`は完全一致する。

### 9.4 Shadow policy

signatureを次へ変更する。

```rust
pub fn shadow_policy(
    state: &SituationState,
    signals: &SignalSnapshot,
    parameters: &CalibrationParameters,
    now: &str,
) -> ShadowDecision
```

既存hardcode `45`と`70`は、それぞれ`parameters.low_confidence_max`と`parameters.classification_min_confidence`へ置換する。全caller、replay、testを更新する。

policy priority:

```text
1. foreground = Sensitive
   -> IGNORE / sensitive-safe-default

2. conversation != Idle
   または microphone = SaaaCapturing | SaaaTranscribing
   -> RESPOND / explicit-saaa-interaction

3. scene = UNKNOWN
   または confidence < lowConfidenceMax
   -> IGNORE / insufficient-signal

4. inputActivity.health = Ready
   かつ inputActivity.state = Idle
   -> OBSERVE / input-idle

5. scene = MEETING
   または userAttention = busy
   -> OBSERVE / user-busy

6. confidence >= classificationMinConfidence
   かつ userAttention = available
   -> SUGGEST / high-confidence-available

7. その他
   -> OBSERVE / passive-observation
```

`POLICY_VERSION`を`mvp2.5-shadow-v1`へ変更する。`RULE_VERSION`とactive profileのrule versionは変更しない。

重要な結果:

- idleでもexplicit SAAA interactionはRESPOND。
- sensitiveは常にIGNORE。
- unknown/degraded/unsupported/disabledはInput Activity導入前のpolicyへfall back。
- idleはstateやledger evidenceを変更せず、decision reasonだけを`input-idle`にする。
- Situationからmodel、TTS、notification、Meeting、application actionを開始しない。

## 10. SQLite and settings migration v7

### 10.1 `runtime_runs` schema

新規DBのCREATE TABLEへ次を直接含める。

```sql
failure_code TEXT CHECK(failure_code IS NULL OR failure_code IN (
  'user-cancelled','app-restarted','configuration-error','child-start-failed',
  'request-timeout','progress-timeout','terminal-timeout','hard-timeout',
  'child-exited','protocol-error','policy-violation','provider-error',
  'response-too-large','internal-error'
)),
supervisor_version TEXT CHECK(
  supervisor_version IS NULL OR length(supervisor_version) BETWEEN 1 AND 64
),
last_progress_at TEXT CHECK(
  last_progress_at IS NULL OR length(last_progress_at) BETWEEN 1 AND 32
),
```

既存status CHECKは変更しない。

### 10.2 migration function

`migrate_v6_to_v7(connection: &Connection)`を追加する。

処理:

1. `PRAGMA user_version`が7以上ならreturn。
2. `pragma_table_info('runtime_runs')`で各column存在を確認。
3. 欠けているcolumnだけ`ALTER TABLE ... ADD COLUMN`する。
4. calibration profile rowは変更しない。
5. `settings_documents.schema_version < 7`を7へ更新。
6. caller transaction内で`PRAGMA user_version = 7`。

同じmigrationを2回呼んでもcolumn追加やdata変更が起きないこと。

column追加SQLは次で固定する。SQLiteでは後付けCHECKをtable-levelへ追加しない。

```sql
ALTER TABLE runtime_runs ADD COLUMN failure_code TEXT
  CHECK(failure_code IS NULL OR failure_code IN (
    'user-cancelled','app-restarted','configuration-error','child-start-failed',
    'request-timeout','progress-timeout','terminal-timeout','hard-timeout',
    'child-exited','protocol-error','policy-violation','provider-error',
    'response-too-large','internal-error'
  ));
ALTER TABLE runtime_runs ADD COLUMN supervisor_version TEXT
  CHECK(supervisor_version IS NULL OR length(supervisor_version) BETWEEN 1 AND 64);
ALTER TABLE runtime_runs ADD COLUMN last_progress_at TEXT
  CHECK(last_progress_at IS NULL OR length(last_progress_at) BETWEEN 1 AND 32);
```

### 10.3 initialize order

`initialize_database`のtransaction順序を次へ固定する。

```text
CREATE TABLE IF NOT EXISTS（v7形状）
  -> migrate_legacy_settings_documents
  -> migrate_v4_to_v5
  -> migrate_v6_to_v7
  -> default settings INSERT OR IGNORE（schema 7）
  -> reconcile_interrupted_runs
  -> meeting::reconcile
  -> PRAGMA user_version = 7
  -> commit
```

`backup_before_migration`は`version >= 7`のときだけskipする。v6実データはmigration前に一度backupされる。

### 10.4 coding run write

`prepare_runtime_run`でcoding routeをinsertするとき、最初から次を保存する。

```text
supervisor_version = mvp2.5-supervisor-v1
failure_code = NULL
last_progress_at = NULL
```

conversation routeとvoice routeは3 fieldをNULLにする。

terminal finalizeでのみ`last_progress_at`を一度保存する。deltaごとにSQLite UPDATEしない。

### 10.5 restart reconcile

`runtime_runs.status = running`の全rowを既存どおり`interrupted`へする。同時に:

```text
failure_code = app-restarted
completed_at = now
error_message = COALESCE(error_message, 'Application restarted')
```

既存`supervisor_version`は変更しない。v6からmigrationした古いrunning rowはNULLのままでよい。

### 10.6 settings versionの更新箇所

全て6から7へ同じcommitで更新する。

```text
src-tauri/src/lib.rs
  default_settings_documents
  validate_settings_document
  initialize_database settings UPDATE
  backup_before_migration threshold

src/lib/contracts.ts
  SettingsDocument.schemaVersion

src/lib/schemas.ts
  z.literal(7)

src/features/settings/SettingsPage.tsx
  save payload schemaVersion

tests/settings.test.ts
  fixture schemaVersion
```

settings JSON shapeは変更しない。

## 11. Frontend contract

### 11.1 runtime failure type

`src/lib/contracts.ts`へ追加する。

```ts
export type RuntimeFailureCode =
  | "runtime_error"
  | "configuration-error"
  | "child-start-failed"
  | "request-timeout"
  | "progress-timeout"
  | "terminal-timeout"
  | "hard-timeout"
  | "child-exited"
  | "protocol-error"
  | "policy-violation"
  | "provider-error"
  | "response-too-large"
  | "internal-error";
```

`RuntimeEvent`のfailed codeを`RuntimeFailureCode`へ変更する。cancelledは別eventなので`user-cancelled`をunionへ含めない。frontendでrecoveryを再生成せず、backendの固定`message`と`recovery`を現行どおり表示する。

`src/lib/runtime.ts`のinvoke signatureは変わらない。

### 11.2 Input Activity type

```ts
export type InputActivityState = "active" | "recent" | "idle" | "unknown";

export type SignalSnapshot = {
  // existing
  inputActivity: {
    state: InputActivityState;
    health: SignalHealth;
  };
};
```

`as any`、optional field、frontend defaultは使わない。Rust snapshotが常にfieldを返す。

### 11.3 Situation Overview

`SituationPage.tsx`のSignal health gridでForeground直後に追加する。

```tsx
<Signal
  label="Input activity"
  value={`${snapshot.signals.inputActivity.state} · ${snapshot.signals.inputActivity.health}`}
/>
```

privacy helpを次へ更新する。

```text
Raw application identity、window title、Calendar details、audio content、exact input idle timeは保存しません。
```

exact seconds、last input time、keyboard/mouse内訳を表示しない。

### 11.4 Calibration candidate editor

`CalibrationParameters`へ追加する。

```ts
inputActiveMaxMs: number;
inputRecentMaxMs: number;
```

`defaultParameters`:

```text
inputActiveMaxMs = 30000
inputRecentMaxMs = 300000
```

既存form末尾へ追加する。

| label | min | max | step |
|---|---:|---:|---:|
| Input active max (ms) | 5000 | 120000 | 5000 |
| Input recent max (ms) | 60000 | 1800000 | 30000 |

`parametersValid`へ`inputActiveMaxMs < inputRecentMaxMs`を追加する。invalid時は次を表示する。

```text
Input active maximum must be lower than input recent maximum.
```

新しいSettings UIは作らない。変更は既存candidate creation flowだけである。

## 12. Replay fixture contract

### 12.1 fixture version

```rust
pub const FIXTURE_SET_VERSION: &str = "situation-fixtures-v2";
```

既存`mvp1-v1.json`は変更しない。`SignalSnapshot.input_activity`のserde defaultにより`Unknown + Unsupported`としてdecodeする。

新規`mvp2.5-v2.json`は次のshapeとする。

```json
{
  "version": "situation-fixtures-v2",
  "scenarios": [
    {
      "id": "explicit-input-wins-idle",
      "samples": [
        {
          "elapsedMs": 0,
          "signals": {
            "sequence": 1,
            "observedAt": "0",
            "foreground": { "category": "coding", "health": "ready" },
            "conversation": { "state": "user-input" },
            "microphone": { "state": "inactive", "health": "ready" },
            "audio": { "state": "silent", "health": "ready" },
            "calendar": { "state": "free", "timeBucket": "none", "health": "ready" },
            "inputActivity": { "state": "idle", "health": "ready" }
          },
          "expectedScene": "CONVERSATION",
          "expectedAttention": "RESPOND"
        }
      ]
    }
  ]
}
```

実際の`signals`は全fieldを省略せず記述する。Input Activityにraw secondsを入れない。

### 12.2 必須scenario

| scenario id | 内容 | 必須期待値 |
|---|---|---|
| `coding-active-to-idle` | sample 1〜3をactive codingで安定化し、sample 4〜6をactive→recent→idle | sample 3〜6はscene CODING。sample 4〜6はSUGGEST→SUGGEST→OBSERVE |
| `explicit-input-wins-idle` | conversation UserInput + input idle | CONVERSATION / RESPOND |
| `sensitive-wins-idle` | Sensitive foreground + input idle | IGNORE |
| `unsupported-parity` | input Unknown/Unsupportedで既存coding sample | MVP 1 baselineと同じscene/attention |
| `degraded-parity` | input Unknown/Degraded | Input Activityを無視 |

各scenarioで`elapsedMs`は0から単調増加し、scenario間でHysteresisをresetする。

### 12.3 replay code

`calibration.rs`にv2用structを追加する。

```rust
struct ReplayFixtureSet {
    version: String,
    scenarios: Vec<ReplayScenario>,
}

struct ReplayScenario {
    id: String,
    samples: Vec<ReplaySampleV2>,
}

struct ReplaySampleV2 {
    elapsed_ms: u64,
    signals: SignalSnapshot,
    expected_scene: SituationScene,
    expected_attention: String,
}
```

全structに`rename_all = camelCase`と`deny_unknown_fields`を付ける。scenario idは既存identifier rule、最大160文字で検証する。scenario数1〜20、各sample 1〜14,400、合計14,400以下。

`expectedAttention`は`IGNORE | OBSERVE | SUGGEST | RESPOND`の4値だけを受理し、それ以外はfixture decode後のvalidation errorにする。v2 fixtureの各`signals`は全field必須であり、serde defaultへ依存させない。

`replay_metrics`は:

1. 既存v1 fixtureを従来どおりreplayする。
2. v2の各scenarioをfresh Hysteresisでreplayする。
3. candidate parametersを`classify_with_parameters`と`shadow_policy(..., parameters, ...)`へ渡す。
4. default parametersでも同じ2 fixture群をreplayする。
5. 次のmetricsを返す。

```json
{
  "fixtureSetVersion": "situation-fixtures-v2",
  "profileRuleVersion": "...",
  "sampleCount": 0,
  "expectedSceneMatches": 0,
  "baselineExpectedSceneMatches": 0,
  "expectedAttentionSamples": 0,
  "expectedAttentionMatches": 0,
  "baselineExpectedAttentionMatches": 0,
  "shadowPolicyCounts": {},
  "deterministic": true
}
```

frontendの`ReplayMetrics`とdecoderへ3つのattention fieldを追加し、Latest replayへ`Attention matches`を表示する。

metric semanticsは次で固定する。

- `sampleCount`: v1とv2の全sample合計。
- `expectedSceneMatches`: candidate parametersでv1/v2の`expectedScene`に一致した数。
- `baselineExpectedSceneMatches`: default parametersで同じ期待値に一致した数。
- `expectedAttentionSamples`: `expectedAttention`を持つv2 sample数。v1は数えない。
- `expectedAttentionMatches`: candidate parametersのShadow decisionがv2期待値に一致した数。
- `baselineExpectedAttentionMatches`: default parametersで同じ期待値に一致した数。
- `shadowPolicyCounts`: candidate parametersで全v1/v2 sampleを評価した`IGNORE/OBSERVE/SUGGEST/RESPOND`別件数。4 keyを0件でも出す。
- `deterministic`: 同じfixture/profileをfresh Hysteresisで2回実行し、scene、attention、全countが完全一致した場合だけ`true`。

## 13. Privacy and diagnostics

M25-00で[`adr/0002-situation-signal-privacy.md`](./adr/0002-situation-signal-privacy.md)を継承する`0003-input-activity-signal-privacy.md`を作る。

保存・表示してよいもの:

- Input Activity category
- Input Activity health
- bounded reason code `input-idle`
- Supervisor version
- failure code
- meaningful progressの最終timestamp

禁止するもの:

- exact idle seconds/milliseconds
- last input timestamp
- key/button/cursor/window title
- raw `CGEvent`
- raw app-server event
- Supervisor用に複製したprompt、assistant text、command、tool args

assistant message本文は既存conversation messageとしてのみ保存する。Supervisor diagnosticsへ複製しない。

`export_diagnostics`は現在のbounded counts/statusだけを維持する。Input Activity exact値やraw app-server dataを追加しない。

privacy verification:

```bash
rg -n "seconds_since_last_event|last_input|idle_seconds|RawInputActivity|raw.*event" src src-tauri
rg -n "println!|dbg!|tracing::|log::" src-tauri/src
```

検索hitはmacOS adapterのAPI call、pure mapper、testだけであること。serializer、repository、diagnostics、UIにhitしたら受け入れ不可。

## 14. 実装ticket

ticketは番号順に実装する。各ticketの完了時に、そのticketで追加・変更した自動testを通してから次へ進む。

| Ticket | Depends on | 主な完了物 |
|---|---|---|
| M25-00 | MVP 2 baseline | spike、privacy ADR、baseline記録 |
| M25-01 | M25-00 | pure Supervisor |
| M25-02 | M25-01 | Codex request/event adapter |
| M25-03 | M25-02 | cancel/cleanup統合 |
| M25-04 | M25-03 | v7 persistence、terminal ordering |
| M25-05 | M25-00 | Input Activity DTO/macOS adapter |
| M25-06 | M25-05 | Shadow policy統合 |
| M25-07 | M25-06 | calibration/replay |
| M25-08 | M25-04、M25-07 | frontend contract/UI |
| M25-09 | M25-08 | integration、manual、soak |
| M25-10 | M25-09 | release evidence |

### M25-00 — Baseline, macOS spike, privacy ADR

変更:

- baseline commandを記録
- `mktemp -d`配下の破棄可能な最小Rust crateでdependencyとAPI symbolが現在のmacOS host targetでcompileすることを確認
- spikeには実行command、macOS version、CPU architecture、結果を記録
- spike/ADR作成

完了条件:

- dependencyとAPI signatureがSection 8.3/8.4どおりcompileする
- 計画変更を要する結果がない
- repositoryへspike用source/binaryを残さない
- ADR Status Proposed。実機確認後のAccepted化はM25-05で行う

### M25-01 — Pure Supervisor

変更:

- `mod run_supervisor;`
- Section 6.2〜6.6の型、method、state machine
- sleep無しtable-driven test

完了条件:

- 全遷移とwatch precedenceをmonotonic integerだけでtest
- terminal idempotency
- one-shot interrupt

### M25-02 — Codex request wait and event projection

変更:

- supervised request wait
- notification correlation
- active item set
- meaningful progress/forbidden item mapping

完了条件:

- foreign/unscoped/duplicate eventがwatchを延命しない
- request wait中のCancelが20秒を待たず終了
- read-only policy test維持

### M25-03 — Cancellation and process cleanup

変更:

- Codex loopへSupervisor effect適用
- interrupt id=4 one-shot
- 3秒grace
- `ProcessGuard::terminate`
- stdout reader error forwarding/join

完了条件:

- Cancel連打でもinterrupt 1回
- unresponsive childがgrace後にreap
- reader threadが残らない

### M25-04 — v7 persistence and terminal ordering

変更:

- runtime columns/migration/settings version
- Codex success/failure transaction helper
- terminal event ordering
- restart reconcile

完了条件:

- DB commit後にterminal event
- failure code表どおり
- v6 backup/migration/reopen成功

### M25-05 — Input Activity DTO/platform integration

変更:

- DTO、serde default、calibration field
- Cargo dependency
- macOS/unsupported adapter
- Situation sample/health

完了条件:

- boundary/invalid/unsupported/disabled test
- raw durationがplatform module外へ出ない
- development/packaged buildでSection 16.1を実施
- permission promptが出ず、ADR StatusをAcceptedへ変更

### M25-06 — Shadow policy integration

変更:

- parameterized `shadow_policy`
- idle suppression
- policy version
- classifier parity test

完了条件:

- scene/state/evidence parity
- explicit/sensitive precedence
- non-ready fallback

### M25-07 — Calibration/replay

変更:

- frontend/Rust parameter fields
- candidate validation
- v1 compatibility
- v2 fixture/scenario replay
- metrics decoder

完了条件:

- old profile JSON decode成功
- active profile row不変
- v1 baseline parity
- v2 attention expectations一致

### M25-08 — Frontend UI/contracts

変更:

- RuntimeFailureCode union
- Input Activity Signal表示
- calibration editor field
- privacy text

完了条件:

- `as any`なし
- exact duration表示なし
- unsupported/degraded表示可能

### M25-09 — Integration and soak

変更:

- fake app-server scenario拡張
- desktop smoke拡張
- 30分soak
- privacy search

完了条件:

- process/thread/task leakなし
- SQLite rowがrunningのまま残らない
- Situation monitor offでAPI callなし

### M25-10 — Release evidence

変更:

- release evidence作成
- 本文StatusをCompleteへ変更
- manual/unsupported/未完了item記録

完了条件:

- 全acceptance criteriaへ証跡linkあり
- MVP 2 manual evidenceを隠していない

## 15. Test plan

### 15.1 Supervisor unit tests

production sleepを使わず、次のtest名で実装する。

```text
request_deadline_is_twenty_seconds
request_completion_clears_deadline
cancel_before_turn_id_finishes_without_interrupt
cancel_after_turn_id_emits_one_interrupt
duplicate_cancel_is_noop
nonempty_delta_resets_progress_deadline
empty_or_foreign_event_does_not_reset_progress
assistant_completion_arms_terminal_gap
new_item_after_assistant_completion_returns_to_running
terminal_gap_timeout_preserves_failure_code
hard_timeout_precedes_terminal_and_progress_timeout
interrupt_grace_finishes_pending_outcome
late_terminal_after_finish_is_ignored
provider_completed_requires_nonempty_content
provider_interrupted_maps_to_cancel_only_after_user_cancel
```

### 15.2 fake app-server tests

既存fixture notificationへ`threadId`と`turnId`を追加する。policyをtest injectionできるprivate helperを作る。

```rust
fn run_codex_turn_process_with_policy(..., policy: RunSupervisionPolicy)
```

production wrapperだけがproduction policyを渡す。

既存`SAAA_CODEX_PATH`差替えtestを拡張する。環境変数はprocess-globalなので、test moduleに`static CODEX_FIXTURE_LOCK: Mutex<()>`を置き、lock取得から環境変数restoreまでを直列化する。restoreはDrop guardで行い、test panic時にも元の値へ戻す。fixture scriptは各testの`tempfile::TempDir`へ生成し、scenario名をscriptへ埋め込む。repositoryへfixture executableを追加しない。

pure Supervisor testは仮想時刻だけを使う。fake process testだけは次のpolicyを使い、production値を待たない。

```rust
RunSupervisionPolicy {
    request_timeout_ms: 1_000,
    progress_idle_timeout_ms: 1_500,
    terminal_gap_timeout_ms: 750,
    interrupt_grace_ms: 500,
    hard_timeout_ms: 4_000,
}
```

scenario:

1. normal start/stream/terminal
2. resume
3. cancel responseあり
4. cancel無応答
5. thread response hang
6. turn response hang
7. progress hang
8. agentMessage完了後terminal無し
9. hard timeout
10. malformed JSON
11. approval request
12. forbidden fileChange/MCP/Web Search
13. stdout close/non-zero exit
14. foreign thread/turn notification
15. duplicate item start/completion
16. response 64,001 chars

process-level scenarioでは次をassertする。

- interrupt送信回数
- functionが期限内にreturnすること。return前にchild reapとstdout reader joinが完了するcontractである
- typed outcome/failure code
- raw payloadがerror/diagnosticsへ無い

M25-04ではnormal、cancel、progress timeout、policy violationの4 scenarioを`execute_turn`まで通すtemp SQLite integration testも追加し、terminal event 1件、runtime status/failure code、Supervisor version/last progress、event受信時点のDB stateをassertする。

### 15.3 Input Activity tests

```text
zero_is_active
active_max_is_active
active_max_plus_one_is_recent
recent_max_is_recent
recent_max_plus_one_is_idle
fractional_seconds_are_floored
nan_is_degraded_unknown
infinity_is_degraded_unknown
negative_is_degraded_unknown
overflow_is_degraded_unknown
unsupported_platform_is_unknown_unsupported
initial_disabled_signal_is_unknown_disabled
```

### 15.4 Situation tests

```text
input_activity_does_not_change_candidate
idle_ready_suppresses_suggest
idle_does_not_override_explicit_interaction
sensitive_precedes_input_activity
idle_unknown_scene_remains_ignore
degraded_activity_falls_back_to_existing_policy
unsupported_activity_falls_back_to_existing_policy
health_change_is_emitted_once
```

### 15.5 Migration tests

```text
clean_database_is_v7
v6_database_is_backed_up_before_v7
v6_runtime_rows_gain_nullable_columns
v7_migration_is_idempotent
settings_documents_are_v7_without_json_shape_change
running_rows_reconcile_to_interrupted_app_restarted
old_calibration_json_uses_input_activity_defaults
old_calibration_profile_row_is_not_rewritten
```

### 15.6 Frontend verification

新しいUI test frameworkは追加しない。既存Bun testとTypeScript compilerで自動検証し、renderingはdesktop smokeで確認する。

- `tests/settings.test.ts`: Settings schema 7を受理し6を拒否
- `bun run typecheck`: `RuntimeFailureCode`、必須`inputActivity`、candidate payloadを型検証
- `cargo test`: Rust serde testで旧/new snapshot、calibration parameters、replay attention metricsをdecode
- `bun run desktop:smoke`: Input Activity表示、candidate validation、exact duration非表示を確認

### 15.7 verification command

```bash
bun run typecheck
bun run test:frontend
cargo fmt --check --manifest-path src-tauri/Cargo.toml
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
cargo test --manifest-path src-tauri/Cargo.toml
bun run build
bun run desktop:smoke
```

## 16. Manual verification and Stop conditions

### 16.1 macOS Input Activity manual verification

development buildと署名済みpackaged buildの両方で実施する。

1. Situation monitoringをoffにし、Input Activity APIが呼ばれないことを確認。
2. monitoringをonにし、Input activityが表示されることを確認。
3. keyboard、mouse、trackpad操作後30秒以内にActiveになることを確認。
4. 30秒超〜5分以内にRecentになることを確認。
5. 5分超でIdleになることを確認。
6. screen lock、unlock、sleep、resume後にpanicしないことを確認。
7. Accessibility/Input Monitoring permission promptが出ないことを確認。
8. 30分samplingし、CPU/memoryが増加し続けないことを確認。
9. SQLite、diagnostics、consoleにexact durationが無いことを確認。

step 1/2ではDebuggerに`CGEventSourceSecondsSinceLastEventType`のsymbolic breakpointを設定する。monitoring offで10秒待ってhit 0回、onでsampling開始後にhitすることを証跡へ記録する。

### 16.2 Agent Run manual verification

1. normal coding turn。
2. long reasoning中のprogress継続。
3. Cancel一回と連打。
4. fixture child hang。
5. app close中のactive run。
6. restart後のinterrupted reconcile。
7. read-only policy violation。

### 16.3 Stop conditions

次の場合は実装を止め、この文書またはADRを改訂する。

- CoreGraphics callだけでAccessibility/Input Monitoring permission promptが出る
- `objc2-core-graphics 0.3.2`がshipping targetでlink/bundleできない
- raw durationを保存しないと要件を満たせない
- current notificationにthreadId/turnIdがなく安全にcorrelateできない
- `turn/completed`以外を成功terminalにする必要がある
- hard timeoutを超えるunbounded graceが必要
- Supervisor導入にCodex routeの権限拡大が必要
- Input Activityがscene score/state変更を必要とする
- Meeting/STT/TTSも同時移行しないとCodex Supervisorをshipできない
- v7 migrationが既存dataを保持できない

## 17. Acceptance criteria

### Agent Run Supervisor

1. thread/turn requestは各20秒以内にterminal failureとなる。
2. progress、terminal-gap、hard timeoutを異なるcodeで識別できる。
3. Cancel/timeout/policy violationのinterruptは最大1回。
4. interrupt無応答childは3秒grace後にkill/waitされる。
5. stdout reader threadがrun後に残らない。
6. foreign/duplicate/unscoped notificationはwatchを延命しない。
7. 成功はnon-empty contentを伴うcurrent `turn/completed: completed`だけ。
8. Codex terminal eventはSQLite commit後に1件だけ送られる。
9. restartでrunning rowがinterrupted/app-restartedになる。
10. prompt/tool args/raw provider eventがSupervisor persistenceへ追加されない。
11. 既存read-only security testが通る。

### Input Activity Signal

1. macOS adapterは固定dependency/APIで実装される。
2. valid observationは固定境界でactive/recent/idleへ分類される。
3. raw durationとlast input timestampはplatform adapter外へ出ない。
4. monitoring off中はOS APIを呼ばない。
5. Input Activityだけを変えてもCandidate/state/scene/confidence/evidence/userAttentionは変わらない。
6. idle Readyはexplicit interactionがない場合のSUGGESTだけをOBSERVEへ抑制する。
7. explicit interactionとSensitive safe defaultがidleより優先される。
8. non-ready healthはMVP 2 policyへfall backする。
9. idleを根拠に自動actionを開始しない。
10. v1 fixtureと旧profile JSONをdata rewriteなしで読める。

### Release quality

1. SQLite/settings schemaはv7で、v6 backup/migration/reopenが成功する。
2. 全verification commandが成功する。
3. fake app-server全scenarioと30分soakでleakがない。
4. privacy/manual packaged evidenceがrelease evidenceへ記録される。
5. MVP 2から引き継いだmanual evidenceの未完了項目が明記される。

## 18. MVP 2.5完了後の候補

次はMVP 2.5に含めない。

- Supervisorをconversation provider、voice、Meetingへ段階適用
- watch値の実測に基づく調整
- user-visible run diagnostics history
- Input Activity専用Settings UI
- Windows/Linux adapter
- Situation提案のfalse-positive分析
