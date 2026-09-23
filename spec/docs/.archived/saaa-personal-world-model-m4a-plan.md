# Personal World Model M4A 実装計画 — 改修後の送信経路評価

作成日: 2026-09-20。状態: 計画作成済み、改修ゲート通過待ち。改修が完了したとは判定していない。

前段は[M3B計画](saaa-personal-world-model-m3b-plan.md)。本書はM3Bの送信不具合を改修した後に実装する評価機能を定める。[接続とモジュール予算計画](saaa-connect-and-module-budget-plan.md)のM3C凍結とMeeting削除方針を維持する。

## 1. 次に完成させるもの

Codingの現在状態が、許可された会話の送信本文へ届き、失効したら本文から外れることを、固定ケースで繰り返し検証できる評価スイートを作る。実際のProviderアダプターとローカルHTTP fixtureを使い、送信されたJSONとgeneration記録を照合する。

前回は「記録からWorldが消えた」「world: Noneを渡した」という内部状態だけを試験し、本文に残る不具合を見逃した。今回はHTTP受信側が観測した本文を判定の正本にする。モデルが何を答えたかを、送信経路の正しさの代用にしない。

成果物は固定ケース、評価runner、機械可読report、回帰ゲート。新たな自然文抽出やWorld推論を追加する段階ではない。M4Aはofflineの経路評価であり、live回答品質と利用者向け有効化はM4B以降に残る。

## 2. 現在の前提

計画作成時、作業ツリーでは次が変更中だった。

- `providers/chat_completions/world_body.rs` が追加され、送信前のWorld除去を担当する方向へ改修中。
- `runtime/context/world/live.rs` への寿命管理の分離、`runtime/conversation_turn.rs` への会話処理の移動。
- Meetingのowner・UI・World adapterが削除中。

これらのファイルの存在は完了証拠ではない。本書の作成ではRust試験を再実行していない。前回レビューの32 passed / 1 ignoredも改修後の成功値として転載しない。

実装開始時に実際のmoduleと関数へ接続点表を更新する。削除されたMeetingをfixtureの都合で復活させない。Worldの対象はCodingJobに限定する。steward GoalとWorld Goalの自動対応付けも行わない。

2026-09-21改訂: Provider対応範囲は後続WD計画を正本とする。AgentSession初回は再検証付きWorld、継続はWorld-free。DynamicLan/共有LARMは共通OpenAI互換adapterを使う。旧「対象外」条件をこの対応表へ置き換える。

## 3. G0 — 改修完了の独立ゲート

本節は改修担当の成果を受け入れる条件であり、M4A内で送信機構を再設計する指示ではない。G0未通過でもfixture仕様とreport型は作れるが、機能接続・性能認定・完了判定へ進めない。

| ID | 改修の受入条件 | 必須の確認方法 |
| --- | --- | --- |
| R1 | 初回送信直前のExpired/ChangedでWorldが除去される | World採用後に時刻/ownerを変更し、HTTP受信本文に当該Worldブロック・fixture識別子がないこと |
| R2 | tool-followupで失効したWorldが再送されない | 一回目はWorldあり、二回目はなし。Tool結果と元のユーザー指示は保持 |
| R3 | 対応表に従い、AgentSessionは初回のみ、未対応経路は送信しない | 当該Providerの実request encoder→HTTP受信本文を検査。manifestだけでは不可 |
| R4 | 本文とmanifestが一致する | 実送信bodyのdigestとrequest_digest、World有無とselected記録が一致 |
| R5 | 誤除去がない | World以外のPersonal State、通常assistant履歴、ユーザー指示、Tool結果を保持 |
| R6 | 既存制御を維持する | Memory OFF、shadow拒否、Scope/policy失効、Provider fallbackで回帰なし |

改修が入力を拒否して送信を止めるケースは、HTTP0件とgenerationの失敗/中断を照合する。Scope/policy失効を、Worldだけ外して必ず送信継続する仕様へ変更しない。

G0証拠には改修revision、dirty差分の有無、試験名、受信request数、本文とmanifestのassert結果を残す。旧不具合を検出する負例も最低二つ用意する。実装ソース内の文字列検索だけを証拠にしない。

## 4. 固定契約

### E1. 評価の実行環境

既存Rustの `#[cfg(test)]` とローカルHTTP fixtureを再利用する。必要な共通helperだけ分離し、別の会話RuntimeやProvider実装を作らない。サーバーは127.0.0.1のOS割当port、合成認証値、合成DBのみ。外部Provider、実API key、実workspace、pi process、ASR、TTSを起動しない。

既存 `quality_eval.rs` のlive用APIへWorldのテスト専用fieldを追加しない。task-localなfixture注入等、既存の試験隔離規則を踏襲する。並列試験でprocess環境変数を変更しない。Memory制御の注入が必要なら改修後の既存test helperを使用し、製品の有効化条件を増やさない。

### E2. 時刻と変更点

TTL試験はfake clockで行う。基準captured_at=1000、expires_at=2000。1999は期限内、2000はExpired、999は時計巻戻り。sleepで期限切れを作らない。

同期点は「compose完了後・初回body構築前」と「一回目のTool結果確定後・二回目body構築前」。既存のtest hookがなければcfg(test)の通知/待機を最小追加する。hookは最大一回だけ変更を挟み、タイムアウト5秒で失敗させる。製品retryや待機を追加しない。

生成中にTTLが切れるだけで完了を失敗させないM3B方針は維持する。今回検査するのは各request投入時の鮮度。生成開始後に既に外部へ渡った内容を取り消せるとは説明しない。

### E3. HTTPの観測

受信JSONをserde_json::Valueとして解析する。World有無は構造化されたmessages/input内の非命令Worldブロックと専用fixture IDの両方で検査する。request全体への単なるsubstring検索だけで合格にしない。AgentSessionはそのProviderのinput envelopeをさらにdecodeする。

模擬応答は固定の正常完了、Tool call、Tool結果後の完了の三形態。Toolは既存の副作用なしfixtureを使う。ストリームも既存SSE形式で送信する。モデル出力の語句一致は送信経路の評価項目に含めない。

要求JSONのdigestは受信HTTP bodyの生byteで計算する。現行manifestが同一JSONを別のbyte表現でhashする場合は、改修担当に「実送信byteかcanonical JSONか」の正本契約を確定してもらう。試験側だけで不一致を無視・変換して通さない。

### E4. 期待値

各ケースは固定ID、初期DB、Memory状態、Provider、変更点、期待HTTP回数、各requestのWorld有無、必須保持データ、manifest結果を持つ。任意のSQLやshellを外部JSONから受け取らない。fixture定義はRustのenumとbuilderにする。

Worldを落とした場合も、現在のユーザー指示は一つ、Personal Stateの根拠とTool結果は残る。Worldが送信された場合は非命令データでありsystem/userの追加にならない。Coding settledを「Goal達成」へ書き換えない。

### E5. Report

新規report型は `schema_version=1`、`suite=world-m4a`、case_id、pass/fail、固定failure_code、request_count、world_sent_per_request、manifest_match、elapsed_msを持つ。case一覧はID順。summaryはtotal/passed/failed/skippedを持ち、skipped>0を完全合格としない。

failure_codeは request_count / world_presence / retained_content / manifest / authority / isolation / timeout / fixture のいずれか。詳細な診断本文は合成fixtureの試験出力だけに限定し、API key、実本文、workspace pathをreportへ保存しない。

runnerの戻り値は成功/失敗を機械判定できる形にし、failed>0ならcargo testを失敗させる。reportは明示した一時出力先へ書く。製品のruntime.logやDBへ評価記録を混ぜない。

### E6. 接続点と変更範囲

| 対象 | 予定変更 |
| --- | --- |
| providers/chat_completions/world_eval_tests.rs（新規） | 実HTTP送信・初回/followupの試験 |
| providers/agent_session配下のWorld評価試験（新規） | 対象外Providerの本文検査 |
| runtime/context/world/eval_cases.rs（新規、cfg(test)） | case enum、共通Coding fixture、期待値 |
| runtime/context/world/eval_report.rs（新規、cfg(test)） | report型・集計・出力 |
| 各Providerの既存test_helpers | サーバー記録と同期点の最小共有 |
| package.json | world:evalコマンドを一つ追加 |
| spec/evidence/world-model/m4a-* | 進捗・結果・合成report |

module登録と必要なtest-only可視性は許可。turns.rs、lib.rs、巨大なquality_eval.rsへ評価処理を追記しない。新表、migration、公開IPC、製品env、World Kind、Source resolverを追加しない。

## 5. 固定ケース（20件）

| ID | 条件 | 期待 |
| --- | --- | --- |
| W01 | Memory OFF、明示ProjectとCodingあり | HTTP1、Worldなし、取得0 |
| W02 | Memory ON、running Coding、認可済みProject | HTTP1、Worldあり、manifest selected |
| W03 | Projectなし | HTTP1、Worldなし |
| W04 | Projectが二つで選択不明 | HTTP1、Worldなし、他Project情報なし |
| W05 | compose後now=1999 | HTTP1、Worldあり |
| W06 | compose後now=2000 | HTTP1、Worldなし、manifest非selected |
| W07 | compose後now=999 | HTTP1、Worldなし |
| W08 | compose後Coding revision据置でcancel_requested | HTTP1、旧running Worldなし |
| W09 | 一回目Worldあり、Tool後TTL超過 | HTTP2、World有→無、Tool結果保持 |
| W10 | 一回目Worldあり、Tool後owner状態変更 | HTTP2、World有→無 |
| W11 | AgentSession初回とTool継続 | HTTP2、World有→無、本文とmanifest一致 |
| W12 | OpenAI互換失敗→AgentSession切替、途中で失効 | primary HTTP1、AgentSession初回と継続HTTP2、旧World転送なし |
| W13 | World＋Personal State混在後に失効 | HTTP1、Worldだけ除去、Personal State保持 |
| W14 | Worldブロックに似た通常assistant履歴 | HTTP1、通常履歴は不変 |
| W15 | Worldが予算で不採用 | HTTP1、Worldなし、既存候補保持 |
| W16 | dispatch前Project/policy失効 | HTTP0、既存generation規則に従い失敗/中断 |
| W17 | dispatch前Coding根拠Source失効 | 旧Worldなし。共通generation依存も失効ならHTTP0、それ以外はHTTP1 |
| W18 | 別run二つの同時実行 | 各HTTP1、相手のfixture ID混入0 |
| W19 | generation完了はTTL後 | 一回目は有効なら送信可、TTLだけで既存completeを失敗させない |
| W20 | shadow候補が製品recordへ誤流入 | HTTP0、記録前拒否 |

W17はfixtureを二分する。同じSourceを通常Personal State候補にも使うケースはHTTP0、Coding参照だけで使うケースはHTTP1かつWorldなし。曖昧な「0または1なら成功」というassertは禁止。report上はW17a/W17bとして出し、全21結果を必須にする。

DynamicLan/共有LARMは共通adapterで再検証する。allocation/leaseを含む実経路の受入はWDのmatrixで別途記録する。

## 6. 作業カード（16枚）

各カードは実装1〜3ファイル＋試験1〜2ファイル。5実装ファイルを超えたら枝番へ分割。helper名は変更可、契約・ケース期待値・ゲートは変更不可。

| ID | 入力契約・対象 | 作業 | 完了条件 |
| --- | --- | --- | --- |
| E00 | §2/3、evidence | 改修revisionとG0証拠を整理 | R1〜R6の実HTTP証拠。未通過ならE03以降保留 |
| E01 | E4、eval_cases | ケースenum・期待値型を追加 | W01〜20、W17a/bの21結果を重複なく定義 |
| E02 | E5、eval_report | report集計・固定codeを実装 | fail1/skipped1を成功扱いしない、ID順、本文fieldなし |
| E03 | E1/E3、既存HTTP helper | request受信記録を追加 | 生bodyと回数を取得、外部接続0、既存fixture回帰通過 |
| E04 | E1/E2、Coding fixture | 実DBとfake clock・同期点を接続 | revision据置変更・期限境界をsleepなしで再現 |
| E05 | W01〜04、chat tests | 基本送信を評価 | Memory OFF取得0、有効時だけWorldあり |
| E06 | W05〜08、chat tests | 初回の期限・状態変更を評価 | 送信bodyとmanifestが一致、通常本文保持 |
| E07 | W09/10、chat tests | Tool followupを評価 | HTTP2、有→無、Tool結果と指示保持 |
| E08 | W11/12、agent/fallback tests | 対象外とfallbackを評価 | 対応表どおりのWorld有無。world=Noneの文字列検査は不可 |
| E09 | W13〜15、body tests | 混在・誤除去・予算を評価 | World以外の内容一致、既存候補を奪わない |
| E10 | W16/17a/17b、scope tests | 失効による停止と省略を評価 | ケース別のrequest数を厳密assert |
| E11 | W18〜20、boundary tests | 並行run、完了、shadow拒否を評価 | 混入0、既存complete規則維持、誤dispatch0 |
| E12 | E5、runner/package | 全ケースを一コマンド化 | world:evalで21以上の結果、失敗時exit非0 |
| E13 | §7、計測試験 | 従来と同条件で回帰測定 | 実clock測定とfake-clock機能試験を分離 |
| E14 | §7、回帰 | 対象suiteと全体ゲートを実行 | filter0禁止、失敗・未実施・成功を区別 |
| E15 | §8、evidence | 改修後基準と結果を確定 | G0、21結果、性能、未検証範囲を明記 |

E01/E02はG0待ちでも独立して作成可能。それ以外はE00通過後に順次進める。改修担当の進行中ファイルをこの計画の都合で書き換えない。

## 7. 検証と性能

```sh
# world:evalはE12で追加。実体は下記の集約試験。
cargo test --locked --manifest-path src-tauri/Cargo.toml --lib world_m4a_suite -- --nocapture
cargo test --locked --manifest-path src-tauri/Cargo.toml --lib runtime::context::world
cargo test --locked --manifest-path src-tauri/Cargo.toml --lib providers::chat_completions
cargo test --locked --manifest-path src-tauri/Cargo.toml --lib providers::agent_session
bun run check:personal-state
bun run size:check
bun run spec:check
bun run check:local
bun run test:rust-packages
```

集約試験が1件でもreport内の必須ケースは21結果以上であることをassertする。case試験はworld_m4a_WNNを名前に含め、個別実行可能にする。失敗ケースをignoreへ移して完了扱いしない。全体ゲートの個別再実行成功を、全体一回通過と書き換えない。

性能は同一端末・debug、warm-up5・sample30、nearest-rank p95=昇順29番目。改修後baselineとM4A実装後を比較する。製品World取得の既存p95<=350msを維持。fixture HTTPの待機時間をWorld取得時間へ混ぜず、総経路時間は別列にする。閾値を緩めず、性能不足は別の改修カードとして切り出す。

## 8. 完了と次段

`spec/evidence/world-model/m4a-progress.md`、`m4a-results.md`、`m4a-report.json`を実装時に作る。改修中の現時点では完了reportを先に作らない。

完了はG0通過、全ケース、全体ゲート、性能回帰、Meeting依存なしが条件。ここで言えるのは「許可された現在状態が正しい送信経路に届き、失効・対象外では送られない」まで。

次のM4Bは利用者が読む回答の品質評価。明示した合成質問と根拠に対する回答を測り、因果の誤断定・状態の誤認・不要参照を評価する。実Providerへの送信と費用が発生するので、本書のoffline runnerから自動実行しない。M3C自然文抽出、M2B外部Evidence永続化、Meeting復活、Worldの公開範囲拡大は本計画に含めない。
