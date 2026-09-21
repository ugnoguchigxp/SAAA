# M4A / G1 offline 受入結果

2026-09-21。M4A runner不在とG1のHTTP/性能未測定を解消した。
対象は合成DBとloopbackサーバーによる受入。全体リリース判定・WD全経路のlive受入は別である。

## 実行結果

| コマンド / 対象 | 結果 |
| --- | --- |
| `bun run world:eval` | 33ケース成功、0失敗、0skip。3つのRust試験が各ケースを実行 |
| `cargo test --lib runtime::context::world` | 55成功、0失敗、性能等2件ignored |
| `cargo test --lib memory::personal_state::world` | 111成功、0失敗、性能等2件ignored |
| `cargo test --lib providers::chat_completions` | 19成功、0失敗 |
| `cargo test --lib providers::agent_session` | 17成功、0失敗、既存live試験1件ignored |
| `cargo test --lib runtime::conversation_turn::conversation_controller` | 10成功、0失敗 |
| `cargo test --lib world_g1_performance -- --ignored --nocapture` | 1成功、実時計測定 |
| `bun run check:personal-state` | 成功（fmt/clippy/単体・統合試験） |
| `bun run test:rust-packages` | 成功 |
| `bun run ipc:check` | 成功 |
| `bun run spec:check` | 成功 |
| `cargo clippy --lib --tests -- -D warnings` | 共有作業ツリーの他ファイルで失敗。World・追加payload・受入fixtureの指摘は修正済み |
| `bun run check:local` | 再開後format/lintを通過しsizeゲートで停止。後続ゲートを全て成功したとはしない |
| `bun run size:check` | 共有作業ツリーの他モジュールで失敗。World担当の新規ファイルを登録し、既存閾値は引き上げていない |

Rustコマンドは `--locked --manifest-path src-tauri/Cargo.toml --lib` を付けて実行した。
report正本は `m4a-report.json`。runnerは必須ID欠落・重複・試験失敗・不正reportを失敗扱いにし、開始時に前回の成功を無効化する。filter 0件で成功しない。

## ケースの意味

- W01〜W10: Memory/Scope/TTLの境界、時計逆行、owner revision据置の状態変化、Tool継続。
- W11/W12: WDの初回限定契約。初回有効/失効、継続World-free、OpenAI互換503後のAgentSession切替。
- W13〜W15: Worldだけの除去、通常assistantの似た文字列と既存候補の保持、byte予算。
- W16: Scope epoch変更はHTTP0。別ケースのpolicy変更は新generationの前なら新policyで再照合し、旧WorldなしでHTTP1。
- W17a: 現在入力Sourceの削除でHTTP0。W17b: Coding専用Source unavailableでWorld-free HTTP1。
- W18a/b: 異なるowner revisionを持つ二つの独立DBで同時実行。
- W19: 有効時にdispatch後、TTLを越えて完了しても成功。W20: shadow混入はHTTP0。
- G1: 五要素、未知対象、Goal/Source/Relation失効、Tool後のgraph除去。
- WD-MCP: Worldはevidence一回だけ。失効時は本文とselectedから除外。接続待ち中の失効と実RPC引数digest一致も検査。

W12は設定解決を含むルート全体E2Eではない。両実adapter、実HTTP、共通fallback可否判定、World失効を検証する。
DynamicLan/共有LARMは共通OpenAI互換adapterの回帰であり、実allocation/音声サービスを動かした受入ではない。

## 性能

debug、同一端末、投影合計100件・ledger 2,000件、実時計、warm-up 5回＋30 sample。
p95は昇順29番目。Frameに実graphがあることと再検証Currentをassertしてから採用した。

| 測定 | 実測 | G1基準 |
| --- | --- | --- |
| prepare p95 | 26.56 ms | 150 ms以下 |
| revalidate p95 | 28.93 ms | 150 ms以下 |
| prepare最大 | 29.02 ms | 500 ms以下 |
| prepare＋revalidate＋render p95 | 56.08 ms | 350 ms以下 |

並行開発プロセスのある環境での値。WDの別目標「追加処理p95 30ms / TTFA悪化10%以内」を達成した証拠として流用しない。

## 残る受入

修正理由は `review-2026-09-21.md`。全体format/size/clippyとWDの実モデル・UI・音声matrixは別途残る。再開後Codex metadata receiptとreasoning-answer-v2を追加した。実モデル回答の正確性、Codexの五要素graph、Situation/DW/scheduleの共通Frame接続は未完了。詳細は ../world-delivery/progress.md。
