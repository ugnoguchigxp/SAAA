# Role Routing 全体ゲート監査

監査日: 2026-09-21

このファイルは完了報告ではない。R1〜R3の受入、性能測定、live laneは未完了である。

## 実施済み

- 隔離test binaryで `rr_` 81件: pass
- RR-29 manual Barrier fixture: update後に届く旧revision completionをdrainingのまま保持することを確認
- V2 regression: `runtime::context` 100件pass（性能gate 2件ignored）、`providers::chat_completions` 24件pass、`tool_selection` 191件pass
- V3: `bun run typecheck` pass、`bun test ./tests/role-routing-codex.test.ts` 2件pass（explicit loopback gatewayのみ、gatewayなしではMCPなし）
- V4: `bun run ipc:check`（binding 5件pass）と`bun run s11tnext:check`はpass。`bun run size:check`はrole-routingを含む多数の未登録/ratchet超過ファイルでfail。並行変更をまとめて登録しないため未解消として残す。
- `cargo fmt --check`: role-routing変更範囲でpass
- `git diff --check`: pass
- desktop smoke: build / bundle / launch / IPC ready の証跡あり
  - `desktop-smoke-20260921-role-routing`
  - `desktop-smoke-20260921-codex-actor`
  - `desktop-smoke-20260921-role-routing-rq52wJ`（build 97,929ms、bundle / launch / ready 成功）

## 未実施・不合格扱い

- A01〜A42の受入を網羅するV5は未実施。
- live SDK isolation、live provider、TTS体感、夜間負荷測定は未実施。
- Sol host tool loop（RR-21）は部分実装。sidecarは既存の認証付きloopback MCP gatewayだけを `rrRoot` に束縛して接続し、gateway はactive rootの会話を検証して `rr_tool_links` のreserve/settleへ帰属する。tokenはchild環境だけで渡し、JSONLへ含めない。`rr_21_bridge_keeps_bearer_token_out_of_jsonl` と `rr_21_role_root_query_is_strict_and_decoded_once` はpass。実認証SDKによるtool roundtrip・revision/model変更時のnew thread・tool budget・live isolationは未実施。
- Provider/TTS/toolの完了順を固定するE26 fixtureは2026-09-22追補で7件pass。live競合は未実施。
- RR-22: `rr_22_` 4件（deadline/cost gate、typed usage、step永続化）はpass。provider別費用換算・切替上限の実dispatch接続は未実施。
- RR-23: `rr_23_` 2件（evidence span/dirty mark、最新routing回答だけへの束縛）はpass。positive/negativeとchallenge新rootは未実施。
- RR-24: `rr_24_` 5件（独立actor、evidence scope、model/actor名を含めないreview packet、review output保存、mutating tool拒否）はpass。review executor、read-only MCP gateway接続、通常turnのreview step組込みは未実施。
- RR-25: `rr_25_` 3件（round limit、unsupported critiqueの保存と自動revision拒否、verified/unresolvedの分離保存）はpass。review後のauthor revision executorとrootへの再dispatchは未実施。
- RR-26: `rr_26_` 3件（implicit execution拒否、stale/cloud拒否、root/policy/revision/candidate/costへ束縛したproposal receipt）はpass。提案・承諾のIPC/UIと承諾後の実dispatchは未実施。
- RR-38: `rr_38_` 3件（無効/不正request拒否、host tool permit/revision確認）はpass。specialist wrapperは既存role-root gatewayにのみ接続しtool envelopeだけを返す。通常executorからの実呼出しは未実施。
- 全体 `cargo check` はrole-routing外のcalendar変更にあるmodule/command重複とOAuth API不整合で停止する。

したがってRR-39、および計画全体を完了とは判定しない。

## 実施済み（2026-09-22、E00〜E09）

E00〜E06 は roadmap の P0/P1 として実装・単体実行済み。E07〜E09 は offline 部品まで。
live lane（L01〜L04）は認証済みモデルを起動しておらず未実施。全体完成とは判定しない。

- `cargo test --locked --lib role_routing::`: 111 passed / 0 failed。
  - E01: `rr_01_unknown_field`, `rr_01_utf8_limit`, `rr_01_invalid_id`。
  - E02: `rr_12_old_revision_result_rejected`, `rr_16_pending_input_blocks_real_acceptance`, `rr_12_db_failure_no_speech`。
  - E03: `rr_02_start_claims_one_planned_step`, `rr_02_one_active_reasoning_step`, `rr_02_migrate_existing_partial_state`, `rr_05_duplicate_completion_once`。
  - E04: `rr_12_intermediate_output_not_final`, `rr_12_finalize_once`。
  - E05: `rr_04_receipt_retry_and_conflict`, `rr_16_multiple_pending_inputs`。
  - E06: `rr_03_policy_cas_conflict`, `rr_18_queued_policy_immutable`, `rr_22_deadline_starts_at_claim`。
  - E07: `rr_06_recipe_invalid_dependency`, `rr_22_recipe_all_branches_bounded`, `rr_06_self_review_alias_rejected`。
  - E08: `rr_05_one_actor_per_conversation`, `rr_05_io_does_not_block_input`, `rr_05_two_steps_run_in_order`。
  - E09: `rr_07_amendment_present_once`, `rr_07_scope_no_widening`, `rr_07_revoked_source`。
  - E11(部分): `rr_22_loop_budget`, `rr_09_shared_resource_group`。
- 全体 `cargo test --locked --lib`: 1340 passed / 0 failed / 23 ignored。
- `cargo test --locked --lib runtime::`: 193 passed / 0 failed / 6 ignored。
- `bun run typecheck`: pass。`bun test ./tests/role-routing-codex.test.ts`: 2 pass。
- `git diff --check`: pass。変更した role_routing ファイルは `rustfmt --check` pass。

## 未実施・部分（2026-09-22 時点）

- E07 recipe compiler / E08 driver+registry / E09 context projection / E11 budget は単体 test 合格だが、
  通常 turn の実 dispatch・AppState 登録・gateway 接続には未接続。したがって部分。
- E10 通常 turn と Provider/Sink 接続、E12〜E37 は未着手。
- V4 `sqlite_architecture`: `main_database_open_and_connection_ownership_are_centralized` が
  `compose_after_connect` の定義を `conversation_turn.rs` 内に要求するが、並行する dirty 変更で
  同関数が import へ移っているため fail。role-routing 変更とは無関係。
- `bun run size:check`: role-routing と他機能の既存未登録/ratchet 超過で fail（主変更の範囲で fail）。
  新規 `steps.rs` / `recipe.rs` / `driver.rs` / `context.rs` は未登録。roadmap の指示どおり
  baseline の一括登録・一括緩和は行っていない。E36 で対象を絞って処理する。
- L01〜L04、性能 P1〜P5、A01〜A42 の live/縦通し受入は未実施。
