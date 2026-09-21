# Personal World Model G1 検証報告 — 保存済み五要素の回答接続

実施日: 2026-09-21。対象計画: `spec/docs/saaa-personal-world-model-graph-answer-plan.md`。
本報告は実装済みの範囲と未達を隠さず記録する。

## 1. 判定

| 区分 | 結果 |
| --- | --- |
| C1 固定質問 parser | 実装・offline 合格（四形式、`?`/`？`、空/超過/制御/複数/長文/Tool 風） |
| C2 正本入力 | 実装・offline 合格（running run・同 conversation・user/transcript のみ） |
| C3 名称解決 | 実装・offline 合格（本番 `SeedPolicy::ExplicitQuestionName` のみ ExactName 可。shadow 拒否維持） |
| C4 GraphRequest | 実装・offline 合格（seed 1・Forward・flags 全 true・limits 不変） |
| C5 空 Frame | 実装・offline 合格（unknown/ambiguous/stale/pending/capacity を区別、例外文漏洩 0） |
| C6 回答方針 | 実装・offline 合格（trusted template へ固定文言、system 1 件、データを system 化しない） |
| C7 送信・失効 | 既存 M4A 経路を再利用。graph fixture の実 HTTP 検証は未実施 |
| G0 / M4A | **部分的**。`m4a-results.md`・`world:eval` 無し。AgentSession は並行 WD 計画で『初回のみ再検証付き World』へ変更され、対象外送信 0 は満たさない |
| 全体 | 本番 compose 配線と offline 受入は閉じた。実 HTTP 本文・性能・live 品質は未検証 |

計画 11 章のとおり、未達を成功として報告しない。

## 2. 操作例と必要データ

明示 Project（resolved scope に 1 つ）と Memory ON の会話で、次のように入力する。

- `「Speculative Decoding」は今の目標にどう関係しますか？`
- `「Decode Latency」は何に影響しますか？`
- `「Voice Latency」にはどんな相関がありますか？`
- `「Decode Latency」は何に依存しますか？`

topic は既存 `query_v2` が解決できる Entity の name または alias に一意一致する必要がある。
一致しなければ `unknown_seed`、複数一致なら `ambiguous_seed` として、候補名を返さず空 Frame を返す。

## 3. C カード別の根拠

### C1（`runtime/context/world/question.rs`）

- `GraphQuestion { topic, intent }`、`QuestionParse = NotRequested | Invalid | Requested`。
- 外側 trim、末尾 `？`/`?` 一つ、四形式の完全一致のみ Requested。
- topic は trim 後 1〜160 byte、制御文字・改行・内側 `「」` を拒否。2,048 byte 超は NotRequested。
- 試験: `world_g1_01_*` 5 件。

### C2（`runtime/context/world/question_input.rs`）

- `runtime_runs(input_message_id)` が指す `conversation_messages` を running run・同 conversation・
  role `user|transcript` で読む。assistant・Tool 結果・recall 本文は parser へ渡さない。
- 読取不能は fail closed の NotRequested。新たな書込みはしない。
- 試験: `world_g1_02_*` 3 件。

### C3（`runtime/context/world/source.rs`）

- private `SeedPolicy::{EntityIdsOnly, ExplicitQuestionName}`。公開 bool/IPC 引数にしない。
- `prepare_candidate`（shadow/既存）は EntityIdsOnly のまま ExactName を InvalidInput。
- `prepare_explicit_question_candidate` だけが ExactName 一件を許可する。
- 別 SQL・FTS・embedding・部分一致・自動選択は追加していない。
- 試験: `world_g1_03_explicit_question_accepts_one_exact_name_but_shadow_does_not`、
  `world_g1_06_alias_resolves_only_when_it_is_unique`。

### C4（`turn.rs` / `question.rs`）

- Requested かつ認可済みのときだけ `compose_parts` に `Option<GraphRequest>` を渡す。
- `seeds=[ExactName(topic)]`、`Forward`、`LimitsV2::m1().capped()`、`IncludeFlags::default()`、
  `explicit_question=true`。intent で他要素を削らない。
- Coding 参照ゼロでも graph 質問なら Frame を取得する。
- 試験: `world_g1_04_*` 2 件、`world_g1_07_plain_chat_*`。

### C5（`render.rs` / `source.rs`）

- `render_world_frame_explicit` は明示質問のときだけ、unknown/ambiguous/stale/pending/capacity を
  持つ空 Frame を wrapper で返す。通常 shadow は EmptyFrame のまま。
- Scope 不許可・DB 破損は空 Frame にせず、既存エラー境界を維持する。
- 試験: `world_g1_05_*` 3 件、`world_g1_09_notice_only_frame_reaches_the_envelope_block`。
  後者は Broker の `combined_block`（実送信本文の元）に notice が verbatim で載ることを確認する。

### C6（`memory/context_window.rs`）

- trusted `CONTEXT_POLICY` に固定の World 読取規則を追加。データから system 文を組み立てない。
- `CONTEXT_POLICY_VERSION` を 2 へ。system message は 1 件のまま。
- 試験: `world_g1_08_policy_carries_the_static_world_reading_rules`。

### C7

- 既存 M4A の WorldLive → 送信前検査 → 本文/manifest 整合経路をそのまま使う。新 Provider 分岐・
  別 receipt・graph 用 TTL は追加していない。
- Broker の `combined_block` に graph JSON と notice が保持されることは `world_g1_04` /
  `world_g1_09` で確認した。graph fixture を実 HTTP 受信本文で検証する試験は未実施（§5）。

## 4. 五要素 fixture

`g1_tests.rs` は合成 fixture で project:p、concept:tech（name: Speculative Decoding, alias: tech/spec-dec）、
metric:decode、metric:voice、goal:natural、actor:user と、tech→decode→voice の作用、voice→goal の
serves_goal、project→goal の has_goal、voice↔decode の correlates_with、decode→tech の depends_on を
保存済み commit 経路で構築する。実技術効果を示すものではない。

| ケース | 期待 | 結果 |
| --- | --- | --- |
| 明示した技術→Goal | relevance/causal 経路と node 保持 | passed |
| 相関だけ | correlation 保持、因果捏造なし | passed |
| 許可 Scope 内にない対象 | unknown_seed、候補名なし | passed |
| 同名/alias の複数一致 | ambiguous_seed、自動選択なし | passed |
| 投影 stale | notice 付き空 Frame | passed |
| 雑談 | graph 取得 0、既存 Runtime のみ | passed |
| 名称に偽命令 | JSON データのまま、system/user 追加なし | passed（parser 試験） |

条件 unknown/unmet・依存 unknown/unavailable・競合/反証・Goal 撤回/Relation 変更の送信前除去は、
`WorldSliceV2` が値を保持することと既存 M4A 回帰に依存し、本報告では実 HTTP で未確認。

## 5. 未検証と制約

- `world:eval` は存在しない。M4A runner を作らず、G1 で類似 runner も新設していない。
- 実 HTTP 受信本文で graph の根拠・条件・notice が保持されることは未確認。
- 性能（Frame/再検証 p95<=150ms、World 追加処理 p95<=350ms、Frame 最大<=500ms）は未測定。
- live Provider の回答品質は未測定。固定応答 mock から品質改善を結論しない。
- AgentSession は並行 WD 計画で初回のみ再検証付き World（followup は World-free）。G1 の
  『対象外 Provider へ送信 0』条件は満たさない。DynamicLan / reasoning MCP の契約は未確定。

## 6. 再現手順

```sh
cargo test --locked --manifest-path src-tauri/Cargo.toml --lib world_g1_
cargo test --locked --manifest-path src-tauri/Cargo.toml --lib runtime::context::world
cargo test --locked --manifest-path src-tauri/Cargo.toml --lib memory::personal_state::world
cargo test --locked --manifest-path src-tauri/Cargo.toml --lib providers::chat_completions
cargo test --locked --manifest-path src-tauri/Cargo.toml --lib providers::agent_session
bun run check:personal-state
bun run spec:check
bun run test:rust-packages
```
