# Personal World Model G1 完了記録

作成日: 2026-09-21。状態: 実装・offline 試験完了（本番 compose 配線まで）。HTTP 本文レベルの受入と性能測定は未検証。
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

## 5. 未検証

- `world:eval` は存在しない（M4A runner 未実装）。G1 では類似 runner を新設していない。
- G1-09 は Broker の combined_block まで検証済み（graph JSON と notice が verbatim）。実 HTTP 受信本文は未検証。
- G1-10/11 の実 HTTP graph fixture 検証は未実施。
- G1-12 の性能測定（Frame/再検証 p95<=150ms、World 追加処理 p95<=350ms、Frame 最大<=500ms）は未実施。
- live Provider の回答品質は測定していない。固定応答 mock から品質改善を結論しない。
- AgentSession は WD 計画により初回のみ再検証付き World（followup は World-free）。G1 の『対象外 Provider へ送信 0』条件は満たさない。

## 6. 次の作業

1. M4A runner（G0 の 21 ケース）を確定し、G1-09/10/11 を実 HTTP で通す。
2. 性能測定を同一端末・debug で実施し、既存 p95 基準を維持することを確認する。
3. DynamicLan / reasoning MCP の World 契約を WD 計画と突き合わせて固定する。
