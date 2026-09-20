# Personal World Model M3B 進捗

作成日: 2026-09-20。並行差分は巻き戻さない。0件実行を合格にしない。

## M3B-00 baseline

| 項目 | 値 |
| --- | --- |
| HEAD | `84ead9752a509cec4d5a81810816cd17cefc507f` |
| M3A | `m3-results.md`。shadow 完了、通常 turns 未配線（着手前） |
| `cargo test --locked --manifest-path src-tauri/Cargo.toml --lib m3_ -- --list` | 22 tests（うち 1 ignored 性能） |
| dirty | M3A world モジュール移動、steward 計画、Tool Selection D5、M3B 計画未追跡。巻き戻していない |

Role Routing は作らない。World 五要素拡張、L-Lang LLM、D6、第2 Writer、新 conversation runtime はしない。

## カード記録

| ID | 試験 | 件数 | 結果 |
| --- | --- | --- | --- |
| M3B-00 | 文書 | 試験未追加 | baseline 記録 |
| M3B-01 | `m3b_01_project_counts_and_refs_come_from_scope` | 1 | passed。Project 0/2 省略。`graph_request: None`。ExactName 経路なし |
| M3B-02 | `m3b_06_memory_off_*` `m3b_07_would_displace_*` | compose 経路 | Memory OFF で prepare 0。ON で compose 2 / prepare 1。lock 外。turns は `compose_for_app` |
| M3B-03 | `m3b_06_expired_*` と既存 `m3_13` | 2 | shadow 拒否維持。非 Current は selected 0 |
| M3B-04 | `m3b_04_expired_after_dispatch_does_not_fail_complete` | 1 | complete 成功。`expired-after-dispatch`。world_frame 表 0 |
| M3B-05 | `m3b_05_agent_session_does_not_record_world_kind` | 1 | chat のみ persistence.world。agent は None |
| M3B-06 | `m3b_06_*` | 5 | Memory OFF、会議、coding、Project のみ empty_request、期限切れ省略。他 Project なし |
| M3B-07 | `m3b_07_*` と更新 `m3_18` | 4 | would_displace、yellow、JSON 一致、命令 1、知識なし文面なし。turns に shadow 文字列なし |
| M3B-08 | 下表ゲート | — | `m3b_` 11 / `m3_` 21+1 ignored / context 45+1 ignored / clippy / size 679 / spec。全並列 check:local は host 過負荷で一時失敗、関連試験は単独再実行 passed |
| M3B-09 | [m3b-results.md](m3b-results.md) | — | 記録 |

`cargo test --locked --manifest-path src-tauri/Cargo.toml --lib m3b_`: 11 passed。

配置: `runtime/context/world/turn.rs`。`source_kind=world-model`。default は `SAAA_MEMORY_ENABLED` 既存。新 flag なし。
