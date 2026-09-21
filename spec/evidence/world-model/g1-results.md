# Personal World Model G1 完了記録

作成日: 2026-09-21。状態: 本番compose・実HTTP・性能のoffline受入まで補完。最新結果は `m4a-results.md`。
計画は `spec/docs/saaa-personal-world-model-graph-answer-plan.md`。カード別は `g1-progress.md`。

## 1. 結論

明示 Project がある会話で、固定四形式の graph 質問が保存済み現在入力から解決され、認可済み
`ExactName` 一件だけを seed として五要素 `WorldSliceV2` を取得し、通常の 1 Candidate として
Broker へ渡る。未知・曖昧・投影 stale・pending・容量超過は空の成功知識ではなく、固定 notice を
持つ空 Frame として同じ JSON wrapper で返す。雑談・Memory OFF・Project 0/複数では graph 取得を行わない。

`graph_request` を本番 `turn.rs` に渡すのは `compose_for_app` だけであり、その値は
`runtime_runs.input_message_id` の保存入力から `parse_graph_question` を通して作る。provider へ渡す
現在のユーザー指示は parser 用に書き換えない。parser の結果を権限・Scope・Tool 権限として扱わない。

## 2. ゲート（対象 suite）

| ゲート | 結果 |
| --- | --- |
| `cargo test --lib world_g1_` | 18 passed / 0 failed |
| `cargo test --lib runtime::context::world` | 53 passed / 0 failed / 1 ignored |
| `cargo test --lib memory::personal_state::world` | 110 passed / 0 failed / 2 ignored |
| `cargo test --lib providers::chat_completions` | 18 passed / 0 failed |
| `cargo test --lib providers::agent_session` | 16 passed / 0 failed / 1 ignored |
| `bun run check:personal-state` | passed |
| `bun run spec:check` | passed |
| `bun run test:rust-packages` | passed |
| `bun run size:check` | 自分の担当ファイルは閾値内（新規 baseline 登録済み）。並行 `role_routing`/`schedule` のみ失敗 |
| `cargo clippy --all-targets -- -D warnings` | 自分の担当ファイルは 0 error。並行ファイルのみ失敗 |
| `cargo fmt --check` | 自分の担当ファイルは clean。並行 `runtime/pi/tests.rs` のみ差分 |

`runtime::context::world` は 53 passed / 0 failed。並行の World Delivery Completion 計画が
AgentSession を「初回のみ再検証付き World、followup は World-free」へ確定し、`m3b_05` もその契約へ
更新された。filter 0 件を成功扱いしていない。

## 3. 固定質問形式（利用者が試せる形）

外側空白を trim し、次に文字列全体が一致した場合だけ質問として扱う。末尾は `？` または `?` のいずれか一つ。

| 入力 | intent |
| --- | --- |
| `「<topic>」は今の目標にどう関係しますか？` | Relevance |
| `「<topic>」は何に影響しますか？` | Influence |
| `「<topic>」にはどんな相関がありますか？` | Correlation |
| `「<topic>」は何に依存しますか？` | Dependency |

- `<topic>` は trim 後 1〜160 UTF-8 byte。制御文字・改行・内側の `「」` を拒否。
- 形式が一致しない一般質問・引用を含む長文・複数対象は `NotRequested`（graph 取得なし）。
- 外形が一致して topic が不正なら `Invalid`（graph 取得なし）。
- 入力全体が 2,048 byte 超なら走査せず `NotRequested`。

必要な条件: 明示 Project が resolved scope に 1 つだけあること、Memory が有効であること。
topic は既存 `query_v2` の許可済み Entity 集合の name または alias に一意一致すること。

## 4. C5 の空 Frame 区別

`render_world_frame_explicit` は、明示質問のときだけ、次を持つ空 Frame を既存 JSON wrapper で返す。

- graph 内 `unknown_seed` / `ambiguous_seed`
- Frame 内 `world_projection_stale` / `world_pending` / `world_capacity_omitted`

通常 shadow の `empty_frame` 判定は変更していない。Scope 不許可・DB 破損は空 Frame へ変換せず、
既存の省略/エラー境界を維持する。

## 5. 2026-09-21 再レビューでの補完

- M4A runnerを実装し、五要素・unknown・Goal/Source/Relation失効を実HTTPで検証した。
- 保存入力のDB読取から `compose_for_app` 相当の本体を通る統合試験を追加した。
- Goal有効化後のfixtureへ訂正し、byte上限付近で省略理由によってグラフ全体が落ちる問題を修正した。
- 元TTLの再検証、本文に存在しないWorldの送信済み記録、AgentSession構築境界、MCPの二重掲載/receiptを修正した。
- 実時計の100投影/2,000ledgerでG1性能基準を通過した。数値は `m4a-results.md`。
- AgentSessionの契約はWDの初回限定を正本として計画を更新した。

上の旧ゲート表は初回実装時の履歴。最新件数・未検証範囲は `m4a-results.md` と `review-2026-09-21.md` を正本とする。実モデル回答品質・WD全経路受入は未認定。
