# 生成検査クローズアウト 進捗

作成日: 2026-09-20。対象外: C14 live、Meeting、Git、Role Routing。

## GC-00 現行との差分（検証報告は GC-07 で改訂）

| カード | 実装 | 試験 | 判定 |
| --- | --- | --- | --- |
| C07 1tx 公開 | `publication_sync.rs`。suspend で catalog 非公開 | `rw_04_*`、`gc_04` | offline合格 |
| C08 retire | `retirement.rs`。active は先に suspend | unit + `gc_04` | offline合格 |
| C09 fake/fixture | `FakeGenerator` + `FixturePackager` | generator 試験 | offline合格。live kit は C14 |
| C10 GenerationService | `generation/service.rs`。test 期限 250ms | `rw_05_*`、`gc_05`、`gc_06` | offline合格 |
| C11 会話接続 | `capability_turn.rs` | `rw_06_*` | offline合格 |
| C12 表示 | 保存済み成果物。改変は integrity | `gc_01`、`rw_06_inspect_without_a_call` | offline合格。on-demand kit は live |
| C13 通し | A inspect 後 B でも A の TS | `rw_13`、`gc_02`、`gc_03` | offline合格 |
| C14 | — | — | 対象外（live未検証） |

## カード

- GC-00: 本表。未達を成功と書かない。
- GC-01〜06: `generation_closeout.rs` と `capability_inspect` 試験。
- GC-07: 検証報告と `results.md`。
