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
