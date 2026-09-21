# Personal World Model G1 進捗

作成日: 2026-09-21。対象計画: `spec/docs/saaa-personal-world-model-graph-answer-plan.md`。
未達を成功として報告しない。filter 0 件を合格にしない。並行差分は巻き戻していない。

## G1-00 baseline・G0 証拠

| 項目 | 値 |
| --- | --- |
| 対象 module | `src-tauri/src/runtime/context/world/` |
| 本番入口 | `runtime/context/world/turn.rs::compose_for_app` → `compose_parts` |
| 既存 template | `memory/context_window.rs::CONTEXT_POLICY`（唯一の trusted system message） |
| DB schema version | 変更なし（本計画は新 table / migration を追加しない） |
| 公開 IPC / 新 env | 追加なし |
| `world:eval` | **存在しない**。M4A の実装済みコマンドが無いため G1 では新設しない（§5） |

### G0 の M4A 前提（着手時の確認）

G0 のチェックリストは前段 M4A（`saaa-personal-world-model-m4a-plan.md`）の R1〜R6 を指す。作業ツリーには
`m4a-results.md` も `world:eval` runner も無い。コード側の一部（送信直前の失効除去、対象外 Provider の
World-free 履歴）は既存の `providers/chat_completions/world_body.rs` と
`runtime/conversation_turn.rs::world_free_history` に入っているが、**G0 の独立した実 HTTP 証跡は揃っていない**。

さらに、並行作業の World Delivery Completion 計画が `providers/agent_session/sse.rs` を変更し、
AgentSession の初回ターンで再検証済み World を送る（`include_world = initial_world && round == 0`）、
followup は `without_world_history` で World-free、という契約になった。共有リポジトリの
`m3b_05_agent_session_records_world_only_for_a_revalidated_initial_turn` はこの新契約を固定している。
これは G0 の「AgentSession 等の対象外 Provider に World を送らない」を **文字どおりには満たさない**
（対象外ではなく、初回のみ送る対応 Provider へ変わった）。G1 では WD 契約を優先し、G0 の当該条件を
未達として扱う。

したがって G0 は **部分的にしか満たしていない**。本計画は「不足する改修を前段として扱い、機能追加と
混ぜない」方針のため、以下へ進んだのは parse・型・source 入口・本番 compose 配線の範囲に限る。
HTTP 受信本文レベル（G1-09/10/11）の完了は保留とする。

## カード記録

| ID | 契約 | 試験 | 結果 |
| --- | --- | --- | --- |
| G1-00 | G0/evidence | 文書 | baseline 記録。M4A runner 無し、AgentSession G0 未達を明記 |
| G1-01 | C1 `question.rs` | `world_g1_01_*` 5 件 | passed。四形式、`?`/`？`、空/超過/制御/複数/長文/Tool 風 |
| G1-02 | C2 `question_input.rs` | `world_g1_02_*` 3 件 | passed。running run・同 conversation・user/transcript のみ。別 run/role/mismatch は None |
| G1-03 | C3 `source.rs` | `world_g1_03_*` 1 件 | passed。本番 ExplicitQuestionName は ExactName 可、shadow は InvalidInput のまま |
| G1-04 | C4 `question.rs` | `world_g1_04_*` 2 件 | passed。seed 1・Forward・flags 全 true・LimitsV2::m1().capped() |
| G1-05 | C5 `render.rs`/`source.rs` | `world_g1_05_*` 3 件 | passed。unknown/ambiguous/stale の空 Frame を wrapper で返す。shadow は EmptyFrame のまま |
| G1-06 | query 統合 | `world_g1_06_*`、`world_g1_04_graph_only_*` | passed。五要素 fixture、alias 一意一致、correlates_with/depends_on 保持 |
| G1-07 | C2/C4 `turn.rs` | `world_g1_07_plain_chat_*` ＋ `question_input` 単体 | passed。`compose_for_app` は `question_input::read` の結果を `compose_parts` へ渡す（ソース確認）。graph 有無の合成は `compose_parts` 経由で検証。`compose_for_app` 自体の DB 読取統合試験は未追加 |
| G1-08 | C6 template | `world_g1_08_policy_carries_the_static_world_reading_rules` | passed。system 1 件、固定文言、質問文を system 化しない |
| G1-09 | C4/5 統合 | `world_g1_04_*` 強化、`world_g1_09_notice_only_frame_reaches_the_envelope_block` | 部分合格。Broker の combined_block（実送信本文の元）に graph JSON と notice が verbatim で載る。実 HTTP 受信本文は未検証 |
| G1-10 | C7 Provider 試験 | — | **保留**。初回/followup の失効は既存 M4A 回帰に依存。graph fixture 追加は未実施 |
| G1-11 | 対象外/fallback | `m3b_05_agent_session_records_world_only_for_a_revalidated_initial_turn`（並行更新） | **契約変更により N/A**。WD 計画で AgentSession は初回のみ再検証付き World。G1 の『送信 0』条件は満たさない。followup は World-free |
| G1-12 | 性能 | — | **未実施**。fake clock の TTL 機能試験のみ。実 clock p95 は未測定 |
| G1-13 | 全体検査 | 下表 | 対象 suite は passed。全体ゲートは並行破損で分離 |
| G1-14 | evidence | 本文書 + `g1-results.md` + `world-graph-answer.md` | 記録 |

## 実装ファイル

- `src-tauri/src/runtime/context/world/question.rs`（新規）: C1 parser + C4 GraphRequest 変換。
- `src-tauri/src/runtime/context/world/question_input.rs`（新規）: C2 保存入力の解決。
- `src-tauri/src/runtime/context/world/source.rs`: private `SeedPolicy`、`prepare_explicit_question_candidate`、C5 render 選択。
- `src-tauri/src/runtime/context/world/render.rs`: `render_world_frame_explicit`（unknown/ambiguous/stale/pending/capacity のみ空 Frame を返す）。
- `src-tauri/src/runtime/context/world/turn.rs`: `compose_parts` に `Option<GraphRequest>` を追加、`prepare_live` で explicit 入口を選択。
- `src-tauri/src/runtime/context/world/mod.rs`: module 登録。
- `src-tauri/src/memory/context_window.rs`: C6 の World 読取方針を trusted template へ追加。
- `src-tauri/src/memory/control_plane/mod.rs`: `CONTEXT_POLICY_VERSION` を 2 へ。
- 試験: `g1_tests.rs`（新規）、`question.rs`/`question_input.rs` 内、`turn_tests.rs` 更新、`context_window.rs` 内。

## TODO として残すもの

- G1-09/10/11 の実 HTTP 受信本文試験（M4A runner 不在のため前段扱い）。
- G1-12 の性能測定（同一端末 debug、warm-up 5、30 sample、p95=昇順 29 番目）。
- AgentSession の World 有無について WD 計画との最終契約確定。
