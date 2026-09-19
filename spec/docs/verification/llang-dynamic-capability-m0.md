# SAAA × L-Lang 動的拡張 M0 検証結果

実施日: 2026-09-19

上位文書: [詳細実装手順・契約・試験仕様](../saaa-llang-dynamic-capability-implementation-guide.md)

## 判定

| 判定 | 結果 |
| --- | --- |
| M0-R（実行接続） | PASS |
| M0-I（inspection接続） | PASS |
| M0全体 | PASS |

M0-Rは、固定した信頼済みruntimeと、そのruntimeから分離したcandidate A/Bについて、inspect・verify・invokeが成立したことを示す。M0-Iは、固定版L-Langのinspectionが対象candidateに対応するTypeScriptとhash群を返すことを示す。どちらもSAAAの通常会話・MCP・配布アプリへの公開を意味しない。

## T00 — baselineと依存版の記録

### 基点

| 項目 | 値 |
| --- | --- |
| SAAA commit | `1b595ffd10c3c77adb19c8d3f9f47c536aa3fe54`（`main`） |
| SAAA作業ツリー | 実装前から未追跡: `.commandcode/`、`spec/docs/saaa-llang-dynamic-capability-{concept,implementation-guide,initial-plan}.md`、`spec/docs/saaa-personal-world-model-concept.md`。本書とM1の成果物は未コミット |
| L-Lang commit | `7fb1a7bb639b18d29055fe032b7ffc5dc6f5ee15` |
| L-Lang provenance | `dirty: true`。採用したのは未コミット作業ツリーのsnapshot（後述のhashで固定） |
| Bun | `1.3.14` |
| Rust | `rustc 1.92.0`、`cargo 1.92.0` |
| データ | 合成fixtureのみ。API認証情報・外部ネットワークなし |

L-Langの変更・未追跡ファイル（実装時点）:

- 変更: `README.en.md`、`README.md`、`concept.md`、`docs/LLANG_CLI_REFERENCE.md`、`docs/README.md`、`examples/jsonc-enabled-user/README.md`、`src/generator.ts`、`src/llang-cli.test.ts`、`src/llang-cli.ts`、`src/llang-smoke.ts`
- 未追跡: `MAIN_CONCEPT.md`、`docs/CAPABILITY_INSPECTION_IMPLEMENTATION_PLAN.md`、`src/llang-capability-inspection.{ts,test.ts}`、`src/llang-predicate-projection.{ts,test.ts}`

採用snapshotの固定hash（SHA-256）:

| L-Lang file | hash |
| --- | --- |
| `src/capability-snapshot.ts` | `f72c913381fc64aef1ff1fd4d259ea31092a636d80df275dfcbd0d3aa705ccbd` |
| `src/capability-host.ts` | `ef5929347d95279f4c1e514751689a0a36a4a80ccb5002e3cd4f794fd7873277` |
| `src/capability-host-cli.ts` | `0b3424ed6d4049d3e6bac5c0ce58e4374882631eb6421faccf5ee4b580c807af` |
| `src/capability-worker.ts` | `5135f377c3baffa15ee6bcb61eb7f03b269cb6e7a766d462c37d8e4df514ad69` |
| `src/capability-invoke-worker.ts` | `b416c7175aa7d7a096351fc271b510715d9bf022f27e5c2b36ae325a8e3edbcb` |
| `src/llang-capability-contracts.ts` | `a9557548b7dbe01729eed63be27f817321c36ddf92e2c67dc8e04d6048758dc0` |
| `src/llang-capability-runtime.ts` | `e088fef204dc39ecd80d87fd9aa28163f8028ae69ffad7b55f1950bf2a645153` |
| `src/llang-capability-verifier.ts` | `397c3624ccc8c075e48eba4a28e0c21eaebd93a1f0d936cd8ea3b51db9f3c50b` |
| `src/llang-case-runner.ts` | `6e78ff88119d1bce405bbb9b879ad1d80f88ed214eddd89ef7af652a9db8c6f6` |
| `src/llang-capability-worker.ts` | `4c1b5afb84b2807c0c9d36d47176c57e46eba6f2faeeace78712be080b332b20` |
| `src/llang-capability-inspection.ts` | `08697247afceb9de299fd19f9cd6c997566c8eda8427ead0de25a5734ec48ede` |
| `src/llang-predicate-projection.ts` | `bafe9af7b7cfec1be0f1d7ddfc2c81ae440ea454d9ac5293fbb703499d505d7e` |
| `examples/jsonc-enabled-user/enabled-user.llang.jsonc` | `b802c4377f83634fcdc8f78b69412a4d8e62500ff48caa34ee85ad218712795c` |
| `examples/jsonc-enabled-user/request.json` | `4ef912ce2b184cd4ca45d3301b4e16cd781ff936145175cb34f5a1071863d126` |
| `examples/jsonc-enabled-user/tests.json` | `f0fb3804b208471799ca1dc4f307b41830d43a906b4a432e644436ad396ee1bf` |
| `examples/saaa-host/request.schema.json` | `741a72e2ad0b6847345ffa44665544eb8fc9d3c5de5946f35c904bc769d35987` |
| `examples/saaa-host/response.schema.json` | `0467ff5e7b5801748869b231cc270f83b1686f2d7603f842751bfa11508698e9` |

### baseline試験

| コマンド | 結果 |
| --- | --- |
| `cargo test --manifest-path src-tauri/Cargo.toml wasm_host_poc --no-fail-fast` | 8 passed、0 failed |
| `cargo fmt --check --manifest-path src-tauri/Cargo.toml` | 変更前から成功 |
| `bun run size:check` | 変更前から成功 |
| `cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings` | 変更後も新モジュール外のエラーは0件（新モジュールはM1で解消） |
| `bun run check:local` | 作業前baselineは未取得。変更後の結果はM1記録に記載し、新モジュール外で失敗した場合は既存問題として区別する |

## T01 — v2 wireの比較とfixture設計

### v1とv2の差

| 項目 | v1 PoC（`llang-host-kit`） | v2（採用） |
| --- | --- | --- |
| package manifest | `version: 1`、files は request 相当が `lock` | `version: 2`、files は `request` / `source` / `build` / `wasm` / `tests` |
| verify report | `version: 1`、`verifier: capability-predicate-v1`、`passed` / `failed` / `errors` / `unchecked`、case に `origin` | `version: 2`、`verifier: llang-capability-v2`、`requestRevision` / `suiteHash` / `sourceHash` / `programHash` / `artifactHash`、`requirements` / `uncoveredRequirements` / `coverage` / `diagnostics`。集計件数と `origin` は存在しない |
| case result | `id` / `origin` / `requirementIds` / `expected` / `actual` / `status` | `id` / `requirementIds` / `expected` / `actual` / `status` |
| invoke result | `{ "value": bool }` | 同じ |
| 外側envelope | `protocol: llang-host-v1`、requestId、packageHash、elapsedMs、apiCalls、status、result/error | 同じ |
| package_hash | `contentHash(parsed manifest)`（キー順を正規化したcanonical JSONのSHA-256） | 同じ方式。対象manifestがv2 |

採用する verifier は `llang-capability-v2` である。v1の `passed` / `failed` / `errors` 集計や `origin` をv2へ要求しない。

### SAAA側Rust型の変更項目

- `CapabilityReport` から v1専用の `passed` / `failed` / `errors` / `unchecked` を廃止し、`requestRevision` / `suiteHash` / `sourceHash` / `programHash` / `artifactHash` / `uncoveredRequirements` / `coverage` / `diagnostics` を追加。`verifier` の許可値を `llang-capability-v2` に固定。
- `CaseResult` から `origin` を削除。
- `PackageManifest.files` の role を `lock` から `request` へ変更。
- `InspectResult` は `manifest` / `contract` / `requirements` / `verification: "not-run"` / `acceptance: "not-run"` のままとし、v2 contractへ合わせる。
- すべての操作で `deny_unknown_fields` を維持し、未知fieldを拒否する。v1型へのfallbackは実装しない。

### 生response fixture

`src-tauri/tests/fixtures/llang-capability-v2/wire/` に、実hostが返した `{ request, response }` をそのまま保存した（inspect / verify A / verify B / invoke true / invoke false / invoke invalid-input）。「想定response」は作らず、Rust試験（W01〜W05）はこの実測JSONを入力にする。

## T02 — runtimeとcandidateの分離（M0-R）

固定版L-LangのHost CLI・worker・schemaをBunでbundleし、candidateとは別の保存領域に置いた。

| 項目 | 値 |
| --- | --- |
| runtime files | `capability-host-cli.ts`、`capability-worker.ts`、`capability-invoke-worker.ts`、`llang-capability-worker.ts`、`request.schema.json`、`response.schema.json` |
| runtime manifest SHA-256 | `9102af66ef3c69e8c594360de7bd7aec1171480fdfe2b261dfa2e1e839481ff1` |
| runtime digest | `afbd1ff24fd1b71da86e47441da7c3ffbd1998628d05b5a193fbdfb57d668dba` |
| candidate A packageHash | `bd4696ec301a6559f4a494d3bbfbd1e7dba57275e246af62256feb997d791a00` |
| candidate A artifactHash | `eeea769bad63479f1202d3551ff508541f3278578bdebcccdb6bb525afb424aa` |
| candidate B packageHash | `cbefe84aac5053ae0373cd62febdc409d5b31e68922e381b6ae8714b172506af` |
| candidate B artifactHash | `3d343f6f6053bb47cac545470c95739e32ba8e90113e1ae660a580cbf7e7e040` |
| candidate A requestRevision | `b24390d8696820b9b3c11ca8a1742f45233556fa26786d506df0acecdd901316` |
| contractHash | `1a545844899f6a8201e9643439adcfd2abc78998b86c956458c60d2d764f6034` |

候補Aは `enabled && !suspended`、候補Bは同じ入出力契約で `enabled` のみを見る。Bは「停止フラグによる除外を外す」という試験上の仕様変更であり、実際のアクセス制御には利用しない。

確認した事項:

- 同じruntime digestでcandidate AとBを切り替えてinspect・verify・invokeできる（H01）。runtime digestは両者で不変。
- runtimeを空白を含む別ディレクトリへコピーしても、candidate Aに対するinspectが成功し、digestが一致する（`m0r_runtime_is_reusable_from_a_moved_directory_with_spaces`）。
- source checkout・認証情報なしで実行できる。子プロセスの環境変数は消去される（H04で親のcredentialが子へ渡らないことを実測）。
- runtime fileの改変とBun不在を、candidateをspawnする前に拒否する（H02）。未知entrypointはmanifestのfilesに含まれないため拒否される。
- SAAA側の実行では、サービス構築後にruntimeを差し替えても実行されない。spawn直前にbundleを再検証し、検査したentrypointと実行するentrypointを同一に保つ（H05。M1で追加した回帰試験）。

M0-R = PASS。同一runtimeで二候補を扱えない、candidate側JSの起動が必要、snapshotのhashと実行対象が対応しない、のいずれも発生していない。

## T03 — inspectionの依存確認（M0-I）

固定版L-Langのinspection CLIをcandidate Aに対して実行した。

```sh
bun run llang inspect <fixtures>/candidate-a/capability.json --json
```

返された値:

| 項目 | 値 |
| --- | --- |
| format | `llang-capability-inspection` v1 |
| profile | `predicate-i32-v1` |
| packageHash | `bd4696ec301a6559f4a494d3bbfbd1e7dba57275e246af62256feb997d791a00` |
| requestRevision | `b24390d8696820b9b3c11ca8a1742f45233556fa26786d506df0acecdd901316` |
| TypeScript sourceHash | `b802c4377f83634fcdc8f78b69412a4d8e62500ff48caa34ee85ad218712795c` |
| TypeScript programHash | `6276b0c146b6124759c46aa96602d485b4f5e0ce64c02792c6c0e0f55a7462cb` |
| TypeScript projectionHash | `ea49df7b261ed1dd2f0d653e13e16efd160b01f1981f586208a2ca5e0045081f` |
| inspection.integrity | `checked` |
| inspection.verification / acceptance | `not-run` |
| semanticEquivalence | `not-checked` |
| apiCalls | 0 |

inspectionは対象packageに対応するTypeScriptと、package/program/projectionのhashを返す。SAAA側で読む表現とhashの対応が取れるため M0-I = PASS。

注意点:

- inspectionはL-Lang固定版の未コミットsnapshot（`src/llang-capability-inspection.ts` など）に依存する。採用snapshotはT00のhashで固定した。別のL-Lang版へ移す場合は再固定が必要。
- `semanticEquivalence: not-checked` のとおり、TypeScript表現とWasmの意味の同値性は証明されていない。SAAAがM1で行う独立acceptanceは、有限ケースの比較であって同値性の証明ではない。
- SAAAのM1はinspectionを呼ばない。実行記録からのTypeScript取得（M3）で接続する。

## fixtureの再生成

```sh
LLANG_ROOT=<L-Lang checkout> bun src-tauri/tests/fixtures/llang-capability-v2/tools/generate.ts
```

生成物の構成とhashは `src-tauri/tests/fixtures/llang-capability-v2/README.md` に記録した。L-Lang package形式は変更していない。

## 次の段階へ渡す依存

| 依存 | 状態 |
| --- | --- |
| v2 packageとhost protocol `llang-host-v1` | 固定済み（本書のhash） |
| 信頼済みruntime bundleとdigest | 固定済み（`afbd1ff2…`） |
| SAAA側独立acceptance | 固定済み（`acceptance/index.json` と真偽値表） |
| inspection CLI | 提供済みだが未コミットsnapshot依存。M3着手時に再固定する |
| L-Lang側への変更要求 | なし |
