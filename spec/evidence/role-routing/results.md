# Role Routing 全体ゲート監査

監査日: 2026-09-21

このファイルは完了報告ではない。R1〜R3の受入、性能測定、live laneは未完了である。

## 実施済み

- 隔離test binaryで `rr_` 67件: pass
- `cargo fmt --check`: role-routing変更範囲でpass
- `git diff --check`: pass
- desktop smoke: build / bundle / launch / IPC ready の証跡あり
  - `desktop-smoke-20260921-role-routing`
  - `desktop-smoke-20260921-codex-actor`

## 未実施・不合格扱い

- A01〜A42の受入を網羅するV5は未実施。
- live SDK isolation、live provider、TTS体感、夜間負荷測定は未実施。
- Sol host tool loop（RR-21）は部分実装。sidecarは既存の認証付きloopback MCP gatewayだけを `rrRoot` に束縛して接続し、gateway はactive rootの会話を検証して `rr_tool_links` のreserve/settleへ帰属する。tokenはchild環境だけで渡し、JSONLへ含めない。`rr_21_bridge_keeps_bearer_token_out_of_jsonl` と `rr_21_role_root_query_is_strict_and_decoded_once` はpass。実認証SDKによるtool roundtrip・revision/model変更時のnew thread・tool budget・live isolationは未実施。
- Provider/TTS/toolの完了順を固定するBarrier競合fixtureは不足。
- RR-22: `rr_22_` 4件（deadline/cost gate、typed usage、step永続化）はpass。provider別費用換算・切替上限の実dispatch接続は未実施。
- 全体 `cargo check` はrole-routing外のcalendar変更にあるmodule/command重複とOAuth API不整合で停止する。

したがってRR-39、および計画全体を完了とは判定しない。
