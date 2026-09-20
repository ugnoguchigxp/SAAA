# Butler Schedule Ledger 作業カード

作成日: 2026-09-20。全26枚、SL-00〜25。[全体計画](saaa-butler-schedule-ledger-plan.md)が正本。状態: **見送り中（準備未了。禁止ではない）**。最小循環の委任正本と TTS hold が揃ってから SL-A を着手する。

## 実行方法

各カードの前提は直前カードの完了。実装1〜3ファイル＋試験1〜2ファイルを標準とする。5実装ファイルを超える場合は先に枝番へ分割する。module登録・機械的importは付随変更可。カードごとのユーザー承認やcommitは必須にしない。

下表の `schedule/` は `src-tauri/src/schedule/`。新規ファイル名は予定。試験は `sl_NN_具体条件` と命名する。Calendar API は fake HTTP サーバーで置き換え、live 確認は SL-24 のみ。

順序は「基準 → 台帳 → tick → 判定 → 保留 → IPC → 認証 → 投影 → 観測 → 反映 → 忘却 → 統合 → 測定 → live → 記録」。SL-A（00〜10）は Google 接続なしで単独完結する。

## 作業一覧

| ID | 対象 | 計画 | 実装すること | 合格条件 |
| --- | --- | --- | --- | --- |
| SL-00 | spec/evidence/schedule-ledger/progress.md | §3 | HEAD、dirty差分、並行作業（M3A/Situation/最小循環）の状態、Goal/Delegation型の有無を記録 | 型が無い場合の暫定方針（文字列参照・NULL許容）を明記。並行変更を巻き戻さない |
| SL-01 | schedule/mod.rs、ledger.rs | §5-2,3,4 | Kind / Status / Origin / FireResult enum と Entry 型。状態遷移の許可表を純関数で定義 | fired→scheduled 等の逆遷移を型で拒否。superseded は supersedes 必須。delegation_ref NULL は Act 不可を表す |
| SL-02 | schedule/schema.sql、persistence/migrate.rs | §6 | 4テーブルと索引を現行 user_version+1 で追加。既存 migration は変更しない | 空DB・既存DBの両方で migration 成功。downgrade なし。移行前 backup 経路が動く |
| SL-03 | ledger.rs（writer 側） | §5-2,3 | insert、CAS 状態更新、supersede（新 entry＋旧 superseded を同一 transaction）、withdraw | revision 不一致の CAS は 0 行。supersede の片側だけ残る状態を作れない |
| SL-04 | ledger.rs（reader 側） | §7 | due 一覧（status/due_at 索引）、entry 取得、supersedes 系列の追跡 | read-only 接続・query_only。1,000 件で due 32 件取得 p95 <= 5 ms |
| SL-05 | schedule/tick.rs | §7 | tick 本体。claim（CAS）→ decide → fired。window_end_at 経過を missed へ | A / D / F / G。同一 entry の並行 tick で発火 1 |
| SL-06 | tick.rs | §7 | 起動時・スリープ復帰時の即時 tick。firing 残留を error:crashed で閉じる | E。外部操作 Task は再実行 0、状態照会経路へ渡す |
| SL-07 | schedule/decide.rs | §5-4, §7 | Act / Hold / Ask / Drop の判定。delegation_ref NULL → Ask。Situation MEETING かつ IGNORE/OBSERVE → Hold | B。Situation の判定は既存 ShadowDecision 経由で、独自の会議判定を持たない |
| SL-08 | schedule/hold.rs | §7, SL-D | Hold → kind=hold_until の新 entry。会議終了で束ねて一通のダイジェストを既存通知経路へ | C。ダイジェストは件数・件名参照のみ。本文をログへ出さない |
| SL-09 | tick.rs、personal_state/scheduler.rs 参照 | §3 | 共有 slot の尊重。foreground generation 中は tick の Act を次回へ繰り越す | 会話 generation 中に Task 起動 0。繰り越しは fire_result=deferred で記録 |
| SL-10 | ipc_contract/、lib.rs、src/lib/generated | §4 | IPC: schedule_list、schedule_add（user_explicit）、schedule_withdraw、schedule_status。`bun run ipc:generate` | zod 検証通過。本文は payload_id 経由。IPC から status を直接書けない |
| SL-11 | schedule/calendar/auth.rs、credentials.rs | §3 | OAuth PKCE、loopback redirect、refresh token を Keychain へ。scope は calendar のみ | SQLite・設定JSON・diagnostics に token 0。macOS 以外は unsupported |
| SL-12 | schedule/calendar/client.rs | §5-6 | events insert / patch(If-Match) / delete / list(syncToken) / privateExtendedProperty 検索。指数バックオフ | 412 → 競合として返す。410 Gone → full sync 要求を返す。本文をエラーログへ出さない |
| SL-13 | schedule/calendar/projection.rs | §5-6,7 | entry 書込みと同一 transaction で pending 登録。ワーカーが pending/stale を拾い投影。extendedProperties に entry_id / rev / hash | H / I。hash 一致で API 呼出し 0。作成前クラッシュは検索で回復し二重作成 0 |
| SL-14 | projection.rs | §5-7 | 状態別の見え方（fired ✓、withdrawn ✕、superseded 差替え）。忘却は本文なしタイトルまたは削除 | 状態ごとに一つの表現。superseded の旧イベント残留 0 |
| SL-15 | schedule/calendar/observe.rs | §5-8 | syncToken 増分取得、差分分類（moved / deleted / title_edited / foreign_event / unchanged）を calendar_observations へ | 分類は純関数で試験。extendedProperties なし → foreign_event |
| SL-16 | observe.rs | §5-10 | エコー判定。投影直後の etag と一致 → unchanged。410 → full sync と全件再照合 | N。エコーによる台帳変更 0 |
| SL-17 | schedule/calendar/reconcile.rs | §5-8,9 | moved → supersede（origin=user_calendar_edit）。deleted → withdraw、ただし委任 Task は確認へ | J / K。確認待ちの間は発火保留 |
| SL-18 | reconcile.rs | §5-9 | 競合（観測 saaa_rev < 現在 revision）→ SAAA 優先、一回だけ通知。title_edited → 既存抽出経路へ候補。foreign_event → 一回だけ確認 | L / M。同じ event の再通知・再質問 0 |
| SL-19 | ledger.rs、forget journal 接続 | §5-7,11 | payload_id と remote_summary を忘却対象へ。忘却で projection を stale にし再投影 | O。バックアップ復元後の再出現 0 |
| SL-20 | projection.rs、client.rs | §5-11 | 分類 confidential / restricted の subject は本文を投影せず参照 ID のみ | S。分類検査は既存 Personal State の分類を参照し独自判定を持たない |
| SL-21 | tick.rs、projection.rs、設定 | §5-12, §8 P/Q/R | 機能 OFF、到達不能、トークン失効、専用カレンダー削除の各振る舞い | P / Q / R。いずれも tick の発火は継続または明示停止し、黙って落とさない |
| SL-22 | src/features/settings/ScheduleSection.tsx（新規） | §4 | 有効化、Calendar 接続・切断、専用カレンダー選択、接続状態表示 | default OFF。token を UI state に保持しない。i18n ja/en |
| SL-23 | schedule/tests/*.rs、fake HTTP サーバー | §8, §9 | A〜S を実DB＋fake Calendar で統合。tick 1,000 entry p95 <= 15 ms、投影 1 tick 8 呼出し上限 | 全 ID に対応する試験名を記録。0 件通過禁止 |
| SL-24 | spec/evidence/schedule-ledger/live-YYYYMMDD.md | §9 | 実 Google 一般アカウントで作成・移動・削除・忘却・再接続を各 1 回 | 予定名・メールアドレスを記録しない。未実施項目を明示 |
| SL-25 | spec/evidence/schedule-ledger/results.md、size baseline | §11 | A〜S 対応、性能、live 結果、未実装（Gmail / Discord / Docs / Planner）を記録。size baseline 登録、閾値を緩めない | `bun run check:local` と spec-html 通過。default OFF 維持を明記 |

## 枝番の目安

- SL-13 が投影ワーカーとエンコードで 5 ファイルを超える場合は 13a（outbox 登録・ワーカー）と 13b（イベント本体の組立・hash）へ分ける。
- SL-17/18 が競合規則で膨らむ場合は規則ごとに純関数の試験を先に置き、reconcile 本体は薄く保つ。

## 禁止事項

- Calendar のイベント時刻を tick の契機にすること。
- 観測から `schedule_entries` を直接 UPDATE すること。
- `fired` / `withdrawn` を Calendar 側から変更すること。
- 第2 writer、独立 daemon、TypeScript 側永続化。
- token を Keychain 以外へ保存すること。
- 会話原文・機微本文を Calendar タイトル・説明へ書くこと。
- 数値ゲートを結果を見て緩めること。
