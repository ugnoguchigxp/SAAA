# MVP 2 実装契約 — Explicit Meeting Mode

## 0. 文書の役割

- Status: Proposed
- 実装前提: [`mvp-1.5-implementation-plan.md`](./mvp-1.5-implementation-plan.md) の受け入れ完了
- 対象: macOS desktopを最初のshipping platformとする
- この文書の読者: 実装担当、レビュー担当、QA担当
- 完了の定義: `M2-00`〜`M2-11`を順に完了し、P0 gateで確定したshipping scopeの受け入れ条件を満たすこと

この文書では、未検証のsystem audioを前提に全体を組まない。まずmicrophone-onlyのMeeting Sessionを縦に完成させ、system audioは独立したfeasibility gateを通過した場合だけ同じlane contractへ追加する。

## 1. 今回達成すること

ユーザーが明示的に開始し、pause/resume/stopできるMeeting Sessionを追加する。会議中の音声はローカルで文字起こしし、non-modalなMeeting画面へpartial/final transcriptを反映する。終了後の既定動作はDiscardであり、ユーザーがSaveを押した場合だけfinal transcriptをSQLiteへ保存する。

固定する安全境界:

```text
automaticMeetingStart = false
autoJoin = false
rawAudioPersistence = false
cloudSttFallback = false
automaticTTS = false
automaticNotification = false
automaticAgentOrCodexRun = false
automaticApplicationAction = false
defaultTranscriptPersistence = discard
```

MVP 1.5のMeeting candidateは、開始画面へ参考情報として表示してよい。ただしcandidateの有無はsession stateを変更せず、Start操作を代行しない。

## 2. 現行コードのレビュー結果

| 現在の実装 | 制約・問題 | MVP 2での判断 |
|---|---|---|
| `App.tsx`が`getUserMedia`、`MediaRecorder`、Blob decode、STT、Chat送信を一括管理 | Meetingへ流用するとChatと長時間captureが密結合する | captureを`features/meeting/audio/`へ分離し、Chat push-to-talkは既存のまま維持 |
| `transcribe_audio`は最大5分の全samplesを一括受信 | 長時間会議、低遅延partial、bounded memoryに適さない | 共通のWhisper実行部を抽出し、Meetingは短いsegment単位でcommandを呼ぶ |
| `run_whisper_transcription`は一時WAVを`tempdir`へ書く | 「fileを一切作らない」という旧計画と矛盾 | MVP 2では暗号化永続保存ではなく一時fileを許容するが、成功/失敗/取消時の削除をtestする。完全in-memory化は別ADR |
| `VoiceEvent`はconversation/run中心 | session/lane/sequenceを識別できない | Meeting専用eventを追加し、VoiceEventを無理に拡張しない |
| `runtime_runs.route_kind`はCHECK制約 | meeting routeを追加するとtable recreationが必要 | Meeting segmentは`runtime_runs`へ入れず、`meeting_sessions`とin-memory jobで管理 |
| `AppState.active_runs`はrun cancellationを保持 | stop時のsegment取消に再利用可能だがsession全体の状態は表せない | 独立した`MeetingRuntime`を`AppState`へ追加する |
| TTSはMeeting状態を知らない | Meeting中にChatのautoSpeakが走り得る | `speak_text`の先頭でMeeting policy guardを必須化する |
| system audio用dependencyがCargoに無い | permission、capture、bundle、停止が未検証 | P0 spikeに隔離し、gate失敗時はmicrophone-onlyでship |
| main windowしか存在しない | 別overlay windowはTauri capabilityとlifecycleの追加が必要 | 最初はmain window内のMeeting surfaceをshipし、floating overlayはP1 gate通過後に追加 |

## 3. Shipping scopeの決め方

### 3.1 必ずshipするCore

- microphone lane 1本
- 明示Start、Pause、Resume、Stop
- bounded segment capture
- local Whisper transcript
- original transcriptのpartial表示とfinal確定
- input/provider/privacy health表示
- 終了後のSave/Discard
- Meeting中のTTS、notification、agent/application action抑止
- app終了、permission revoke、STT失敗時のcleanup

### 3.2 Gate通過時だけshipするOptional

- macOS system audio lane
- microphone + system audioの2 lane同時実行
- floating always-on-top overlay window
- local translation

Optionalが一つも通らなくても、Coreの受け入れ条件を満たせばMVP 2は完成とする。UIは未対応機能を壊れたtoggleとして出さず、`Unavailable in this build`と理由を表示する。

## 4. State machine

Rustの`MeetingRuntime`だけをsession stateの正とする。React stateやwindow visibilityを正にしない。

```rust
pub enum MeetingState {
    Idle,
    Preflight,
    Ready,
    Active,
    Paused,
    Stopping,
    Completed,
    Failed,
}
```

許可遷移:

| From | Command/Event | To | 必須処理 |
|---|---|---|---|
| Idle | `meeting_preflight` | Preflight→Ready | provider/model/device/permissionを検査。captureしない |
| Ready | `start_meeting` | Active | session生成、capture token発行、policy lock取得 |
| Active | `append_meeting_audio_segment` | Active | segmentをSTTへ渡しeventを返す |
| Active | `pause_meeting` | Paused | capture token無効化、active segment取消、buffer破棄 |
| Paused | `resume_meeting` | Active | 新capture tokenを発行 |
| Active/Paused | `stop_meeting` | Stopping→Completed | segment取消、final確定、resource解放 |
| Completed | `save_meeting_transcript` | Idle | finalだけtransaction保存、session memory破棄 |
| Completed | `discard_meeting` | Idle | transcript memory破棄、DBへ本文を書かない |
| 非Idle | unrecoverable error | Failed | capture停止、token無効化、bounded error保持 |
| Failed | `discard_meeting` | Idle | resourceとmemoryを破棄 |

禁止遷移は`MEETING_INVALID_STATE`を返し、現在stateを変えない。`pause`、`stop`、`discard`は同じsession idで再送されても安全なidempotent操作にする。

### 4.1 app終了と再起動

- Active/Paused/StoppingのsessionはSQLiteへ本文保存しないため復元しない。
- app close hookで`MeetingRuntime::shutdown()`を呼び、capture tokenとworkerを停止する。
- `meeting_sessions`にmetadataを作成済みなら`interrupted`として終了時刻を記録する。ただしtranscript entryは明示Save前には存在してはならない。
- 次回起動時に`active/paused/stopping` metadataが残っていれば`interrupted`へreconcileする。

## 5. モジュールと変更ファイル

### 5.1 Rust

```text
src-tauri/src/meeting/
├── mod.rs                    # MeetingRuntimeとuse case
├── contracts.rs              # command input/output/event/error code
├── state_machine.rs          # 遷移表。DB/Tauri非依存
├── repository.rs             # metadataと明示保存transcript
├── policy.rs                 # TTS/agent/action guard
└── stt/
    ├── mod.rs                # MeetingStt traitとsegment validation
    └── local_whisper.rs      # 既存Whisper coreのadapter

src-tauri/src/situation/
└── ...                       # owned microphone stateの更新だけ。Meetingロジックを入れない

src-tauri/src/lib.rs          # AppState、command、migration v6、handler登録
```

`run_whisper_transcription`、`write_whisper_wav`、`resample_pcm`、`whisper_transcript_line`はMeeting固有moduleへコピーしない。`src-tauri/src/voice/local_whisper.rs`へ共通coreとして抽出し、既存`transcribe_audio`とMeeting adapterの両方から呼ぶ。抽出後も既存Chat voice testの期待値を変えない。

### 5.2 Frontend

```text
src/features/meeting/
├── MeetingPage.tsx
├── MeetingPreflight.tsx
├── MeetingSession.tsx
├── MeetingCompleted.tsx
├── useMeetingSession.ts
└── audio/
    ├── microphoneCapture.ts
    ├── pcm.ts
    └── segmentQueue.ts

src/lib/contracts.ts
src/lib/runtime.ts
src/lib/schemas.ts
src/App.tsx
src/App.css
tests/meeting-contracts.test.ts
tests/meeting-segment-queue.test.ts
```

`App.tsx`の`Surface`へ`meeting`を追加し、primary menuに`Meeting`を追加する。Meetingのcapture stateを`App.tsx`へ持ち上げない。

## 6. Command/DTO contract

### 6.1 Preflight

```rust
pub struct MeetingPreflightInput {
    pub microphone_device_id: String, // max 256; "default"可
    pub system_audio_enabled: bool,
    pub stt_model_path: String,        // canonicalizeしfileを検証
    pub translation_enabled: bool,
}

pub struct MeetingPreflightResult {
    pub state: MeetingState,           // Readyのみ成功
    pub microphone: LaneHealth,
    pub system_audio: LaneHealth,
    pub stt: ProviderHealth,
    pub translation: ProviderHealth,
    pub shipping_capabilities: MeetingCapabilities,
    pub blocking_errors: Vec<MeetingError>,
}
```

Frontendは`navigator.mediaDevices.getUserMedia`を実行してpermission/deviceを確認し、取得した全trackを結果確定直後に`stop()`してから`meeting_preflight`へ渡す。preflight用streamをcomponent stateへ残さない。Rust側はFrontend申告だけを信用せず、model pathとbuild capabilityを検証する。preflight中にMediaRecorderやAudioWorkletを開始しない。

### 6.2 Start

```rust
pub struct StartMeetingInput {
    pub session_id: String,
    pub microphone_device_id: String,
    pub microphone_enabled: bool,
    pub system_audio_enabled: bool,
    pub stt_model_path: String,
    pub translation_enabled: bool,
    pub persistence_mode: PersistenceMode, // MVP 2開始時はDiscardのみ受理
}
```

Start成功時に返す`capture_token`はランダム128bit相当のopaque Stringとする。audio segment commandはsession idとtokenの両方を必要とし、pause/stopでtokenを即時無効化する。tokenをSQLite、log、diagnosticsへ書かない。

### 6.3 Segment

```rust
pub struct MeetingAudioSegmentInput {
    pub session_id: String,
    pub capture_token: String,
    pub lane: MeetingLane,           // Microphone | SystemAudio
    pub sequence: u64,
    pub samples: Vec<f32>,
    pub sample_rate: u32,
    pub started_at_ms: u64,          // session相対時刻
    pub duration_ms: u32,
}
```

Server-side bounds:

```text
sampleRate: 8_000..=96_000
durationMs: 1_000..=15_000
samples: <= sampleRate * 15
sequence: laneごとに厳密単調増加
queued segments: laneごとに最大2
session transcript chars in memory: 最大200_000
single final segment text: 最大8_000 chars
```

queueが上限なら古いsegmentを黙って捨てない。`MEETING_BACKPRESSURE`を返し、Frontendはcaptureをpauseして復旧案を表示する。

### 6.4 Event

```rust
#[serde(tag = "type", rename_all = "camelCase")]
pub enum MeetingEvent {
    StateChanged { session_id: String, state: MeetingState },
    TranscriptPartial { session_id: String, lane: MeetingLane, sequence: u64, text: String },
    TranscriptFinal { session_id: String, lane: MeetingLane, sequence: u64, text: String, language: Option<String> },
    TranslationFinal { session_id: String, source_sequence: u64, text: String, language: String },
    LaneHealthChanged { session_id: String, lane: MeetingLane, health: LaneHealth },
    Failed { session_id: String, code: String, message: String, recovery: String },
}
```

`watch_meeting`は既存`watch_situation`と同じChannel patternを使うが、event queueは最大128件。partialは同じlane/sequenceの最新値で置換し、final/state/errorは捨てない。finalでqueueが満杯ならsessionをpauseし`MEETING_BACKPRESSURE`を送る。

### 6.5 Command一覧

| Command | Output | Notes |
|---|---|---|
| `meeting_preflight` | `MeetingPreflightResult` | captureなし |
| `start_meeting` | `MeetingSnapshot` | Readyからのみ |
| `append_meeting_audio_segment` | `MeetingSegmentResult` | `spawn_blocking`でlocal STT |
| `pause_meeting` | `MeetingSnapshot` | idempotent |
| `resume_meeting` | `MeetingSnapshot` | 新tokenを返す |
| `stop_meeting` | `MeetingSnapshot` | idempotent、Completedまで待つ |
| `get_meeting_snapshot` | `MeetingSnapshot` | memory上のcurrent session |
| `watch_meeting` | `void + Channel<MeetingEvent>` | queue max 128 |
| `save_meeting_transcript` | `SavedMeeting` | Completedからのみ |
| `discard_meeting` | `void` | memory本文を破棄 |

## 7. Frontend capture contract

### 7.1 Microphone segmenter

Core shipping pathはWebViewの`getUserMedia`とWeb Audio APIを使う。`MediaRecorder`のcompressed Blobを長時間蓄積しない。

実装順:

1. `getUserMedia({audio: device constraint})`でstreamを取得。
2. `AudioContext`と`MediaStreamAudioSourceNode`を作る。
3. `AudioWorkletNode`を優先し、利用不可ならScriptProcessor fallbackは採用せずunsupportedを表示する。
4. Float32 PCMを5秒単位に区切る。
5. lane queueへ入れ、同時送信は1件に限定する。
6. command完了後に対象ArrayBufferへの参照を破棄する。
7. pause/stop/unmountでtrack.stop、node.disconnect、context.close、queue.clearを必ず実行する。

`public/audio/meeting-processor.js`をworkletとして追加する。workletはnetwork、storage、DOMへアクセスせず、PCM frameをpostMessageするだけとする。

### 7.2 Partialの意味

既存Whisper CLIは真のstreaming APIではない。MVP 2 Coreの`partial`は「直近5秒segmentの処理中に得たCLI出力」、`final`はそのsegmentの確定結果であり、session全文の再推論ではない。segment境界で単語が欠ける可能性を既知制限として表示する。

重複除去は次の限定ルールだけを使う。

- 前segment末尾32文字と次segment先頭32文字の最大一致を除く。
- 一致が8文字未満なら除去しない。
- LLMによる修正、要約、話者推定をしない。

### 7.3 UI state

Meeting画面は次を必ず表示する。

```text
state / elapsed time
microphone selected device and health
system audio capability and health
STT provider = local-whisper / model filename
translation = disabled or explicit route
persistence = Discard unless user presses Save after stop
original transcript window
Pause / Resume / Stop
Completed時のSave / Discard
```

Stopはprimary destructive actionとして常に同じ位置へ表示する。error bannerでStopを覆わない。Meeting active中に別Surfaceへ移動してもsessionは継続するが、primary navへ赤いActive indicatorとStop導線を出す。

## 8. SQLite v6

MVP 2のschema versionは6。MVP 1.5と同様、backup後にtransaction migrationする。settings documentは6へ上げるが、Meeting設定documentはこのMVPでは増やさない。Voice settingsの`sttModel`と`inputDeviceId`をpreflight初期値として使う。

```sql
CREATE TABLE meeting_sessions (
  id TEXT PRIMARY KEY,
  status TEXT NOT NULL CHECK(status IN (
    'active', 'paused', 'completed', 'saved', 'discarded', 'failed', 'interrupted'
  )),
  microphone_enabled INTEGER NOT NULL CHECK(microphone_enabled IN (0, 1)),
  system_audio_enabled INTEGER NOT NULL CHECK(system_audio_enabled IN (0, 1)),
  stt_provider_id TEXT NOT NULL CHECK(stt_provider_id = 'local-whisper'),
  stt_model_label TEXT NOT NULL CHECK(length(stt_model_label) <= 256),
  translation_provider_id TEXT,
  persistence_mode TEXT NOT NULL CHECK(persistence_mode IN ('discard', 'explicit-save')),
  started_at TEXT NOT NULL,
  ended_at TEXT,
  saved_at TEXT,
  error_code TEXT
);
CREATE INDEX idx_meeting_sessions_started
  ON meeting_sessions(started_at DESC);

CREATE TABLE meeting_transcript_entries (
  id TEXT PRIMARY KEY,
  session_id TEXT NOT NULL,
  lane TEXT NOT NULL CHECK(lane IN ('microphone', 'system-audio')),
  sequence INTEGER NOT NULL CHECK(sequence >= 0),
  original_text TEXT NOT NULL CHECK(length(original_text) BETWEEN 1 AND 8000),
  original_language TEXT,
  translated_text TEXT CHECK(translated_text IS NULL OR length(translated_text) <= 8000),
  translated_language TEXT,
  started_at_ms INTEGER NOT NULL CHECK(started_at_ms >= 0),
  ended_at_ms INTEGER NOT NULL CHECK(ended_at_ms >= started_at_ms),
  created_at TEXT NOT NULL,
  FOREIGN KEY(session_id) REFERENCES meeting_sessions(id) ON DELETE CASCADE,
  UNIQUE(session_id, lane, sequence)
);
CREATE INDEX idx_meeting_transcript_session_sequence
  ON meeting_transcript_entries(session_id, lane, sequence);
```

### 8.1 保存境界

- Start時に`meeting_sessions` metadataを`persistence_mode=discard`で作成してよい。
- Active/Paused中、`meeting_transcript_entries`は常に0件でなければならない。
- Save commandはmemory上の全final entryを一つのtransactionでinsertし、sessionを`saved`へ更新する。
- 1 session最大2,000 entries、合計1 MiB相当のUTF-8 textを上限とする。超過時はSaveを拒否し、範囲選択機能はMVP 2に追加しない。
- Discardはsession metadataを`discarded`に更新し、memory本文を破棄する。metadataも残したくない場合の削除操作はSettingsのPrivacy controlsで別途提供する。
- diagnosticsへ本文、model path、device id、capture tokenを出さない。件数とstatusだけを出す。

## 9. Meeting policy guard

`MeetingPolicy`は最低限次の問い合わせを持つ。

```rust
pub fn blocks_tts(&self) -> bool;
pub fn blocks_automatic_agent_run(&self) -> bool;
pub fn blocks_notification(&self) -> bool;
pub fn blocks_application_action(&self) -> bool;
```

MVP 2で実在する実行経路には必ずguardを置く。

- `speak_text`: Active/Paused/Stoppingなら`MEETING_POLICY_TTS_BLOCKED`。
- Chat response完了後の`autoSpeak`: Frontendでも抑止するが、Rust guardを正とする。
- CodexはユーザーがChatから明示開始する既存経路まで禁止しない。ただしMeetingから自動起動するコードを作らない。
- notification/application actionは現行未実装。Meeting moduleのcall-graph testで将来の混入を検出する。

Meeting開始前にTTSが動作中ならStartを拒否し、ユーザーへ`Stop speech and retry`を返す。勝手に既存TTSを停止しない。

## 10. P0 feasibility gates

### M2-00A — Microphone worklet spike

検証項目:

- Tauri WebViewでAudioWorkletがbundleからloadできる。
- 30分fixture captureで5秒segment、queue最大2、heapが継続増加しない。
- pause/stop/unmount後にOSのmicrophone indicatorが消える。
- permission拒否と途中revokeが復旧可能なerrorになる。

PassならCore実装へ進む。Failならnative microphone captureのADRを作り、実装方式を決めるまでM2-03以降を開始しない。

### M2-00B — macOS system audio spike

spike専用branch/moduleで次を検証する。

- ScreenCaptureKit等の採用候補、最小macOS version、必要entitlement、permission文言。
- app/system audioをPCM frameとして得られる。
- 画面frameを取得・保存せずaudioだけに限定できる。
- pause/stop/revoke/app closeでcapture objectを解放できる。
- signed app bundleで動作する。

成果物は`spec/docs/spikes/mvp-2-macos-system-audio.md`。採用crate/framework、サンプルコード、entitlement、実測cleanup、既知リスクを書く。

Gate:

- 全項目Pass: Optional laneをM2-08で実装。
- 一つでも未確認: build capabilityをfalseにしてmicrophone-onlyでship。
- workaroundとしてvirtual audio deviceの導入をユーザーへ要求しない。

### M2-00C — Floating overlay spike

別Tauri windowが必要な場合だけ実施する。window create/destroy、always-on-top opt-in、focusを奪わない、session終了で閉じる、capability設定を確認する。Failならmain window Meeting surfaceをshipする。

## 11. 実装チケット

### M2-01 — v6 migrationとreconcile

- DDL、backup閾値、settings schema version 6を実装する。
- startupでunfinished sessionを`interrupted`へする。
- test: v5→v6 data preservation、migration idempotency、transcript 0件。
- Done: MVP 0〜1.5の全table/setting/profileが保持される。

### M2-02 — State machineとpolicy

- `meeting/state_machine.rs`と`policy.rs`を純粋Rustで実装する。
- test: 全許可遷移、全禁止遷移、idempotent stop、TTS block。
- Done: Tauri/DBなしで遷移表を網羅できる。
- Depends on: M2-01。

### M2-03 — Whisper core抽出

- 既存voice関数を`voice/local_whisper.rs`へ移動する。
- 一時WAV cleanupを成功/失敗/取消でtestする。
- existing `transcribe_audio` command signatureは変更しない。
- Done: 既存voice fixture、resampling、cancellation testが同じ期待値でpass。

### M2-04 — MeetingRuntimeとcommands

- Runtime、event queue、capture token、segment validation、commandを実装する。
- token comparisonとsequence validationを先に行い、不正inputをworkerへ渡さない。
- test: queue bound、invalid token、duplicate/out-of-order sequence、pause中segment拒否。
- Depends on: M2-02, M2-03。

### M2-05 — Microphone capture

- AudioWorklet、PCM segmenter、queue、cleanupを実装する。
- `useMeetingSession`だけがstream/node/contextを所有する。
- test: PCM chunk boundary、queue backpressure、cleanupのunit testと手動permission test。
- Depends on: M2-00A, M2-04。

### M2-06 — Meeting UI

- nav、preflight、active、paused、completed、failedの各状態を実装する。
- Stopは常時表示、SaveはCompletedだけ、StartはReadyかつblocking error 0件だけ有効。
- Situation candidateは補助labelのみ。
- test: stateごとのbutton enablement、error recovery、Surface移動中indicator。
- Depends on: M2-05。

### M2-07 — Explicit Save/Discard

- repositoryとcommandsを実装する。
- Save前DB本文0件、transaction failure時0件、Save後reopen、Discard後memory破棄をtestする。
- UIで保存対象、件数、言語、削除場所をconfirmする。
- Depends on: M2-04, M2-06。

### M2-08 — System audio lane（Gate通過時のみ）

- M2-00Bで確定したnative adapterを`meeting/audio/system/macos.rs`へ実装する。
- microphoneと同じsegment DTOへ正規化する。
- lane別sequence、health、stopをtestする。
- Gate未通過時: 実装せずcapability falseのtestを追加してDoneとする。

### M2-09 — Translation（providerが確定した場合のみ）

- settingsにlocal translation providerが存在する場合だけADR後に実装する。
- routing keyは`meeting.translate`。fallbackなし。final segmentだけを対象にする。
- provider未確定時: UIをdisabled、commandを未登録としてDoneとする。
- Cloud consent UIはMVP 2 Coreに入れない。

### M2-10 — Overlay（Gate通過時のみ）

- M2-00C Passなら別window、Failならmain window surfaceを完成形とする。
- always-on-topはsessionごとのopt-in、default false。
- overlayが閉じてもcaptureを暗黙stopしない。primary windowへ状態を残す。

### M2-11 — Release evidence

`spec/docs/mvp-2-release-evidence.md`を作る。

記録内容:

- shipping capabilities（microphone/system audio/overlay/translation）
- migration backupとreopen結果
- 30分、2時間のresource soak結果
- permission denied/revoke、pause/resume、stop、app closeの結果
- Save前DB本文0件、Save後件数、Discard結果
- raw audio temporary file cleanupの確認
- TTS/notification/agent/application action guard
- test件数、desktop smoke、既知制限

## 12. エラーコード

UIで文字列matchingをしない。最低限次を固定する。

```text
MEETING_INVALID_STATE
MEETING_PERMISSION_DENIED
MEETING_DEVICE_UNAVAILABLE
MEETING_STT_MODEL_MISSING
MEETING_STT_FAILED
MEETING_INVALID_CAPTURE_TOKEN
MEETING_OUT_OF_ORDER_SEGMENT
MEETING_BACKPRESSURE
MEETING_POLICY_TTS_BLOCKED
MEETING_SYSTEM_AUDIO_UNAVAILABLE
MEETING_TRANSLATION_UNAVAILABLE
MEETING_SAVE_LIMIT_EXCEEDED
MEETING_INTERRUPTED
```

`message`と`recovery`は既存`redact_runtime_text`相当を通し、各最大500文字。path、device id、transcript、CLI stdout全文を含めない。

## 13. テストと検証コマンド

```bash
bun run typecheck
bun run test:frontend
cargo fmt --check --manifest-path src-tauri/Cargo.toml
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
cargo test --manifest-path src-tauri/Cargo.toml
bun run build
bun run desktop:smoke
```

最低限追加するRust test:

```text
version_five_database_migrates_to_six_without_data_loss
startup_reconciles_unfinished_meeting_without_transcript_persistence
meeting_state_machine_rejects_every_undefined_transition
meeting_stop_pause_and_discard_are_idempotent
meeting_segment_queue_is_bounded_and_ordered
invalid_capture_token_never_reaches_stt
meeting_policy_blocks_tts_while_active_or_paused
meeting_save_is_atomic_and_discard_writes_no_transcript
meeting_temp_audio_is_removed_on_success_failure_and_cancel
meeting_module_has_no_notification_agent_or_application_action_calls
```

Frontend test:

- segmentは5秒で切られ、同時送信1、queue最大2。
- pause/stop/unmountで全track、AudioContext、queueをcleanup。
- preflight error中はStart disabled。
- Active/PausedではSave hidden、CompletedだけSave/Discard表示。
- Surface移動後もActive indicatorとStop導線が残る。
- v6 settings snapshotを受理し、v5 save payloadを拒否。

手動desktop matrix:

| Case | Expected |
|---|---|
| permission初回許可 | preflight確認中だけindicatorが点灯し、track停止後からStart操作までは消灯する |
| permission拒否 | captureなし、recovery表示、Start disabled |
| 途中revoke | FailedまたはPaused、track停止、buffer破棄 |
| 30分active | bounded queue、継続的heap増加なし |
| pause 5分 | transcript増加なし、microphone indicator消灯 |
| resume | 新token、新sequence streamで再開 |
| stop連打 | crashせず同じCompleted snapshot |
| Save | reopen後にfinalだけ復元 |
| Discard | transcript tableへ本文0件 |
| active中Chat TTS | policy error、音声再生なし |
| app close | capture停止、次回metadata interrupted |

## 14. 受け入れ条件

### Core（必須）

1. ユーザー操作なしにMeeting、microphone、STTが開始しない。
2. microphone-only sessionをStart/Pause/Resume/Stopできる。
3. 5秒segment処理でqueueとmemoryがboundedである。
4. partial/final transcriptがlaneとsequenceを保って表示される。
5. pause、stop、permission revoke、app closeでcapture resourceと一時audioを解放する。
6. Active/Paused中はSQLiteにtranscript本文が0件である。
7. Completed後の明示Saveだけがfinal transcriptをtransaction保存する。
8. Discard後にmemory本文が消え、SQLiteへ本文を保存しない。
9. Meeting中にTTS、notification、自動Model/Codex、application actionが起動しない。
10. Cloud STT fallbackが存在せず、model/device/path/transcriptをdiagnosticsへ出さない。
11. v5→v6 migrationでMVP 0〜1.5データを失わない。
12. 既存Chat voice、Settings、Codex、Situationのtestとdesktop smokeに回帰がない。

### Optional（capability=trueでshipする場合のみ）

13. system audioは明示opt-in、permission-gated、audio-onlyで、lane単独停止できる。
14. floating overlayはdefault offで、focusを奪わず、終了時に破棄される。
15. translationは明示enable、確定済みlocal route、fallbackなしである。

## 15. 実装中にADRが必要な条件

- AudioWorkletが対象Tauri WebViewで安定せずnative microphone captureへ変更する。
- Whisperのsegment境界品質を上げるためoverlap audioを保持したい。
- 一時WAVも禁止し、Whisper libraryのin-process利用へ変更する。
- system audioのframework、entitlement、最小macOS versionを決める。
- 別window overlayへ新しいTauri capabilityを追加する。
- translation providerまたはCloud consentを追加する。
- transcript上限を超えた会議の部分保存・exportを追加する。

ADRが必要な項目は、判断が確定するまでOptional扱いとし、担当者の推測でCoreへ混ぜない。
