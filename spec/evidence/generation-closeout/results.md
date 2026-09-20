# 生成検査クローズアウト 検証結果

作成日: 2026-09-20。計画: `spec/docs/saaa-generation-closeout-plan.md`。
対象外のまま: C14 live、Meeting、Git、Role Routing。

## 実装済み

- `/capability inspect` は保存済み成果物を先に表示する。TS ファイルと report の `typescript.source` が食い違えば integrity。他人の call は拒否する。
- suspend は同一 transaction で `tool_selection` catalog を非公開にする。過去 call の inspect は残る。
- 生成 job の cancel は `cancelled`、timeout は `failed`。自動再生成しない。試験のモデル待ちは 250ms。
- 更新中に catalog epoch が変わると conflict。A の active は維持する。
- acceptance 不一致の更新は failed。grant は増えない。A は残る。

## offline合格

- `cargo test --locked --manifest-path src-tauri/Cargo.toml --lib gc_`: 9 passed / 0 failed
- `cargo test --locked --manifest-path src-tauri/Cargo.toml --lib rw_`: 8 passed / 0 failed
- `cargo test --locked --manifest-path src-tauri/Cargo.toml --lib generated_capabilities`: 128 passed / 0 failed
- `bun run size:check`（閾値未緩和。新規試験・分割ファイルの baseline 登録のみ。727 files）
- `bun run spec:check`

## live未検証

- C14 / G11 実モデル。credential と実 kit が必要。fake で完了扱いしない。
- 会話からの実 kit `inspect` CLI（設定が無いときは保存済みが無ければ未生成と返す）。

## 未着手

- Role Routing v2 の解凍
- Meeting 削除
- `cargo clippy --lib -- -D warnings` 全体（既存 ASR `dead_code`。生成・検査モジュールの新規 warning は 0。`#![allow(dead_code)]` は使っていない）
- `bun run check:local` 全ゲート
