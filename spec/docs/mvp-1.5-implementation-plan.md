# MVP 1.5 実装契約 — Situation Calibration and Review

## 0. 文書の役割

- Status: Proposed
- 実装前提: [`mvp-1-release-evidence.md`](./mvp-1-release-evidence.md) がAcceptedであること
- 次のマイルストーン: [`mvp-2-implementation-plan.md`](./mvp-2-implementation-plan.md)
- この文書の読者: 実装担当、レビュー担当、QA担当
- 完了の定義: 本文の`M15-01`〜`M15-09`が完了し、受け入れ条件を証跡付きで満たすこと

この文書は方向性だけを示すロードマップではない。実装中に担当者が新たなデータモデルやAPIを考案しなくてよいように、変更場所、契約、移行、テスト、完了条件を固定する。

## 1. 今回達成すること

MVP 1で記録したboundedなSituation判断を、ユーザーが評価し、固定fixtureで再生し、候補パラメータと現行パラメータを比較できるようにする。

MVP 1.5は校正機能であり、介入機能ではない。次は全期間を通して不変とする。

```text
automaticModelCall = false
automaticTTS = false
automaticNotification = false
automaticApplicationAction = false
actualExecution = NONE
actualPresentation = SILENT
```

校正候補は自動適用しない。ユーザーがReview画面で明示的にAcceptした版だけを、次回tickからShadow分類に使う。Reject、失敗、画面を閉じた操作はactive profileを変更しない。

## 2. コードレビュー結果と設計判断

| 現在の実装 | レビュー結果 | MVP 1.5での扱い |
|---|---|---|
| `src-tauri/src/situation/classifier.rs`に閾値`45/70/3/5/10000ms`が直書き | replay対象となるルールを差し替えられない | `CalibrationParameters`へ抽出し、現行値をdefaultにする |
| `RULE_VERSION = "mvp1-rules-v1"` | コード定数だけではAccept/Rollback履歴を表せない | SQLiteのprofileを正とし、定数は初回seed専用にする |
| `situation_ledger`はtransition/decision/heartbeatだけを保存 | 毎tickのcandidate変化、stale、UNKNOWNをSQLだけでは再構成できない | raw sampleを保存せず、heartbeat区間の集約カウンタを追加する |
| feedbackは`accurate/inaccurate/unsure` | 正誤と害・不要を一つのenumへ混ぜると既存意味が壊れる | verdictは維持し、`impact`と`reason_code`を追加する |
| Inaccurate選択時にUIが`UNKNOWN`を自動保存 | ユーザーの訂正ではなくUIの推測になる | Scene訂正を明示選択にし、未選択は`null`のまま保存する |
| JSON decodeに`unwrap_or_default`がある | 壊れた評価データが正常値に見える可能性がある | 校正用JSONはdecode失敗をエラーとしてUIに表示する |
| `SituationPage.tsx`が取得、監視、表示、feedbackを一ファイルで担当 | Review追加で責務が過密になる | `review/`配下へAPI hookと表示部品を分離する |
| settings文書とDBのversionは4 | 新テーブルと契約追加にversion境界が必要 | settings schemaと`PRAGMA user_version`を5へ上げる |
| 8時間fixtureとcall-graph safety testが既にある | 強い回帰防止策として利用可能 | 削除・弱体化せず、profile/replay対応へ拡張する |

### 2.1 実装中に変更してはいけない判断

- feedbackの正誤軸は`accurate | inaccurate | unsure`のまま維持する。
- 影響軸は`none | no-effect | harmful`とする。既存行は`none`へ移行する。
- 任意コメント欄は作らない。理由はallowlistのreason codeだけとする。
- 実運用の毎tick signal、アプリ名、window title、calendar本文、audioは保存しない。
- calibrationの対象は閾値とhysteresisだけにする。evidence weightの編集UIは作らない。
- replay fixtureはリポジトリ同梱の静的JSONとし、ユーザーの履歴をfixtureへ変換しない。
- Reviewを開いたことを理由に監視を開始しない。

## 3. モジュール境界

### 3.1 Rust

```text
src-tauri/src/situation/
├── contracts.rs                 # 既存。feedback/snapshotの共有契約を更新
├── classifier.rs                # 既存。profile引数を受け取る純粋分類器へ変更
├── mod.rs                       # 既存。active profileとquality accumulatorを保持
├── repository.rs                # 既存。ledger/feedback/settingsに限定
└── calibration/
    ├── mod.rs                   # use caseの公開口
    ├── contracts.rs             # profile/run/review DTO
    ├── metrics.rs               # 集約・率・比較。DB/Tauriへ依存しない
    ├── replay.rs                # fixture読込と決定論的replay
    └── repository.rs            # profile/run/quality windowのSQLite操作

src-tauri/fixtures/situation/
├── manifest.json
└── mvp1-v1.json
```

`lib.rs`にはTauri command、マイグレーション呼び出し、`generate_handler!`登録だけを置く。metricsやcandidate lifecycleを`lib.rs`へ実装しない。

### 3.2 Frontend

```text
src/features/situation/
├── SituationPage.tsx            # Overview/Reviewの切替と既存overview
└── review/
    ├── SituationReview.tsx       # 画面composition
    ├── useSituationReview.ts     # load/mutate/error state
    ├── QualitySummary.tsx
    ├── FeedbackEditor.tsx
    ├── ReplayComparison.tsx
    └── CalibrationHistory.tsx

src/lib/contracts.ts              # Rust DTOと1対1の型
src/lib/runtime.ts                # invoke wrapper
src/lib/schemas.ts                # settings schema version 5
tests/situation-review.test.ts
```

CSSは既存の`src/App.css`とhono-standard由来のdesign tokenを使う。Review専用の別themeは作らない。

## 4. 固定する契約

### 4.1 Sceneとパラメータ

Rustでは校正境界に限り、任意Stringではなくenumを使う。既存ledgerのStringはmigration互換のため、このMVPでは全面置換しない。

```rust
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum SituationScene {
    Conversation,
    Meeting,
    Coding,
    Writing,
    Media,
    Focus,
    Solo,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CalibrationParameters {
    pub classification_min_confidence: u8, // 50..=95, default 70
    pub low_confidence_max: u8,             // 0..=60, default 45
    pub enter_sample_count: u8,             // 1..=10, default 3
    pub exit_sample_count: u8,              // 1..=20, default 5
    pub cooldown_ms: u64,                    // 0..=60_000, default 10_000
}
```

追加の相互制約:

```text
lowConfidenceMax < classificationMinConfidence
candidate JSON serialized size <= 2 KiB
```

`classify(signals, parameters)`と`Hysteresis::update(candidate, parameters, now, now_ms)`へ変更する。default profileを渡したとき、MVP 1の全classifier testが同じ結果になること。

### 4.2 Feedback

```rust
pub struct SituationFeedbackInput {
    pub ledger_id: String,
    pub verdict: String,          // accurate | inaccurate | unsure
    pub impact: String,           // none | no-effect | harmful
    pub corrected_scene: Option<String>,
    pub reason_code: Option<String>,
}
```

許可する`reason_code`:

```text
wrong-scene
stale-signal
unstable-transition
unwanted-suggestion
missed-meeting-candidate
insufficient-evidence
```

validation:

- `verdict=inaccurate`では`reason_code`を必須にする。
- `impact=harmful`では`reason_code`を必須にする。
- `corrected_scene`があれば既存`validate_scene`を通す。
- `impact=no-effect`は`proposed_attention=SUGGEST`のledger行だけ許可する。
- feedbackは同じ`ledger_id`へupsert可能とする。

### 4.3 Quality window

`SituationRuntime::tick`内で、永続化前に次のカウンタだけを加算する。生のSignalSnapshotは保持しない。

```rust
pub struct QualityWindowAccumulator {
    pub started_at_ms: u128,
    pub sample_count: u64,
    pub candidate_change_count: u64,
    pub stable_transition_count: u64,
    pub unknown_sample_count: u64,
    pub stale_owned_signal_count: u64,
    pub decision_ignore_count: u64,
    pub decision_observe_count: u64,
    pub decision_suggest_count: u64,
    pub decision_respond_count: u64,
    pub health_ready_count: u64,
    pub health_disabled_count: u64,
    pub health_permission_denied_count: u64,
    pub health_unsupported_count: u64,
    pub health_degraded_count: u64,
}
```

- heartbeat保存時またはmonitoring停止時にwindowをflushする。
- `sample_count=0`のwindowは保存しない。
- 1 windowの最大期間は既存`heartbeat_interval_ms`。
- flush成功後だけaccumulatorをresetする。DB失敗時は最大2 window分までsaturating addし、それ以上は`degraded` eventを出してresetする。
- flappingは`candidate_change_count / sample_count`、staleは`stale_owned_signal_count / sample_count`として計算する。
- 分母が20未満の率は`null`を返し、UIは`Insufficient data`と表示する。

### 4.4 Review snapshot

```ts
export type SituationReviewSnapshot = {
  activeProfile: CalibrationProfile;
  quality: SituationQualityMetrics;
  feedbackQueue: SituationLedgerEntry[]; // max 50, newest first
  latestRun: CalibrationRun | null;
  candidates: CalibrationCandidate[];    // max 20, newest first
};
```

全list APIはlimitをサーバー側で固定する。Frontendから任意limitやSQL filterを渡さない。

## 5. SQLite v5 migration

`initialize_database`を段階migrationへ変更する。現行の`CREATE TABLE IF NOT EXISTS`は維持してよいが、version 4→5の変更を`migrate_v4_to_v5(&mut Connection)`として分離する。起動時backupは`version < 5`で作る。

### 5.1 DDL

```sql
ALTER TABLE situation_feedback
  ADD COLUMN impact TEXT NOT NULL DEFAULT 'none'
  CHECK(impact IN ('none', 'no-effect', 'harmful'));

ALTER TABLE situation_feedback
  ADD COLUMN reason_code TEXT
  CHECK(reason_code IS NULL OR reason_code IN (
    'wrong-scene', 'stale-signal', 'unstable-transition',
    'unwanted-suggestion', 'missed-meeting-candidate',
    'insufficient-evidence'
  ));

CREATE TABLE situation_quality_windows (
  id TEXT PRIMARY KEY,
  started_at TEXT NOT NULL,
  ended_at TEXT NOT NULL,
  rule_version TEXT NOT NULL,
  counters_json TEXT NOT NULL CHECK(length(counters_json) <= 4096),
  created_at TEXT NOT NULL
);
CREATE INDEX idx_situation_quality_windows_ended
  ON situation_quality_windows(CAST(ended_at AS INTEGER) DESC);

CREATE TABLE situation_calibration_profiles (
  id TEXT PRIMARY KEY,
  rule_version TEXT NOT NULL UNIQUE,
  base_rule_version TEXT,
  status TEXT NOT NULL CHECK(status IN ('candidate', 'active', 'superseded', 'rejected', 'rolled-back')),
  parameters_json TEXT NOT NULL CHECK(length(parameters_json) <= 2048),
  created_at TEXT NOT NULL,
  decided_at TEXT,
  decision_reason_code TEXT,
  FOREIGN KEY(base_rule_version) REFERENCES situation_calibration_profiles(rule_version)
);
CREATE UNIQUE INDEX idx_situation_calibration_one_active
  ON situation_calibration_profiles(status) WHERE status = 'active';

CREATE TABLE situation_calibration_runs (
  id TEXT PRIMARY KEY,
  profile_id TEXT NOT NULL,
  fixture_set_version TEXT NOT NULL,
  status TEXT NOT NULL CHECK(status IN ('completed', 'failed')),
  metrics_json TEXT CHECK(metrics_json IS NULL OR length(metrics_json) <= 8192),
  error_code TEXT,
  started_at TEXT NOT NULL,
  completed_at TEXT NOT NULL,
  FOREIGN KEY(profile_id) REFERENCES situation_calibration_profiles(id) ON DELETE CASCADE
);
CREATE INDEX idx_situation_calibration_runs_completed
  ON situation_calibration_runs(completed_at DESC);
```

### 5.2 migration手順

1. `user_version`を読む。
2. `< 5`なら既存backup機構でDB全体をbackupする。
3. transactionを開始する。
4. 上記DDLを実行する。column存在確認を行い再実行可能にする。
5. `mvp1-rules-v1` profileをdefault parameters、`active`で`INSERT OR IGNORE`する。
6. 6 settings documentsの`schema_version`を5へ更新する。value JSONは変更しない。
7. `PRAGMA user_version = 5`を設定してcommitする。
8. startup時にactive profileが0件または2件以上ならSituation monitoringを開始せず、復旧可能なエラーを返す。

既存のConversation、Message、Codex thread、runtime run、ledger、feedbackを再生成しない。migration testでは行数と代表値を比較する。

## 6. Tauri command

| Command | Input | Output | Side effect |
|---|---|---|---|
| `get_situation_review_snapshot` | なし | `SituationReviewSnapshot` | なし |
| `submit_situation_feedback` | 拡張input | `SituationReviewSnapshot` | feedbackをupsert |
| `create_situation_calibration_candidate` | `CalibrationParameters` | `CalibrationProfile` | `candidate` profileを作る。activeは変えない |
| `run_situation_calibration` | `profileId` | `CalibrationRun` | fixtureを同期replayしrunを保存 |
| `decide_situation_calibration` | `profileId`, `decision`, `reasonCode` | `SituationReviewSnapshot` | accept/reject/rollback transaction |
| `clear_situation_history` | なし | 既存`SituationSnapshot` | ledger、feedback、quality window、runを削除。profileは残す |

候補作成時のstatusは`candidate`とする。`rejected`はユーザーが明示的にRejectした後だけ使用する。

`run_situation_calibration`はfixtureが14,400 samples以下のためcommand内の`spawn_blocking`で完了を待つ。進捗ChannelはMVP 1.5では作らない。処理が2秒を超える場合だけ別ADRを作り、cancelable jobへ変更する。

Acceptは`SituationRuntime::activate_profile(connection, profile)`へ集約する。command側でDB更新とruntime更新を別々に実行しない。このmethodはruntime lockを取得した後、parametersを検証し、DB transactionをcommitし、同じlock guardへprofileを代入してから解放する。代入自体はfallibleな処理を持たせない。

Accept transaction:

1. 対象が`candidate`で、最新runが`completed`であることを検証。
2. 現在の`active`を`superseded`へ更新。
3. 対象を`active`へ更新。
4. transactionをcommitする。
5. 保持中のruntime lockへ対象profileを代入してlockを解放する。

Rollbackは直前の`superseded`をactiveに戻し、現在activeを`rolled-back`にする。同じmethod、lock順序、transactionとruntime反映順序を使う。全Situation commandのlock順序を`runtime → database`へ統一し、逆順取得を禁止する。

## 7. Fixture contract

`manifest.json`:

```json
{
  "fixtureSetVersion": "situation-fixtures-v1",
  "files": ["mvp1-v1.json"],
  "maxSamples": 14400
}
```

各sampleは既存`SignalSnapshot`と`elapsedMs`だけを持つ。expected sceneはmetrics計算用のラベルとしてfixtureに置き、runtime inputへ渡さない。

```ts
type ReplaySample = {
  elapsedMs: number;
  signals: SignalSnapshot;
  expectedScene: SituationScene;
};
```

必須シナリオ:

- explicit conversation開始/終了
- coding、writing、media、soloの安定区間
- communication + calendar meeting likely
- sensitive foregroundによるUNKNOWN safe default
- permission denied、unsupported、degraded
- 1〜2 sampleだけのnoiseが遷移しないこと
- 5 sample low confidenceでUNKNOWNへ戻ること
- SUGGEST/RESPONDを含む全shadow policy path

replayはwall clock、乱数、DBを参照しない。同一fixture、同一profileを100回実行してmetrics JSONがbyte-equivalentであること。

## 8. 実装チケット

### M15-01 — v5 migrationとprofile seed

- 変更: `lib.rs`のbackup閾値、migration、settings schema versionを5へ更新する。
- test: v4 DBを作成しmigration後の全既存行、active profile 1件、user_version 5を検証する。
- Done: migrationを2回呼んでも失敗せず、既存6 settings documentのvalueが変わらない。

### M15-02 — Parameterized classifier

- 変更: `CalibrationParameters`を追加し、classifier/hysteresisのmagic numberを置換する。
- test: default parity、各bound、相互制約、cooldown、enter/exit count。
- Done: 既存Situation unit testが無変更の期待値で通る。
- Depends on: M15-01。

### M15-03 — Quality accumulator

- 変更: `RuntimeInner`へaccumulatorを追加し、tickで加算、heartbeat/停止でflushする。
- test: 0 sample非保存、flush後reset、DB失敗時のbounded behavior、率のnull分母。
- Done: 8時間fixtureでもmemoryとDB row数がbounded。
- Depends on: M15-01, M15-02。

### M15-04 — Feedback v2

- 変更: repository DTO、join query、Frontend型、UI editorを更新する。
- test: 既存feedback互換、invalid combination拒否、upsert、reopen、cascade delete。
- Done: Inaccurateを選んでもSceneを勝手にUNKNOWNへしない。
- Depends on: M15-01。

### M15-05 — Deterministic replay

- 変更: fixture、parser、metrics、run repository、commandを実装する。
- test: max sample/unknown field/invalid enum拒否、100回同一結果、failed runがactive profileを変更しない。
- Done: baseline profileの結果がrelease evidenceへ貼れるJSONとして得られる。
- Depends on: M15-02, M15-03。

### M15-06 — Candidate lifecycle

- 変更: create/run/accept/reject/rollbackとruntime profile swapを実装する。
- test: invalid transition、active unique、transaction rollback、app reopen後のactive復元。
- Done: Accept前後とRollback後のfixture結果が期待profile versionを示す。
- Depends on: M15-05。

### M15-07 — Review UI

- 変更: Situationに`Overview | Review`切替を追加し、review部品を実装する。
- 表示: active version、data sufficiency、quality counters、feedback queue、baseline比較、version history。
- error: command errorを既存`error-banner`へ表示し、空配列へ握り潰さない。
- Done: keyboardだけでfeedback、replay、accept/reject/rollbackを操作できる。
- Depends on: M15-04, M15-06。

### M15-08 — Safety、privacy、diagnostics

- 変更: diagnosticsへ件数、active rule version、latest calibration statusだけを追加する。
- 禁止: metrics JSON、feedback、transcript、evidenceの本文をexportしない。
- test: call-graph guardに新moduleを含め、outbound/TTS/notification/application actionが無いこと。
- Done: `actualExecution=NONE`、`actualPresentation=SILENT`の既存testが通る。

### M15-09 — Release evidence

- `spec/docs/mvp-1.5-release-evidence.md`を作る。
- version、migration backup path、test件数、fixture hash、baseline metrics、privacy inventory、既知の制限を記録する。
- Done: 下記acceptanceを証跡へ1対1でリンクする。

## 9. テストと実行コマンド

実装完了時はrepository rootで次を順に実行する。

```bash
bun run typecheck
bun run test:frontend
cargo fmt --check --manifest-path src-tauri/Cargo.toml
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
cargo test --manifest-path src-tauri/Cargo.toml
bun run build
bun run desktop:smoke
```

最低限追加するtest名:

```text
version_four_database_migrates_to_five_without_data_loss
default_calibration_profile_preserves_mvp_one_classification
quality_windows_are_bounded_and_flush_on_pause
calibration_replay_is_deterministic
failed_or_rejected_candidate_never_changes_active_profile
accepted_candidate_survives_reopen_and_rolls_back
feedback_v2_validates_cross_field_rules
calibration_module_has_no_outbound_or_intervention_calls
```

Frontend test:

- v5 settings 6-document snapshotを受理し、v4 save payloadを拒否する。
- feedback cross-field validationをRustと同じ期待値で確認する。
- Reviewのinsufficient data、loading、empty、errorを確認する。
- Accept/rollbackのconfirmを確認する。

## 10. 受け入れ条件

1. v4 DBがbackup後にv5へ移行し、既存の設定、Conversation、Codex thread、ledger、feedbackを失わない。
2. default profileはMVP 1と同じ分類・hysteresis結果を返す。
3. 実運用quality metricsはraw sampleを保存せず算出できる。
4. feedbackは正誤、影響、訂正Scene、理由を明示的に保存し、再起動後も復元する。
5. fixture replayは決定論的で、profile間の差を表示できる。
6. candidateはallowlist外のparameterと範囲外の値を受理しない。
7. Acceptしたcandidateだけがactiveになり、Reject/失敗はruntimeへ影響しない。
8. 直前profileへRollbackでき、再起動後もその状態を保つ。
9. Reviewはユーザーが開いた時だけ表示され、監視、通知、TTS、Model、外部操作を開始しない。
10. diagnostics、SQLite、fixtureにraw app identity、window title、calendar detail、audio、prompt、responseを保存しない。
11. 8時間fixtureでevent queue、quality window、run historyがboundedである。
12. MVP 1のsafety testとdesktop smokeに回帰がない。

## 11. MVP 2へ進む判定

次をすべて満たしたときだけMVP 2へ進む。

- privacy inventoryとcall-graph guardがpass。
- default profileに対して候補profileのharmful feedback率が悪化していない。分母20未満なら改善判定をせずdefaultを維持する。
- Meeting fixtureでfalse positive数がbaselineを超えない。
- unsupported/degraded時にUNKNOWNまたはIGNOREへ落ちる。
- active profileのAccept/Rollbackがreopenを含めて検証済み。

数値が不足している場合はMVP 1.5を失敗扱いにせず、default profileのままMVP 2を「明示開始のみ」で実装する。Situation候補をMeeting開始の根拠には使わない。

## 12. Stop / ADR条件

次の場合は実装者が独自判断でscopeを広げず、`spec/docs/adr/`へ判断材料を記録する。

- 評価にraw signal保存が必要になった。
- fixture replayが2秒を継続的に超え、job/cancel設計が必要になった。
- SQLite transaction後のruntime profile swapを安全に補償できない。
- feedback taxonomyへ自由文や新しい個人情報を追加したい。
- 校正対象をevidence weightやLLM分類まで広げたい。
