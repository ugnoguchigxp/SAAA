# 有限受入runner

```sh
bun run personal-state:live deployment.json evidence.json
```

`deployment.json` は model/release/runtime、policyDigest/promptDigest/tokenizerDigest（64桁のSHA-256）、snapshotMode（off/on）、maxInputTokens、maxInputBytes、maxOutputTokens（最大2000）、command（executableと引数の配列）を持つ。credentialをこのファイルやcommand引数へ含めず、製品用harnessの認証経路を使う。

commandは明示指定された製品用SAAA harnessを、shellを介さず起動する。stdinへschemaVersion、scenarioId、repetition、checkpoint、合成sources、deployment、resetState=trueをJSONで渡す。gold/forbiddenは渡さない。harnessは各回で隔離したSAAA状態を使い、実際の抽出・投影を経た結果を `personal-state-eval.ts` のresultSchemaに従うJSONでstdoutへ返す。itemsは永続化された投影、coveredSourcesは実際のcoverage、violationsは実行監査から取得する。モデル自己採点を使わない。

64件×3回、直列実行。1回の上限はscenario deadlineと30秒の小さい方、stdoutは1MiB。timeout時には子プロセス群を終了する。stderrは保存せず、失敗には固定codeのみを残す。所要時間はrunnerが実測して上書きし、欠測を成功として除外しない。

製品用harnessは `src-tauri/src/bin/personal_state_harness.rs` に追加した。実機の全反復はruntime消去用設定の管理者適用とgoldの人手確認待ち。runnerの子プロセス制御と192回の実行は合成試験で確認した。humanReviewed=falseのdraft goldでは、全件一致してもcertified=trueにならない。


製品harnessのビルド:

```sh
cargo build --locked --manifest-path src-tauri/Cargo.toml --features quality-eval-harness --bin personal_state_harness
```

`deployment.json` の `command` はビルド済み `src-tauri/target/debug/personal_state_harness` の絶対パスを指定する。`SAAA_LARM_CONTROL_URL` と `SAAA_PERSONAL_STATE_RUNTIME` は認証済みcapabilityのruntimeに一致させる。goldは `p1-ja-v2` で、局所条件のrequest IDがworker入力に渡るよう修正した。実際の配備値はcapabilityから取得し、固定の模擬値では有効化しない。現在の実機制約と管理者適用手順は [製品接続の進捗](product-connection-progress.md) を参照。
