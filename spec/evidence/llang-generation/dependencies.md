# SAAA × L-Lang 生成・検査 依存記録（T00）

記録日: 2026-09-20。実装開始時点の状態と、採用した検査・生成入口を固定する。既存の未コミット
変更は退避・削除していない。

## ベースライン

| 項目 | 値 |
| --- | --- |
| SAAA commit | `584d9b9fc9f7679382288321ba6455ceeabd5354` |
| SAAA 作業ツリー | 未コミット変更あり（43件）。うち本実装は `src-tauri/src/generated_capabilities/**`、`src-tauri/src/tool_selection/**`、`src-tauri/src/providers/**`、`src-tauri/src/runtime/mod.rs` と新規 `generation/`・`inspection/`・`runtime/capability_commands.rs`・`scripts/llang/`。他は並行作業中の未コミット変更で、本実装は触れていない |
| DB schema version | 着手時 25 → 本実装で 26（`generated_capability_generation_jobs`、`generated_capability_call_owners`、`generated_capability_inspections` を追加） |
| L-Lang commit | `a116435f2d890fae6ed16100bbdffb4ddd0ce1e2` |
| L-Lang dirty | `false`（着手時。`git status --porcelain` が空。M0時点の未コミットsnapshot依存は解消済み） |

注記: セッション後半に並行作業が L-Lang 側の docs と `src/llang-module-value-wasm.ts` を変更し、
作業ツリーは dirty になった（本実装は L-Lang を変更していない）。kit は作業ツリーを bundle するため、
dirty 状態で再生成すると digest が変わる。管理者設定の `expectedKitDigest` は clean HEAD
`a116435f` に対する下記 digest を基準とし、dirty な tree で作った kit は再固定するまで拒否される。
本実装の unit 試験は digest 不一致を拒否することを確認している。
| Bun | `1.3.14` |
| 既存 M1 trusted runtime digest | `afbd1ff24fd1b71da86e47441da7c3ffbd1998628d05b5a193fbdfb57d668dba`（M0 で固定。本実装は変更しない） |

## L-Lang 入口（12.1 の確認）

実在を確認した入口:

```text
bun run llang package <source.llang.jsonc> --request <request.json> --suite <tests.json> --metadata <metadata.json> --out-dir <new-directory>
bun run llang inspect <capability.json> --out-dir <new-directory> --json
```

- `src/llang-cli.ts` に `package` / `inspect` サブコマンドが存在する。
- `package` 成功時の `verification`/`acceptance` は `not-run`。合格とは解釈しない。
- `inspect` は `program.inspection.ts` と `inspection.json` を出力する。report は
  `format=llang-capability-inspection`、`version=1`、`typescript.source/projectionHash/sourceHash/programHash`、
  `artifacts.artifactHash`、`packageHash` を持ち、`semanticEquivalence=not-checked` を保持する。
- `llang develop` と旧 v1 の `capability verify` は呼ばない。

## 生成 kit（C01）

新規 `scripts/llang/build-generation-kit.ts` が、`llang package`/`inspect` と同じ L-Lang ライブラリ
関数（`packageLlangCapability` / `inspectLlangCapability`）を呼ぶ薄い固定 dispatch を Bun で
self-contained に bundle し、`manifest.json` に entrypoint・file一覧・hash・L-Lang HEAD/dirty・
Bun版を保存する。digest は sorted `name\0hash\n` の SHA-256（runtime bundle と同じ規約）。

生成した kit の実測値:

| 項目 | 値 |
| --- | --- |
| entrypoint | `llang-cli.js` |
| files | `llang-cli.js`、`package.json` |
| kit digest | `7bd4d6415ab4214236ca0ac05710878c3ab8db18323327c8ccff5f5b05033c83` |
| bunVersion | `1.3.14` |
| llangVersion | `a116435f2d890fae6ed16100bbdffb4ddd0ce1e2` |
| llangDirty | `false` |

self-test で確認した項目（`--selftest`）:

- パスに空白を含む別ディレクトリへ kit を移動しても `package`/`inspect` が動く。
- L-Lang checkout なしで `package`/`inspect` が動く（bundle に内包）。
- kit file を改変すると再計算 digest が manifest と一致しなくなる。
- `inspect` は `format=llang-capability-inspection` と `semanticEquivalence=not-checked` を返す。

## 生成設定・要求

- `SAAA_LLANG_GENERATION_CONFIG`（絶対path）。`formatVersion=1`、`enabled` 必須、`bunPath`/`kitRoot`/
  `expectedKitDigest`/`requestsPath`。上限 64 KiB、未知 field 拒否。
- requests ファイルは上限 1 MiB、`formatVersion=1`、entries 最大 32。request/suite/metadata は
  読込時に SHA-256 を固定し、job workspace へ byte copy する。

## 対応 provider

- 生成は既存 `providers/openai_compatible/structured.rs` の設定選択・credential 取得を再利用し、
  生成専用 options（max output 4096、外側 deadline）を追加した `complete_generation` を使う。
  既存の訂正抽出（700 tokens / 5s）は変更しない。未対応 provider は `unavailable`。

## 未確定・blocker

- 作業ツリーに並行作業中の `src-tauri/src/steward/**`（未追跡）と `runtime/turns.rs` 等の未コミット
  変更があり、`cargo test --lib` の全クレートコンパイルが一時的に失敗することがあった。本実装は
  これらを変更していない。着手直後の `cargo test --lib generated_capabilities` は 78 passed で
  成功した（steward 追加前）。
