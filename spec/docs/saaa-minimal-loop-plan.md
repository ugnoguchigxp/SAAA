# SAAA 最小執事循環 実装計画

作成日: 2026-09-20。状態: Step 4 の実装計画。コードは本計画の作成では変更しない。上位は[執事循環フェーズ](saaa-steward-loop-phase-plan.md)と[Concept §16](saaa-personal-ai-concept.md)。カードは本書 §8（14枚）。

着手は Step 3 完了後。先行実装しない。

## 1. 次に完成させるもの

ユーザーが明示登録した一件の Goal に対し、指定 Git ワークスペースのテスト失敗を許可範囲で調べ、結果を一メッセージで報告する。別話題へ移っても仕事は続き、委任を撤回すれば止まり、再起動後は未完了を一覧して状態照会する。

最初の循環に含めない: 自動修正、複数 Goal、定期ポーリング、LLM による Goal 推定、新しい実行デーモン。

## 2. 正本の置き方

| 責務 | 正本 | 派生物 |
| --- | --- | --- |
| Goal（望む未来） | 新規 `steward_goals`（履歴追加。状態変更は行追加） | World の Goal ノードへ自動転記しない |
| Delegation | 新規 `steward_delegations` | Situation や World から権限を作らない |
| Task 識別と循環状態 | 新規 `steward_tasks` | 実行そのものは持たない |
| 実行 | 既存 `coding_jobs` / `coding_runs` / `runtime_runs` | 再起動 reconcilation は既存 `coding/recovery.rs` |
| 会議中の通知保留 | Situation の最新判定（Step 3 の読取口） | 分類器変更なし |

会話 Runtime は分岐しない。Goal / Delegation / Task は Turn Orchestrator への Reducer（明示トリガ、Coding 前景遷移）と、完了時の一メッセージ投入として接続する。Context Broker へ新しい命令候補を増やして権限を与えない。

## 3. 範囲

追加するもの:

- schema version を現行 25 から加算した migration。既存 migration 関数は変更しない。
- Goal / Delegation / Task の内部API。公開 IPC は最小（登録・撤回・一覧）。本文を Settings へ複製しない。
- 明示発話「テストを確認して」を既存 turn 経路で検出する Reducer。自由文からの広範抽出はしない。固定語と既存 tool `coding_*` の再利用に閉じる。
- Situation `foreground.category == Coding` への遷移を観測し、有効な Delegation があるときだけ inspect 相当を既存 coding 経路へ載せる。遷移自体を許可とはみなさない。
- 重複キー（workspace + 失敗シグネチャ）。同一キーの queued/running Task があるとき新規作成しない。
- 報告メッセージ。会議中（Step 3 と同条件）は保留キューへ載せ、復帰後に1通。

含めないもの:

- coding とは別の job runner、別 SQLite、TypeScript 永続化。
- 書き込み・パッチ適用・commit の許可。
- World 五要素の更新を Goal 達成とみなす処理。
- Role Routing。

## 4. Goal / Delegation / Task の型

Goal:

```text
id, scope_key, origin (user_explicit),
status (active | paused | withdrawn),
success_condition (text, ユーザー記入),
created_at, superseded_by (nullable)
```

status の変更は UPDATE ではなく新行 + 旧行の superseded_by。忘却は既存 forget journal。

Delegation:

```text
id, goal_id, workspace_id,
ops (read | test_run のみ。bit ではなく固定enum),
budget_runs, budget_ms, notify (on_complete | on_failure | both),
status (active | withdrawn),
withdrawn_at
```

撤回は即時。running の coding job は既存 `coding_cancel` 相当。撤回後の新規 start 0件。再開要求だけで withdrawn を active に戻さない。

Task:

```text
id, goal_id, delegation_id, coding_job_id (nullable until start),
loop_state (queued | running | awaiting_user | done | failed | cancelled),
dedupe_key, report_json (結果要約。モデルconfidenceなし),
held_reason (nullable: meeting)
```

`loop_state` は循環の識別であり、coding の process 状態の複製ではない。coding 側の語彙との写像は §5。

Feature default OFF。`SAAA_MEMORY_ENABLED=1` かつ既存 Memory 設定配下の steward flag（default false）が立ったときだけ Reducer が動く。OFF でも登録済み行の撤回と forget は動かす。

## 5. 既存 coding への写像

| loop_state | coding_jobs / coding_runs | 再起動後 |
| --- | --- | --- |
| queued | job `queued`、run 未 start または start 前 | 一覧に出し、ユーザー「続きを」で start |
| running | run `starting` / `running` | recovery が live PID を見て outcome_unknown または interrupted。不明なら再実行せず inspect |
| awaiting_user | 実行は止める。質問は report に書く | 再実行しない |
| done | run `settled` かつ complete が boolean true のものだけ。モデル申告は無視 | 完了のまま |
| failed | `failed`、または settled でも complete が true でない | 再実行しない |
| cancelled | `interrupted` / cancel_requested の確定 | 撤回済みとして報告 |

Codex は既存 read-only sandbox。許可操作が read と test_run を超える request 文字列は start 前に拒否する。L2 相当を超える権限モデルは作らない。

coding_jobs は `conversation_id` とユーザーメッセージ `source_id` を持つ。循環 Task は PRIMARY 会話または明示 Scope の既存会話に載せる。Situation の Coding 遷移だけで source メッセージを偽造して権限を作らない。トリガは (1) ユーザー明示、(2) 既に有効な Delegation がある前景遷移、の論理積に近い形とし、(2) 単独では start せず inspect に限る。

(2) だけで start が必要になった場合は実装を止め、代替案を相談する。代替案の候補:

1. inspect のみ自動、start は明示トリガ必須（本計画の既定）。
2. 事前にユーザーが「この workspace を監視して」と登録した Delegation があるときだけ、Coding 遷移で queued の既存 Task を start する。新規 Task は作らない。

新しい runner は代替にしない。

## 6. 受入シナリオ（実DB、自動化）

ロードマップ §11 の統合受入を一件に落とす。

1. Goal を明示登録する（テスト失敗の調査。Scope は指定 workspace / Project）。
2. 別話題へ会話を切り替える。
3. 許可範囲で Task が完了し、通知条件に従って1メッセージが届く（会議中なら保留、終了後に届く）。
4. 委任を撤回する。
5. プロセス相当の再起動（同じ実DBを開き直す）。
6. 「続きを」で、撤回済みなら復活させず撤回を報告する。撤回していない分岐では正しい Task を inspect して再開する。

合格: Scope 漏洩 0、同じ失敗の重複 Task 0、撤回後の新規実行 0。進行中への撤回と遅延結果は別試験。

## 7. 検証コマンド

```sh
cargo test --locked --manifest-path src-tauri/Cargo.toml --lib sl_
cargo test --locked --manifest-path src-tauri/Cargo.toml --lib ml_
bun run check:personal-state
bun run size:check
bun run ipc:generate
bun run spec:check
bun run check:local
```

性能ゲートは新設しない。既存 coding と Broker の予算を緩めない。live Codex / 実テスト失敗の再現は live未検証 として分ける。offline は合成失敗シグネチャと fixture workspace で回す。

## 8. 作業カード（14枚）

| ID | 対象 | 実装すること | 合格条件 |
| --- | --- | --- | --- |
| ML-00 | spec/evidence/steward-loop/progress.md | schema 25、coding 状態語彙、recovery 方針、Memory flag を記録 | 既存 migration を書き換えない方針を明記 |
| ML-01 | persistence/schema.rs 加算 | steward_goals と履歴。既存 version 分岐は追加のみ | 旧DBが開き、Goal 0件。上書きUPDATEなし |
| ML-02 | 同上 | steward_delegations。ops は read/test_run のみ CHECK | 不正ops挿入0。撤回行が即時 status=withdrawn |
| ML-03 | 同上 | steward_tasks。coding_job_id と dedupe_key。loop_state CHECK | 同一 dedupe_key の active 重複 INSERT 0 |
| ML-04 | 内部 repository（SqliteWriter 経由） | Goal/Delegation/Task の登録・一覧・撤回。第2 writer なし | forget journal 経由の忘却試験。直接DELETEなし |
| ML-05 | Reducer（既存 turns 近傍、新経路なし） | 固定トリガ「テストを確認して」→ queued Task。flag OFF なら no-op | 通常会話の命令位置1のまま。別 Runtime 分岐0 |
| ML-06 | Situation 読取（Step 3 API 再利用） | foreground=Coding 遷移で、active Delegation がある Task だけ inspect | 遷移のみで start 0、権限導出0 |
| ML-07 | coding 接続 | start は read/test 範囲の既存 API。budget 超過は start しない | 書込みツール呼出し0。budget 後は awaiting_user または failed |
| ML-08 | 重複抑止 | 失敗シグネチャ + workspace の dedupe | 同一失敗2回観測でも Task 1 |
| ML-09 | 報告 | done/failed/cancelled で1メッセージ。会議中は held_reason=meeting | Step 3 条件中に TTS 0。復帰後1通 |
| ML-10 | 撤回 | Delegation withdrawn → coding_cancel、以後 start 0 | 「続きを」で復活0 |
| ML-11 | 再起動 | 未完了一覧。outcome_unknown は inspect。再実行0 | recovery 既存経路を使う。独自 PID 操作0 |
| ML-12 | 受入試験（実DB） | §6 シナリオと漏洩/重複/撤回後実行 | 3条件すべて0件。件数>0 |
| ML-13 | size、ipc、check:local、results.md | baseline 登録。live未検証を分離 | 閾値緩和0。生成品質を完了扱いしない |

ML-05 の固定トリガ以外の自然文理解は実装漏れとして残してよい。
