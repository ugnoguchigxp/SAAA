# Butler Schedule Ledger 実装計画 — 期限台帳、tick、Google Calendar 投影

作成日: 2026-09-21。状態: **SL-A〜D 実装済み（offline）。SL-24 live 未実施**。

見送り理由: 発火の権限は Goal / Delegation / Task に依存する。時刻の到来を権限にしない。会議中の保留は Situation TTS hold に依存する。いずれも執事循環の最小循環（Step 4）と TTS ゲート（Step 3）が先。揃い次第、§2 の SL-A（Calendar なしの期限台帳と tick）から着手する。

上位は[Personal AI Concept](saaa-personal-ai-concept.md) §4（循環の契機に「期限」「定期確認」を含む）と §9（委任）。前提となる並行作業は World Model M3A/M3B、Situation の TTS 抑止、最小Task循環（Goal / Delegation / Task Runtime の最初の実体）。実装担当は本書と[作業カード](saaa-butler-schedule-ledger-work-cards.md)をセットで使う。

## 1. 次に完成させるもの

SAAA が「時間の経過」を契機に動けるようにする。現在の契機はユーザー発話と Situation 遷移だけであり、リマインド、締切前の準備、保留した結果の後刻通知、定期確認のいずれも成立しない。

三つの台帳を追加し、混ぜない。

```text
schedule_entries        約束の時計。SAAA が所有する唯一の正本
calendar_projections    約束の見せ方。Google Calendar への冪等な投影（再構築可能）
calendar_observations   ユーザーの手書き修正。Calendar 側の差分を観測として受け取る
```

Google Calendar は執事の予定を**可視化し、ユーザーが予定を動かす・消すことで指示を返す**ための面である。トリガの時計、約束の正本、権限の根拠にはしない。Calendar が到達不能でも執事は遅刻しない。

この段階の利用者は開発者本人。default OFF。Gmail / Drive / Discord は本計画の範囲外とし、§10 に次段の判断だけを残す。

## 2. 段階の整理

| 段階 | 今回の扱い | 成果 |
| --- | --- | --- |
| SL-A 期限台帳と tick | 本書で実装 | `schedule_entries`、tick、再起動復元、Situation 保留、発火→既存 Task/通知 |
| SL-B Calendar 投影 | 本書で実装 | 専用カレンダーへの outbox 型投影、状態別の見え方、忘却連動 |
| SL-C Calendar 観測 | 本書で実装 | syncToken 増分取得、差分分類、候補としての台帳反映、競合規則 |
| SL-D 保留ダイジェスト | 本書で実装（最小） | 会議中に保留した発火を終了時に束ねて一通で伝える |
| Gmail センサー / Discord チャネル | 次の独立計画 | 約束候補の抽出、外出中の連絡 |

SL-A は Google 接続なしで単独完結し、単独で価値を持つ。SL-B/C は SL-A の上に載せる。SL-A を飛ばして Calendar のイベント時刻を契機にする実装は禁止する。

## 3. 実装調査で分かった注意点

確認基準は 2026-09-20 の作業ツリー。並行作業（M3A、Situation、最小循環）が進行中のため、着手時に HEAD と dirty 差分を SL-00 で記録する。

- DB 書き込みの owner は `persistence/sqlite/writer.rs` の単一 `SqliteWriter`。本計画の全テーブルも同じ writer を通す。配送ワーカーや tick に第2 writer を作らない。
- `memory/personal_state/scheduler.rs` は `foreground()` / `blocking_generation()` / `interrupt()` で共有 slot を提供する。tick はこの slot を尊重し、ユーザーの会話 generation 中に重い処理を割り込ませない。
- outbox 型配送は `memory/personal_state/{managed,product,materializer,product_cleanup}.rs` に既存実装がある。Calendar 投影は同じ形（同一 transaction で pending 登録、ワーカーが拾う、指数バックオフ、多層 cleanup）を踏襲し、汎用化はしない。
- 忘却は既存 forget journal（DB 外の opaque ID と時刻）を通す。本計画で本文を持つ列は `payload_id` 経由の消去可能 payload と `calendar_observations.remote_summary` の二つ。両方を journal 対象にする。
- 認証情報は `credentials.rs` の Keychain 経路（service `com.saaa.provider-api-key` 相当）に載せる。OAuth の refresh token を SQLite・設定 JSON・backup・diagnostics へ書かない。macOS 以外は Calendar 接続を unsupported とする。
- Situation の `ShadowDecision`（`situation/classifier.rs`）は現時点で runtime から参照されていない。並行作業の Step 3 が TTS 抑止で最初の参照を作る。tick の Hold 判定はその同じ判定関数を使い、独自の会議判定を持たない。
- Goal / Delegation の型は並行作業（`saaa-minimal-loop-plan.md`、着手時に存在を確認）で定義される。存在しない場合、`scope_ref` は既存 `runtime/context/scope.rs` の Scope 参照、`delegation_ref` は文字列参照のまま NULL 許容で進め、型の確定後に外部キー相当の検査を追加する。時刻の到来が権限にならないという意味論は型の有無に依存させない。
- 既存 IPC 型を変更するため `bun run ipc:generate` が必要。新規 Rust ファイルは `bun run size:check` の baseline に登録する。

## 4. 範囲と配置

| 責務 | 予定ファイル |
| --- | --- |
| 台帳の型、状態遷移、CAS 更新 | `src-tauri/src/schedule/ledger.rs`（新規） |
| tick、発火判定、再起動復元 | `src-tauri/src/schedule/tick.rs`（新規） |
| 発火時の「する／待つ／聞く／しない」 | `src-tauri/src/schedule/decide.rs`（新規） |
| 保留キューとダイジェスト | `src-tauri/src/schedule/hold.rs`（新規） |
| DDL と migration | `src-tauri/src/schedule/schema.sql`（新規）、`persistence/migrate.rs` への加算 |
| Calendar HTTP adapter（etag、syncToken、extendedProperties） | `src-tauri/src/schedule/calendar/client.rs`（新規） |
| 投影 outbox とワーカー | `src-tauri/src/schedule/calendar/projection.rs`（新規） |
| 観測取得と差分分類 | `src-tauri/src/schedule/calendar/observe.rs`（新規） |
| 観測の台帳反映と競合規則 | `src-tauri/src/schedule/calendar/reconcile.rs`（新規） |
| OAuth（PKCE、Keychain 保存、refresh） | `src-tauri/src/schedule/calendar/auth.rs`（新規） |
| module 登録、AppState への handle 追加 | `src-tauri/src/lib.rs`、`app_state.rs` |
| IPC（一覧、手動登録、撤回、接続状態） | `src-tauri/src/ipc_contract/`、`src/lib/generated/` |
| 設定 UI（有効化、カレンダー選択、接続） | `src/features/settings/`（既存 Section 形式） |

Calendar client は MCP ではなく専用 adapter とする。etag・syncToken・extendedProperties を汎用 MCP ツール経由で制御しにくいため。Personal State の payload、World、Tool registry、Provider protocol は変更しない。

## 5. 固定する設計判断

1. 時計は SAAA。tick は 30〜60 秒間隔、スリープ復帰時と起動時に即時一回。秒精度は保証しない。
2. `schedule_entries` の時刻変更は UPDATE しない。新 entry を作り旧を `superseded` にし `supersedes` で繋ぐ。revision の系列が観測との突合に必要。
3. `status` は単方向。`scheduled → firing → fired | missed`。任意時点から `withdrawn | superseded`。`fired` を `scheduled` へ戻さない。
4. `delegation_ref` が NULL の entry は発火しても実行せず「聞く」に落ちる。時刻の到来を権限にしない。
5. 発火の成功（`fired`）と仕事の完了（Task の done）は別の台帳の別の状態。tick は Task の完了を書かない。
6. 投影は outbox 型で冪等。イベントに `saaa_entry_id` / `saaa_rev` / `saaa_hash` を `extendedProperties.private` として埋め、作成前クラッシュは検索で回復し二重作成しない。更新は `If-Match: etag`。
7. Calendar 上の `fired` / `withdrawn` イベントは削除せず記号で残す。`superseded` は新イベントへ差し替え旧を削除。忘却のみ物理削除または本文なしタイトルへ置換。
8. 観測は直接 entry を書かない。`calendar_observations` に入れ、`reconcile` が候補として反映する。
9. 競合規則。時刻はユーザー優先（SAAA 側 revision が観測の `saaa_rev` より新しい場合のみ SAAA 優先＋一回通知）。削除はユーザー優先だが委任 Task は確認。本文は SAAA 優先でユーザー編集は抽出候補。`fired` / `withdrawn` は SAAA のみ変更可。`foreign_event` は常に確認。
10. 自分の投影のエコーを観測として再処理しない。投影直後の etag を保存し一致なら `unchanged`。
11. Calendar / Docs に置くのは執事の予定・判断・要約のみ。会話原文・機微な記憶本文は参照 ID にとどめ、既存分類 confidential / restricted は送信しない。
12. default OFF。Calendar 接続 OFF でも SL-A は動く。無効化時は tick を止め、`firing` の entry を `error:disabled` で閉じ、投影は残す。

## 6. 台帳の形

DDL は SL-02 で確定する。列名は予定。

```sql
CREATE TABLE schedule_entries (
  id             TEXT PRIMARY KEY,
  kind           TEXT NOT NULL,      -- task_run | reminder | check_in | digest | hold_until
  subject_ref    TEXT NOT NULL,      -- goal: / task: / commitment: / conversation:
  scope_ref      TEXT NOT NULL,
  due_at         INTEGER NOT NULL,
  window_end_at  INTEGER,
  status         TEXT NOT NULL,      -- scheduled | firing | fired | missed | withdrawn | superseded
  origin         TEXT NOT NULL,      -- user_explicit | delegation | planner_candidate | user_calendar_edit
  delegation_ref TEXT,
  revision       INTEGER NOT NULL DEFAULT 1,
  supersedes     TEXT,
  created_at     INTEGER NOT NULL,
  fired_at       INTEGER,
  fire_result    TEXT,               -- started | deferred | suppressed_meeting | no_delegation | error:<code>
  payload_id     TEXT
);
CREATE INDEX schedule_due ON schedule_entries(status, due_at);

CREATE TABLE calendar_projections (
  entry_id        TEXT PRIMARY KEY,
  calendar_id     TEXT NOT NULL,
  event_id        TEXT,
  projected_rev   INTEGER NOT NULL,
  projected_hash  TEXT NOT NULL,
  remote_etag     TEXT,
  remote_updated  INTEGER,
  state           TEXT NOT NULL,     -- pending | synced | stale | remote_deleted | error
  attempts        INTEGER NOT NULL DEFAULT 0,
  next_attempt_at INTEGER
);

CREATE TABLE calendar_observations (
  id              TEXT PRIMARY KEY,
  event_id        TEXT NOT NULL,
  entry_id        TEXT,
  observed_at     INTEGER NOT NULL,
  remote_etag     TEXT NOT NULL,
  remote_start    INTEGER,
  remote_end      INTEGER,
  remote_status   TEXT,              -- confirmed | cancelled
  remote_summary  TEXT,              -- 消去可能。忘却対象
  diff_kind       TEXT NOT NULL,     -- moved | deleted | title_edited | foreign_event | unchanged
  handled         TEXT NOT NULL      -- pending | applied | asked | ignored | conflict
);

CREATE TABLE calendar_sync_state (
  calendar_id     TEXT PRIMARY KEY,
  sync_token      TEXT,
  last_full_sync  INTEGER,
  last_error      TEXT
);
```

## 7. tick と発火

```text
tick(now):
  1. reader: status='scheduled' AND due_at<=now を due_at 順に最大32件
  2. writer CAS: status='scheduled' AND revision=? のときだけ firing へ。失敗はスキップ
  3. decide(entry, situation, delegation):
       Act        → 既存 Task Runtime / 通知へ。fire_result=started
       Hold(until)→ kind=hold_until の新 entry を作る。fire_result=deferred
       Ask        → delegation_ref NULL。候補通知。fire_result=no_delegation
       Drop(why)  → fire_result=suppressed_*
  4. writer: status='fired'
  5. writer: window_end_at<now の scheduled を missed へ
```

再起動時は tick を一回走らせるだけで復元する。`firing` のまま残っている entry は `error:crashed` で閉じ、外部操作を伴う Task だった場合は再実行せず状態照会へ回す（既存 coding ジョブの再開規則に従う）。

Hold は Situation が MEETING かつ attention が IGNORE / OBSERVE の場合に限る。会議終了（hysteresis 経過）で `hold_until` の entry がまとめて発火し、SL-D が一通のダイジェストに束ねる。

## 8. 合格基準

| ID | 条件 | 期待値 |
| --- | --- | --- |
| A | due_at 到来、delegation あり、Situation 通常 | 一回だけ発火し Task/通知が一件。二重発火 0 |
| B | due_at 到来、delegation_ref NULL | 実行 0、候補通知 1、fire_result=no_delegation |
| C | 会議中に due_at 到来 | 実行 0、hold_until 生成、会議終了後にダイジェスト 1 通に束ねる |
| D | 停止中に due_at と window_end_at を通過 | 起動時に missed。実行 0、記録 1 |
| E | firing 中にクラッシュ → 再起動 | error:crashed で閉じる。外部操作 Task は再実行 0 |
| F | 時刻変更 | 新 entry + superseded。旧 entry の発火 0 |
| G | 撤回後に due_at 到来 | 発火 0 |
| H | 投影: 作成前クラッシュ → 再実行 | Calendar 上のイベント 1 件（二重作成 0） |
| I | 投影: 同じ entry を 100 回投影 | API 呼出しは差分がある時のみ。hash 一致なら 0 |
| J | 観測: ユーザーが時刻を動かす | 新 entry（origin=user_calendar_edit）、旧 superseded、次回観測で unchanged |
| K | 観測: ユーザーが委任 Task のイベントを削除 | withdrawn せず確認 1 件。確認まで発火は保留 |
| L | 観測: SAAA が先に変更、ユーザーも変更 | SAAA 優先、通知 1 回、以後の観測で再通知 0 |
| M | 観測: 専用カレンダーに直接書かれた予定 | foreign_event、確認 1 回。No 後は ignored で再質問 0 |
| N | 投影のエコー | unchanged。台帳変更 0 |
| O | 忘却 | payload と remote_summary を消去、Calendar 側は再投影で本文なし。復元後も再出現 0 |
| P | Calendar 到達不能 / トークン失効 | tick は継続し発火する。投影は pending 蓄積、通知 1 回 |
| Q | 専用カレンダー削除 | 全 projection を remote_deleted、再作成後に全件再投影 |
| R | 機能 OFF | tick 停止、firing を error:disabled で閉じる。DB 上の entry は保持 |
| S | confidential / restricted 分類の subject | Calendar へ本文 0。参照 ID のみ |

## 9. 検証と測定

```sh
cargo test --locked --manifest-path src-tauri/Cargo.toml --lib schedule::
cargo test --locked --manifest-path src-tauri/Cargo.toml --lib sl_
bun run ipc:check
bun run size:check
bunx --bun spec-html check ./spec/docs --warnings-as-errors
bun run check:local
```

Calendar API は試験では fake HTTP サーバー（etag / syncToken / extendedProperties / 409 / 410 Gone を再現）で置き換える。実 Google アカウントへの live 確認は SL-24 で一回だけ行い、結果を evidence に残す。live 未実施のまま SL-B/C を完了扱いしない。

tick の追加負荷は 1,000 entry・due 32 件で p95 <= 15 ms（DB 読み書きのみ、Provider 呼出しなし）。投影ワーカーは 1 tick あたり最大 8 API 呼出し。数値は開発ゲートであり Google 側の遅延を保証しない。

対象 0 件は不合格。コンパイル失敗、試験不合格、未実施を区別する。並行作業のコードを無断で巻き戻さない。

## 10. 次段へ渡す判断

1. **Discord 連絡チャネル**: 外出中の連絡は Gmail より Discord DM が適する（即時性、Bot 権限の狭さ、送信者の自明さ）。本計画の outbox / observation の型をそのまま流用できる。独立計画で扱う。
2. **Gmail センサー**: あなた宛てメールから約束・締切候補を抽出し `schedule_entries` の候補にする。契約サービス系は無視。送信は下書きまで、委任付き送信は後段。独立計画で扱う。
3. **執事の作業メモ（Google Docs 一枚）**: 「あなたについて今考えていること」の永続化と訂正入口。Personal State の assertion からの投影として、本計画の projection 型で扱えるかを検討する。
4. **Assist Planner**: `origin=planner_candidate` の entry を作る主体。本計画では型だけ用意し、候補生成は行わない。
5. **アカウント**: 一般アカウントを使う。正本が SAAA にあるため、後でアカウントを切り替えても再投影で復元できる。

## 11. 完了記録

実装時に `spec/evidence/schedule-ledger/progress.md` と `results.md` を作成する。各カードの変更関数、試験名・件数、期待値、未実施、A〜S 対応、live 確認の有無を記録する。本文・予定名・メールアドレスをログへ残さず、合成 fixture の出力例だけを保存する。

完了条件は 26 カード（SL-00〜25）と A〜S、全体ゲート、live 確認 1 回、default OFF の維持。Gmail / Discord / Docs / Planner は未実装として引き渡す。
