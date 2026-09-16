# Personal State core

Personal State P1の先行実装。時刻、ID、認可結果、source metadata、実測予算をAdapterから受け取る決定的コアで、IOやモデル実行を行わない。

`Ledger::apply`はpatchを原子的に検証し、`project`は指定時刻と権限に対して状態を返す。`forget`はtombstoneを適用し、消去すべきpayload参照と依存閉包を返す。raw本文は保持しない。

```sh
bun run check:personal-state
cargo test --locked --manifest-path crates/personal-state-core/Cargo.toml
```

アプリの `src-tauri/src/memory/personal_state/` から利用する。SQLite CAS、実payload消去、durable outbox、principal binding、HTTP、抽出worker、診断はこのcrateが代行しない。入力は信頼済みAdapterが検証・発行すること。モデルの出力から認可やfenceを作らない。

接続条件と未完了項目は `spec/evidence/personal-state/p1-00-contracts.md` を参照。
