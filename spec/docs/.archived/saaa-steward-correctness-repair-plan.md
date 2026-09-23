# Steward 整合性の改修計画

作成日: 2026-09-22。状態: 未着手。対象: 委任仕事の台帳、起動順、実行依頼、登録済み時間予算、報告配送、forget、撤回。

本書は [委任残件計画](saaa-delegated-work-repair-terra-plan.md) の後続である。`spec/evidence/delegated-work/repair-progress.md` が DWR カードを完了としていても、下に書いた関数の現状は別である。完了行を、この計画の受入に使わない。

## 1. 方針

権限の追加拒否、行動の抑制、ディスク読み取りの縮小はしない。次は変更しない。

| 触らないもの | 理由 |
| --- | --- |
| `runtime/pi/process.rs` の Seatbelt（`file-read*`、`process*`）と Codex SDK 親プロセスの置き方 | ディスク読み取りを狭めない |
| `steward/request_intent.rs` の許可・拒否句 | 権限判定を厳しくしない |
| `scope_epoch` を dispatch や runner の停止条件に足すこと | Scope 撤回で実行を新たに止めない |
| `source_now_forbids` のダイジェスト取得失敗を拒否へ変えること | 失敗時に実行を新たに止めない |
| DWR-08 / DWR-09 / DWR-10 の scoped read と固定 argv | 操作集合を狭める計画であり、本書の対象外 |

直すのは、保存した仕事が別の仕事と混ざる、画面の操作が起動に届かない、頼んだ本文が実行に渡らない、登録した `budget_ms` が走っているプロセスに届かない、報告の挿入と配送済み印が分かれる、という不整合である。新しい上限や拒否理由は足さない。

既存の Coding service、Pi runner、SQLite writer、steward の module を使う。第二の実行系は作らない。他作業の未コミット差分は消さない。共有ファイルは編集直前に読み直す。

## 2. いまのコードで残っている欠陥

確認日は 2026-09-22。行番号は動くので、関数名を正とする。

| ID | 欠陥 | 根拠 |
| --- | --- | --- |
| C1 | 報告 message の挿入と配送済み更新が同一トランザクションではない | `steward/outbox.rs` の `flush_unflushed` は複数 `execute` を行う。`steward/report.rs` の `flush_held_reports` と `steward/reduce.rs` の `publish` 呼び出しは `SqliteWriter::write` である。`write` は BEGIN しない。`transact` だけが `unchecked_transaction` を張る |
| C2 | 登録した時間予算が実行中のプロセスを止めない | `runtime/pi/runner.rs` の待受期限は `Duration::from_secs(1800)`。`steward/budget.rs` の `remaining_deadline_ms` は `steward/tests/dwr.rs` 以外から呼ばれない。その SQL は `coding_runs.started_at`（`now_iso` のミリ秒文字列）を `julianday` に渡す。SQLite の `julianday` はその文字列を日付と解釈せず、消費時間は 0 になる。`dispatch` の `budget_exceeded` は次の起動前だけを見る |
| C3 | 実行へ渡る依頼文が Goal の内容ではない | `steward/queue.rs` の `request_for_task` は read / test_run ごとに固定英語を返す。`summary` も対象も引かない |
| C4 | 画面の並び替えが起動順にならない | `WorkPage` は `reorder_steward_queue` で `queue_rank` を書く。本番の `steward/queue.rs` `next_eligible` は `ORDER BY t.rowid`。`queue_rank` を見る `next_queued_work` はテストからしか呼ばれない |
| C5 | forget が同じ会話の未配送報告まで消す | `steward/invalidation.rs` の `forget_source` は、その source の task に加え、`conversation_messages.id` から引いた `conversation_id` の未配送報告全部を `invalidated` にする |
| C6 | Goal を指定しない撤回が、最新 Goal の閉鎖と会話内ジョブの取消で食い違う | `repository.rs` の `withdraw` は `latest_work`（`ORDER BY g.rowid DESC LIMIT 1`）だけを superseded する。`withdraw_steward_delegation` はその前に `active_job_ids` で会話内の実行中ジョブを cancel する。Goal 指定は別関数 `withdraw_goal` |

## 3. 完成条件

- 同じ報告 revision の会話 message は、配送境界の途中失敗のあと再openしても 1 通である。
- 委任に保存された `budget_ms` が、その委任の実行中プロセスの停止期限になる。固定 1800 秒へ置き換わらない。Seatbelt と tool 許可は変えない。
- runner に渡る依頼文に、その Goal の `summary` と step の recipe が入る。
- 同一会話で queued の並びを変えると、次に `next_eligible` が選ぶ task が変わる。
- 一つの source の forget は、その source に紐づく未配送報告だけを消す。同じ会話の別 Goal の未配送報告は残る。
- 撤回は Goal ID を必須にする。指定した Goal の task と job だけが対象になり、別 Goal の task 状態と job は残る。`withdraw` と `withdraw_goal` の二重意味を残さない。

## 4. カード

推奨順は SC-00 → SC-01 → SC-02 → SC-03 → SC-04 → SC-05 → SC-06 → SC-07。SC-02 と SC-03、SC-04 と SC-05 は依存がなければ入れ替えてよい。1 カードは原則 1 つの動作変更とし、実装ファイルは 5 つ程度までとする。テストは別枠。`repository.rs` へ処理を足し続けず、既にある `outbox` / `budget` / `queue` / `invalidation` を使う。

証跡は `spec/evidence/steward-correctness/progress.md` と `results.md` に、カード ID、変更ファイル、実行したコマンド、成功件数、未実施を書く。未実施を成功に数えない。

### SC-00: 対象関数の再確認

- 依存: なし。
- 実施: 着手時の HEAD と dirty を記録する。C1〜C6 の関数を読み、本書の根拠がまだ成り立つかを確認する。既に直っていればそのカードを対象外と書き、別の欠陥を足さない。
- 完了: progress に、対象にするカードと外したカードが分かれている。

### SC-01: 報告配送を一つのトランザクションにする

- 依存: SC-00。対象: `steward/report.rs`、`steward/reduce.rs`、`steward/outbox.rs`、必要なら `persistence/sqlite/writer.rs` は読取りのみ。
- 実施: `flush_unflushed` を呼ぶ本番経路を `SqliteWriter::transact` に載せる。message 挿入、conversation 更新、`mark_flushed`、配送 cursor をそのトランザクションの中で終える。TTS と UI emit は commit の後のままにする。`outbox.rs` 先頭の「同一トランザクション」という文と、呼び出しが一致するようにする。
- 試験: `sc_01_flush_rolls_back_message_when_mark_flushed_fails`。挿入後・mark 前に失敗させ、再openで message が 0 または配送済み 1 通であり、未配送のまま message だけが増えていないことを確認する。
- 完了: 本番の `publish` と `flush_held_reports` が `write` のまま `flush_unflushed` を呼ばない。

### SC-02: 保存済み時間予算を実行中のプロセスへ渡す

- 依存: SC-00。対象: `steward/budget.rs`、`steward/dispatch.rs`、`runtime/pi/runner.rs`。`process.rs` の許可プロファイルは編集しない。
- 実施: `remaining_deadline_ms` をミリ秒文字列の差分で計算する。`julianday` をこの列に使わない。dispatch が runner へ残量を渡し、待受ループの固定 1800 秒をその残量に置き換える。残量が尽きたときは既存の abort 経路で子プロセスを止める。新しい予算種別は足さない。`budget_runs` の既存の消費規則は変えない。
- 試験: `sc_02_short_budget_stops_before_fixed_1800_seconds`。短い sleep fixture と小さい `budget_ms` で、1800 秒まで待たずに停止することを確認する。`sc_02_remaining_deadline_uses_elapsed_run_time` で、開始から経過したミリ秒が残量から引かれることを確認する。
- 完了: `runner.rs` に委任実行向けの固定 1800 秒期限が残っていない。`remaining_deadline_ms` の本番呼び出しが 1 つ以上ある。

### SC-03: 実行依頼に Goal の summary を載せる

- 依存: SC-00。対象: `steward/queue.rs`、依頼文を保存している `steward/dispatch.rs` または `repository.rs` の `persist_task_plan` 呼び出し。
- 実施: `request_for_task` が Goal の `summary` と step recipe を依頼文に含める。recipe ごとの「ファイルを変更しない」等の既存の一文は残してよい。許可判定や operations の集合は変えない。
- 試験: `sc_03_runner_request_contains_goal_summary`。登録した summary の一意な marker が、runner へ渡る payload に含まれることを確認する。
- 完了: 固定英語だけを返す分岐が、summary を無視したまま残っていない。

### SC-04: 起動順を queue_rank に合わせる

- 依存: SC-00。対象: `steward/queue.rs`。画面の並び替え IPC は既にあるので増やさない。
- 実施: `next_eligible` の並びを `t.queue_rank`、同点は `t.rowid` にする。会話をまたぐ選択でも、各会話の中の rank を崩さない。`next_eligible_at` の待機は維持する。新しい優先ゲートは足さない。
- 試験: `sc_04_reorder_changes_next_eligible_task`。同じ会話の queued を入れ替えたあと、dispatcher が選ぶ先頭が入れ替え結果と一致することを確認する。
- 完了: 本番の選択 SQL が `ORDER BY t.rowid` だけではない。

### SC-05: forget の無効化を source に限定する

- 依存: SC-00。対象: `steward/invalidation.rs`。
- 実施: 未配送報告の更新条件から、会話 ID 全体に広がる `OR` を外す。その source の task、またはその source に紐づく報告だけを無効化する。本文を空にする既存の扱いは、その対象に限って維持する。別 Goal の報告を残すことが、忘れた source の本文を復活させる条件になってはならない。
- 試験: `sc_05_forget_keeps_sibling_goal_report`。同じ会話に未配送報告を 2 件置き、一方の source だけを forget し、他方の digest が残ることを確認する。
- 完了: `conversation_id IN (SELECT conversation_id FROM conversation_messages ...)` による報告の一括無効化がない。

### SC-06: 撤回を Goal 指定の一つに揃える

- 依存: SC-00。対象: `steward/commands.rs`、`steward/repository.rs`、`src/features/coding/stewardApi.ts`、生成 IPC が必要ならその生成手順。
- 実施: ユーザー撤回は `withdraw_goal` に収束させる。`goal_id` 無しの `withdraw_steward_delegation` は、最新行だけを閉じつつ会話内ジョブを全部 cancel する組合せをやめる。呼び出し元が Goal を渡さない場合はエラーにし、暗黙に最新 Goal を選ばない。指定 Goal の cancel 対象は、その Goal の job に限る。別 Goal の `loop_state` と job は変えない。
- 試験: `sc_06_withdraw_one_goal_leaves_the_other_running`。A と B が active のとき A を撤回し、B の task と job が残ることを確認する。Goal 未指定の IPC は拒否されることを確認する。
- 完了: `withdraw` と `withdraw_goal` の両方が本番 IPC から違う意味で呼ばれない。

### SC-07: 回帰

- 依存: SC-01〜06 のうち実施したもの。
- 実施: `cargo test --manifest-path src-tauri/Cargo.toml sc_` と、既存の `cargo test --manifest-path src-tauri/Cargo.toml steward::`。触った Frontend があれば `bun test` の該当ファイルと `bun run ipc:check`。`bun run size:check` で触った module の予算超過を記録し、超過を隠すための上限引き上げはしない。
- 完了: 新規試験が 0 件成功ではない。既存 `steward::` の失敗があれば、今回の差分と分けて results に書く。

## 5. 検証で見ないもの

実モデル、実マイク、署名、公証、sleep/wake、ディスク外読み取りの拒否、自然文の拒否句の追加は、この計画の完了条件に入れない。それらが未実施でも SC-01〜06 は閉じられる。逆に、fixture が通ったことをもって DWR-11 や DWR-15 の過去の完了宣言を再掲しない。
