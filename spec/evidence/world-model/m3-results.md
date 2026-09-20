# Personal World Model M3A 完了記録

作成日: 2026-09-20。状態: M3A 実装完了（shadow 検証経路）。生成品質・通常会話投入・live Provider は未実施。計画は [M3A計画](../../docs/saaa-personal-world-model-m3-plan.md)、契約は [M3A実行契約](../../docs/saaa-personal-world-model-m3-execution-contract.md)、カード別は [m3-progress.md](m3-progress.md)。

## 1. 結論

既存 `WorldFrameService` から寿命付きの非命令 Candidate を作り、実 Context Broker で World なし／ありを比較する shadow 経路を実装した。`source_kind=world-model-shadow` は `generation_inputs::record` の先頭で拒否し、dispatch しない。

通常 turns、chat_completions、agent_session、conversation_controller には配線していない。製品 flag も無い。

## 2. ゲート

| ゲート | コマンド | 結果 |
| --- | --- | --- |
| M3 試験 | `cargo test --locked --manifest-path src-tauri/Cargo.toml --lib m3_` | 21 passed / 0 failed / 1 ignored |
| Context | `... --lib runtime::context` | 34 passed / 0 failed / 1 ignored |
| Clippy | `cargo clippy --locked --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings` | passed |
| size | `bun run size:check` | passed（676 files）。閾値未緩和 |
| spec | `bun run spec:check` / `spec-html check ./spec/docs --warnings-as-errors` | passed |
| check:local | `bun run check:local` | passed |
| personal-state | `bun run check:personal-state` | passed |
| World adapter suite | `cargo test --lib memory::personal_state::world` | 121 passed / 1 ignored |
| rust packages | `bun run test:rust-packages` | passed |

filter 0件を成功扱いしていない。M3-19 は `#[ignore]`。認定値は §3。

## 3. 性能（M3-19）

同一端末・debug・warm-up 5・30 sample・昇順29番目を p95。fixture は投影100・ledger 2,000・Runtime 8。

| 測定 | p95 | max | 条件 | 判定 |
| --- | --- | --- | --- | --- |
| render + 二回 compose | 0.447 ms | （full max に含む） | <= 20 ms | 合格 |
| 全経路 run_shadow | 29.358 ms | 34.968 ms | p95 <= 350 ms、max < 1000 ms | 合格 |

作成 35 / 採用 35（warm-up+sample）。開発ゲートであり音声 SLO ではない。基準は結果を見て緩めていない。

## 4. 合格基準 A〜L

| ID | 対応 | 結果 |
| --- | --- | --- |
| A | `m3_05` `m3_03` `m3_14` | 一候補、非命令 wrapper、Frame JSON を parse して照合 |
| B | `m3_02` 空/欠落 Project、`m3_07` link 削除 | World なし。他Project情報を summary に出さない |
| C | `m3_09` 固定入力で baseline_bytes 一致 | 同 Broker 結果 |
| D | `m3_15` Source 削除後の旧 Frame revalidate | Current でない。TTL 内でも旧候補不採用 |
| E | `m3_07` 1999/2000/999、`m3_12` clock 2000 | expired。再取得 0 |
| F | `m3_11` 予算衝突 | 単位省略。根拠切断なし |
| G | `m3_11` would_displace | 既存 selected 維持、World 不採用 |
| H | `m3_16` | JSON 復元。system/user 追加 0 |
| I | `m3_13` | record エラー、planned、dispatch 0 |
| J | `m3_08` `m3_17` | 入力不変、current instruction 1 |
| K | `m3_10` `m3_07` empty/expired | 理由を区別。省略を知識なしと断定しない |
| L | `m3_12` `m3_18` | 別 run 拒否、DB generation 0、全体 cache なし |

## 5. 未接続（意図的）

- `turns.rs` の候補配列への追加 0（`m3_18` が文字列検査）
- M2B、M3B generation 依存、M3C 自然文抽出、M4 品質
- Frame TTL 1,000 ms の延長なし。生成中 lock ではない
- IPC / HTTP / MCP / 環境変数による有効化なし

## 6. カード外でやらなかったこと

Role Routing と Tool Selection の dirty は巻き戻していない。新しい認可プロバイダ、LLM 抽出、長寿命 receipt は作っていない。

`run_shadow` / `prepare_candidate` は `ScopeSnapshot` を引数に取る。S1 の `scope::load` は呼び出し側が Writer ロック外で行う。同一 Writer 上での入れ子 read は deadlock になるため。

## 7. 次段 M3B へ渡す判断（計画 §8）

1. dispatch までの鮮度 TTL と、生成中の意味・権限の再検証を別型にする。M2A Expired を無視する変更は禁止。
2. stream / 音声 / 保存 / Tool のどの境界で検証するかを Provider ごとに決める。record 拒否だけでは本文流出の全経路を防いだと称さない。
3. 選択された World にだけ依存 receipt。再起動で復元しない。
4. 途中失効の再生成回数・費用・中断表示は未決。
5. trusted 要求の製品供給元（明示 Project / Runtime / seed）を決める。LLM に authorized を決めさせない。

M3A は回答品質を測っていない。有効化は default OFF、既存 Memory 設定配下に限る。

## 8. 完了判定

| 条件 | 状態 |
| --- | --- |
| M3-00〜21 | 実装済み。M3-20 の `check:local` と `test:rust-packages` は passed |
| A〜L | 対応試験あり |
| 性能ゲート | 合格（§3） |
| 通常会話未投入 | §5 |
| 生成品質 | live未検証。完了扱いしない |
