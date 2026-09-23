# SAAA 生成検査クローズアウト 実装計画

作成日: 2026-09-20。状態: 実装済み（C14 / Role Routing / Meeting は対象外のまま）。

[接続とモジュール予算](saaa-connect-and-module-budget-plan.md)（RW-00〜09）は接続まで完了した。生成の機能契約正本は [実行版検査・動的生成 統合実装計画](saaa-llang-generation-inspection-plan.md) の 12・13 章と受入 G01〜G12。本書は再掲しない。本書が足すのは「まだ合格にしてはいけない穴」を順に閉じる作業だけである。

## 1. 次に完成させるもの

登録要求 A を fake で生成し、通常会話（および既存 MCP 入口）から同じ revision を呼び、inspect で TypeScript を取得し、B へ更新したあと A の call から見た TS が変わらず、停止・復帰しても過去履歴が残る状態。検証報告が現行コードと一致する。

## 2. 既存計画との関係

| 計画 | 扱い |
| --- | --- |
| 接続とモジュール予算 RW | 完了扱い。本書でやり直さない |
| 生成計画 C01〜C12 の部品 | 正本。不足試験だけ足す |
| 生成計画 C14 / G11 | 対象外（live）。完了扱いにしない |
| Role Routing v2（RR-00〜39） | 既にある。本書では着手しない |
| Steward の M3C / Butler / 自動修正 | 凍結のまま |
| Meeting、Git dirty、署名・公証 | 対象外 |

相談して止まる条件: Scope / 忘却 / single writer と矛盾する、外部送信が必要になる、size 閾値を緩める、`#![allow(dead_code)]` で clippy を通す、C14 を fake で代替する。

## 3. 実装調査で分かった残件

- `spec/docs/verification/llang-generation-inspection.md` は C07/C10/C11 を「未実装」と書いており、現行と食い違う。未達表を更新するまで全体完了と読めない。
- `rw_13` は A 生成→invoke→B 更新→A の call 行維持まで。inspect 表示、MCP 同一 revision、suspend/retire 後の過去 TS は通していない。
- `/capability inspect` は保存済み成果物があれば表示し、無ければ kit 設定時だけ `capability_inspect_run` で実行する。試験は live kit 無しでは「未記録 / 未生成」まで。C13 の「実行履歴から TS を取る」は fixture inspector が無い。
- C08 の retire/復帰と C07 の `unpublish_tool_for_capability` は部品がある。生成 job 経路からの停止・台帳非公開の通し試験が無い。
- 取消は `check_cancel` と job 終端まで来ている。caller abort と process 回収の会話経路試験は未接続。
- RW-09 の `cargo clippy --all-targets -D warnings` と `bun run check:local` は未完走。既存 ASR `dead_code` を本計画の合格条件にしない（触ったモジュールだけ `-D warnings`）。

## 4. 契約

**B0 正本。** 型・DDL・禁止事項は生成計画 12 章。衝突したら生成計画を優先する。

**B1 試験。** 計画 12.8 の ①〜⑩のうち live 以外を `rw_` または既存 `generated_capabilities` 試験でカバーする。0 件実行を合格にしない。

**B2 inspect。** 他人の call 拒否、64KiB 超は要約、fence escape。成果物が無い call は kit 無しで成功を装わない。試験用に live CLI を要求しない。

**B3 公開。** 生成の auto_activate 失敗は grant を増やさない。suspend/retire は catalog から外し、過去 call の inspection は残す。

**B4 報告。** 検証報告の「未実装」を、実装済み / offline合格 / live未検証 / 未着手に書き直す。C14 を offline 合格で埋めない。

**B5 規模。** size 閾値未緩和。新しい SqliteWriter、新しい Runtime、新しい env 禁止。既存 `SAAA_LLANG_GENERATION_CONFIG` 以外を足さない。

## 5. ファイル分担

| ファイル | 役割 |
| --- | --- |
| `generated_capabilities/tests/generation_flow.rs` または新 `generation_closeout.rs` | C13 の残り（inspect、停止、過去 TS） |
| `runtime/capability_inspect*.rs` | 保存済み表示と on-demand。試験用 Inspector を kit 無しで差し込めること |
| `publication_sync.rs` / `retirement.rs` | unpublish を suspend/retire に接続する試験 |
| `spec/docs/verification/llang-generation-inspection.md` | 現行との差分を正す |
| `spec/evidence/generation-closeout/` | カード証拠と results.md |

5 実装ファイルを超えるカードは枝番へ。

## 6. ゲート

| ゲート | 合格 |
| --- | --- |
| 本計画の試験 | `cargo test --locked --manifest-path src-tauri/Cargo.toml --lib rw_` および新 `gc_`。failed 0、件数 > 0 |
| 生成回帰 | `cargo test --locked --manifest-path src-tauri/Cargo.toml --lib generated_capabilities` |
| Clippy（触った crate 面） | 変更モジュールに新規 warning 0。全ターゲット `-D warnings` の既存 ASR は未達として記録してよい |
| size | `bun run size:check`。閾値未緩和 |
| spec | `bun run spec:check` |
| 報告 | results.md が検証報告と矛盾しない |

## 7. 完了報告

`spec/evidence/generation-closeout/progress.md` と `results.md`。C14 と Role Routing と Meeting は未着手のまま残してよい。

## 8. カード（GC-00〜07）

一枚を完了してから次へ。並行着手しない。

| ID | 対象 | 契約 | 実装すること | 合格条件 |
| --- | --- | --- | --- | --- |
| GC-00 | evidence と検証報告の差分表 | B4 | 現行ファイルから C07〜C13 の実装/試験有無を 1 表に記録。検証報告はまだ書き換えない | 未達を成功と書かない。C14 対象外を本文に書く |
| GC-01 | inspect の kit 無し試験 | B2 | 保存済み成果物の表示、未記録、他人の call。on-demand は Fake/Fixture inspector で 1 本 | live kit 0。`rw_06` / 新 `gc_01` > 0 |
| GC-02 | C13 ①② | B0/B1 | A 生成→会話 invoke→inspect→B 更新→同じ A call の TS/revision 不変 | fake + fixture。credential 0 |
| GC-03 | C13 ③⑥ と G06 | B0 | 不正 source で acceptance 失敗、A 維持、grant 増 0。改変 TS は integrity | 既存失敗経路を会話/job から再現 |
| GC-04 | G04 / C08 通し | B3 | suspend で catalog 非公開、過去 call は inspect 可。retire は active 拒否の既存契約を壊さない | `gc_04`。tool 行が残って呼べないこと |
| GC-05 | G07 / ⑫のうち取消 | B1 | 生成中 cancel で job が cancelled。自動再生成 0。timeout は failed | job 行が running のまま残らない |
| GC-06 | G08/G10 の欠け | B1 | 更新中 epoch 変更で conflict。別 principal の inspect 拒否（未カバーなら追加） | barrier または既存 CAS 試験で足りるなら新コード 0 |
| GC-07 | 検証報告 + results.md + size | B4/B5 | 検証報告を現行に合わせて書き直し。G11 は live未検証。触った面の clippy | 報告と results が一致。size 合格。C14 を完了扱いしない |

指示例: 「GC-01 だけを実装してください。live kit を試験に要求しないでください。他人の call を成功にしないでください。」

## 9. この次

本書のあと、製品機能の次計画は既にある [Role Routing v2](saaa-role-routing-plan.md) である。着手するときは steward 凍結表と衝突するため、明示的に解凍してから RR-00 を取る。本書と同時に始めない。
