# Context window / records / memory

着手前 `git status --short`（2026-09-22 23:00 JST 頃）:

- `M spec/docs/context-window-records-memory-implementation-plan.md`
- `?? spec/docs/context-window-records-memory-handoff.md`

HEAD: `b62a719` Finalize context record design decisions。branch `main`。

`DATABASE_SCHEMA_VERSION` 着手前: 38。CW-10 で 39（`generation_usage`。records DDL も同じ 39 に載せた）。

既存資産のシグネチャは計画 §1 と一致（`run_with_options`、`compose_after_connect`、`sha256_hex`、`available_agent_tools`）。

着手前 `cargo test --manifest-path src-tauri/Cargo.toml runtime::context`: 106 passed, 1 failed, 2 ignored。失敗は既存の `m3b_05_agent_session_records_world_only_for_a_revalidated_initial_turn`（`session.contains("world.provider_history(history)")`）。

計画 §1 と実物の差分: なし（上記失敗テスト以外）。

## CW-00 (2026-09-22 23:01 JST)

evidence を作成。

## CW-01 (2026-09-22 23:01 JST)

未実施。10 ターンの `bun run tauri dev` は実 Provider との対話が必要で、この作業環境では実行していない。表は空のまま。数値は作らない。

## CW-02 (2026-09-22 23:02 JST)

`src-tauri/src/README.md` に `records` と `runtime/context/segment` を計画中として追記。

## CW-10..CW-14 (2026-09-22 23:05 JST)

- `cargo test --manifest-path src-tauri/Cargo.toml cw_1 --lib` → 8 passed, 0 failed
  - cw_10, cw_11 x3, cw_12, cw_13 x2, cw_14
- `cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings` → 失敗。`role_routing` の既存 `field_reassign_with_default` が 22 件。今回のファイルは出ていない。
- `bun run size:check` → 失敗。baseline が現行ツリーより古く、着手前から多数のファイルが +10% を超えている。新規 `usage.rs` だけ baseline に登録した。既存上限は上げていない。
- `ml_01_schema_version_and_empty_goals` の期待値を 38 から 39 に変更。定数そのものを固定しているテストで、schema version を上げるカードの帰結。

## CW-20 (2026-09-22 23:08 JST)

- `cargo test --manifest-path src-tauri/Cargo.toml cw_20 --lib` → 2 passed, 0 failed
- schema version は 39 のまま（CW-10 で上げ済み）。
- `records/mod.rs` と `records/schema.rs` を baseline に登録。

## Phase P1 完了 (2026-09-22 23:08 JST)

- HEAD: 未 commit（この作業ツリー）
- 実行コマンドと結果:
  - cargo test --manifest-path src-tauri/Cargo.toml cw_1 --lib → 8 passed, 0 failed
  - cargo clippy ... -D warnings → 既存 role_routing で失敗（上記）
  - bun run size:check → 既存 ratchet 超過で失敗（上記）
- 計画との差分: `GenerationHandle::id` は CW-45 まで未使用のため `allow(dead_code)`。費用計算は計画どおり未実装。
- 未実施・保留: CW-01 の実機 10 ターン。clippy と size:check の既存失敗。
- 手動確認: なし（CW-36 / CW-46 まで）
- 提案: なし

## CW-21..CW-35, CW-41..CW-45, CW-50, CW-53, CW-54 (2026-09-22 23:34 JST)

- `cargo test --manifest-path src-tauri/Cargo.toml --lib cw_` → 63 passed, 0 failed
- records の commit / stream / FTS / 認可 / outline / capture / tools / catalog / forget と、Segment の create・append・carry・trigger・prefix 計測を追加。schema version は 40。
- `BackendRouter::new` は 3 backend。`SAAA_CONTEXT_SEGMENTS=1` のとき `AppState.context_segments_enabled` が true。既定は OFF のまま。CW-46 の実機比較前に既定 ON へ反転していない。
- `compose_after_connect` はまだ Segment 経路へ切り替えていない。builder は初期 Segment の作成と予算超過の判定まで。

## 未実施（停止条件ではない）

- CW-01 / CW-36 / CW-46 の `bun run tauri dev` 対話。実 Provider セッションがこの環境にない。数値は作っていない。
- CW-51 の journal recover テスト、CW-52 の `secure_delete` テスト（pragma は `SqliteWriter::open` に追加済み）。
- `cargo clippy -D warnings` と `bun run size:check` は着手前からの既存失敗が残る。
- `forget_personal_source` は records の失効と Segment の invalidate を呼ぶ。CW-51 の journal recover テストは未追加。

## レビュー後 (2026-09-23)

- 読み取りツールは principal の解決に失敗したら tool error を返す。読み取りは writer ではなく reader を使う。
- blob は id で結び、同じ hash でも domain が違えば別物として読む。
- Segment の既定は OFF。`SAAA_CONTEXT_SEGMENTS=1` のときだけフラグが立つ。compose は常に `budget.apply` を通す。プロンプトと違う active Segment は作らない。
- カタログ登録は `register_revision` の結果をそのまま返す。
- 同じ本文の Segment entry は既存 blob の ref_count を増やす。
- 会話メッセージの forget は呼び出し元の epoch を tombstone に書く。
- web capture は run_id を残す。
- CW-01 / CW-36 / CW-46 の実 Provider 対話は未実施。数値は空。

## 残カードのコード (2026-09-23)

- `SAAA_CONTEXT_SEGMENTS=1` かつ chat completions のとき、`compose_after_connect` は `segment::builder::build` の履歴を返す。既定は OFF。実測前なので既定 ON にはしない。
- `store_result` は `mcp_result` を保存し `record_id` を埋める。
- `assemble` は principal が取れるとき `ensure_registered` を呼ぶ。
- diagnostics の `contextMetrics.records` に件数、bytes、10 GiB 比を出す。
- CW-01 / CW-36 / CW-46 の対話表と CW-55 の `bun run check:local` は、実 Provider と既存の clippy / size 失敗があるため未了。数値は作っていない。
