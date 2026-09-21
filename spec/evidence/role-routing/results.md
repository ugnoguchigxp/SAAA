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
- Provider/TTS/toolの完了順を固定するBarrier競合fixtureは不足。
- RR-22: `rr_22_` 4件（deadline/cost gate、typed usage、step永続化）はpass。provider別費用換算・切替上限の実dispatch接続は未実施。
- RR-23: `rr_23_` 2件（evidence span/dirty mark、最新routing回答だけへの束縛）はpass。positive/negativeとchallenge新rootは未実施。
- RR-24: `rr_24_` 5件（独立actor、evidence scope、model/actor名を含めないreview packet、review output保存、mutating tool拒否）はpass。review executor、read-only MCP gateway接続、通常turnのreview step組込みは未実施。
- RR-25: `rr_25_` 3件（round limit、unsupported critiqueの保存と自動revision拒否、verified/unresolvedの分離保存）はpass。review後のauthor revision executorとrootへの再dispatchは未実施。
- RR-26: `rr_26_` 3件（implicit execution拒否、stale/cloud拒否、root/policy/revision/candidate/costへ束縛したproposal receipt）はpass。提案・承諾のIPC/UIと承諾後の実dispatchは未実施。
- RR-38: `rr_38_` 3件（無効/不正request拒否、host tool permit/revision確認）はpass。specialist wrapperは既存role-root gatewayにのみ接続しtool envelopeだけを返す。通常executorからの実呼出しは未実施。
- 全体 `cargo check` はrole-routing外のcalendar変更にあるmodule/command重複とOAuth API不整合で停止する。

したがってRR-39、および計画全体を完了とは判定しない。
