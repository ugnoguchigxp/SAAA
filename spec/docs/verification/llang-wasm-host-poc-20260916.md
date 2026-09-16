# L-Lang Wasm実行キット Rust接続PoC 検証結果

実施日: 2026-09-16

## 判定

合格。SAAAのRustホストから、L-Langの単発Wasm実行キットをinspect・verify・invokeできた。これは開発用integration testによる接続PoCの合格であり、通常会話への能力公開、自動製造、受け入れ・配備gate、配布アプリでの動作確認を意味しない。

## 対象

| 項目 | 値 |
| --- | --- |
| SAAA基点commit | `68af784f9ac1dbd1cdf9bcea8b84531cb63190c3` |
| L-Lang commit | `0124d704f8228cb0c34d6379cf0302641199d82e` |
| L-Lang provenance | `dirty: false`のdetached worktreeから生成 |
| Bun | `/Users/y.noguchi/.bun/bin/bun`、`1.3.14` |
| protocol | `llang-host-v1` |
| packageHash | `1d31a73c93cb0136ef9b8521112a2c8efd42d79041d5008585b8874ffe3beb68` |
| kit.json SHA-256 | `c3272ba0ff6331179b5e4a92d1b4f077d96c3bfabee7dadaab86fcd2d0f47575` |
| データ | 合成fixtureのみ。API認証情報なし |

## 実装範囲

- `request.schema.json`と`response.schema.json`のDraft 2020-12検査
- inspect、verify、invokeのoperation別Rust型と内部整合性検査
- 信頼済み`kit.json`と全収録ファイルのSHA-256検査
- shellを介さない絶対パスのBun起動、空の環境変数集合
- 1プロセス1要求、stdin EOF、stdout/stderr並行読み取り
- request 64 KiB、stdout 1 MiB、stderr 64 KiBの上限
- inspect/invoke 15秒、verify 30秒のホスト期限
- timeout/cancel/上限超過時のkillとwaitによる回収
- transport error、構造化L-Lang error、正常な`false`の分離
- `#[cfg(test)]`のRust integration test入口のみ。Tauri IPCおよび会話toolには未登録

## 必須試験

| 試験 | 結果 | 確認内容 |
| --- | --- | --- |
| 基本接続 | 成功 | inspect、verify、invoke。APIキーなし、`apiCalls: 0` |
| 受付4通り | 成功 | `enabled=true, suspended=false`だけtrue。他は正常なfalse |
| 入力不正 | 成功 | 文字列booleanを`invalid-input`として保持 |
| 部品検証と外側応答 | 成功 | 外側okでも内部fail/errorをpassにしない |
| 候補不一致・改変 | 成功 | 異なるpackageHashと試験コピーの改変を拒否 |
| 不正応答 | 成功 | 未知protocol、ID/hash不一致、型不正、複数JSON、空応答、過大出力を拒否 |
| 環境・パス | 成功 | Bun不在を明示。空白を含むコピー先で実行成功 |
| timeout・cancel | 成功 | ハングするBunプロセスをkill後にwaitし、回収完了を確認 |
| 公開範囲 | 成功 | テスト時だけコンパイル。通常会話toolおよびTauri commandへの追加なし |

## 検証コマンドと結果

```sh
cargo test --manifest-path src-tauri/Cargo.toml wasm_host_poc --no-fail-fast
```

- 8 passed、0 failed

```sh
cargo fmt --check --manifest-path src-tauri/Cargo.toml
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
bun run size:check
```

- すべて成功
- module-size: 518 files

```sh
bun run test
```

- frontend: 322 passed、0 failed
- Rust library: 531 passed、0 failed、13 ignored
- IPC/architecture integration tests: 5 passed、0 failed

最初の対象試験では、stdin書き込みfutureの所有期間によりEOF通知が遅れ、正常な子プロセスがホスト期限まで待つ問題を検出した。stdinを明示的にdropしてからwaitするよう修正し、対象試験と全体試験を再実行して成功した。

## 未実施・後続

- 配布アプリへのBun同梱と署名済みbundle内での実行
- 同一OSユーザーの別プロセスに対する書き込み分離
- Coding Agentによる自動製造、修正、配備、切り戻し
- SAAA独自の受け入れgate、registry、有効版管理
- 通常会話からの能力選択・呼び出し
- fixture以外の実生成候補への接続

## L-Lang側の契約修正

今回の接続試験で必要になったL-Lang側の契約修正はない。
