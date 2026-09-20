# 接続とモジュール予算 検証結果

作成日: 2026-09-20。

## 実装済み

- コマンド列挙を `command_registry` へ移し、`lib.rs` の本番行予算を維持した。
- 会話ターンは dispatcher と `conversation_turn` に分け、永続化後・provider 前で `/capability` を処理する。
- C07 公開は 1 transaction。catalog 失敗で activation が残らない。
- C10 `GenerationService`。起動時 reconcile。running job は initialize で interrupted。自動再生成しない。
- C11–C13 の fake 経路: 不正コマンドはモデルへ送らない。A 生成→call→B 更新で A の call 行は残る。
- Coding 画面から steward Goal の登録・撤回。トリガ文面 `テストを確認して`。二重 Goal は既存エラーを画面向けに説明する。
- Settings / 音声セッションの行を既存 section・純関数へ移した。Meeting は未変更。

## offline合格

- `cargo test --locked --manifest-path src-tauri/Cargo.toml --lib rw_` : 7 passed / 0 failed
- `bun test tests/coding-steward.test.ts`
- `bun run size:check`（閾値未緩和。欠落ファイルの baseline 登録と、削除済み試験 2 件の stale 行削除）

## live未検証

- C14 実モデル。credential なしでは動かさない。
- 会話からの実キット package/inspect CLI。C13 は fixture packager。

## 未着手

- Meeting 削除
- Git / dirty 整理
- Clippy `-D warnings` 全体（既存: `network_asr`、voice ASR helpers、`large_enum_variant` など。新規 `allow` は足していない）
- `bun run check:local` 全ゲート
- inspect コマンドの TypeScript 表示を会話へ接続（parse と `render_inspection` 試験はある）
