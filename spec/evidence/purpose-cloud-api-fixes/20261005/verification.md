# 用途別クラウドAPI切替 修正検証記録

2026年10月5日。対象は2026年10月4日のレビューで挙げた六件、初期提供の登録・会話・音声・UI、その拡張としてのnative会話APIと画像・音楽。基準commitは `86826f02fe0e14ef2ada1526797e156f68b57727`。変更は専用worktreeで作成し、並行作業を残したままメイン作業フォルダへ統合した。

**F01–F06の欠陥挙動を受入assertへ変更し、全件合格した。全計画・出荷受入の完了判定はしていない。** 記憶・索引移行（P6）、Agent／検索の専用用途Binding（P7）は未実装。実APIとmacOSの実機音声受入（P8）は未実施。総合品質スコアは保留する。

## レビュー指摘との対応

| 指摘 | 修正 | 検証 |
|---|---|---|
| F01 無効化後の送信と回答採用 | 開始時Routeを固定し、各送信・Tool前と同一transaction内の採用で最新の無効化／同意／secretを検査 | queueのrevoked／consentはLLM 1回で停止し、追加送信・最終採用なし。通常の選択変更は旧Routeを維持 |
| F02 候補と実行先の不一致 | 共通adapter対応理由を候補と保存で共有。AgentSession／非harness LARMは理由付き非対応。音声必須の保存契約をUIへ反映 | Registryとfrontend helperで不適合を拒否。実行可能なChat／native／HTTP音声へ接続 |
| F03 draft復旧不能 | 保存成功ごとにrevisionを更新。保存済みdraftへのキー修復・有効化・削除を追加 | React故障試験でsecret失敗後、同じ接続ID・resource IDで再開。重複draftなし |
| F04 保存したIDが消失 | legacy所有IDと新規prefixを保存前に検証。cloud fixtureを有効なIDに変更 | Registryの保存・再読込と実queueで一致。標準cloudケースはLLM 4回、LARM 0回で採用 |
| F05 互換optionsの未反映 | Resource／Routeへoptionsを保持し、UI編集を追加。hostが所有するtoolsのみ上書き | 実requestの4stepすべてでcompletion token指定・thinking=false・stream=falseを確認。JSONは同じHTTP応答を処理 |
| F06 全体期限のリセット | 準備からTool・音声待ち・採用まで一つのdeadline。attemptは残り時間以下 | 1秒期限fixtureは4stepを完了せず採用なし。自動再試行を停止。terminal回答でも期限切れを拒否 |

## 追加した動作と回復

会話では、出力・Tool実行前の最初の推論に限り、接続不能・混雑・一時不提供から指定された直接APIの代替先へ移る。認証失敗、取消、期限切れ、出力後、Tool実行後は代替しない。実際に使ったRouteのattemptと採用を保存し、古い設定の成功を現在の設定の成功として表示しない。

HTTP ASRは発話IDを永続予約し、途中版と最終版で経路を固定する。確定済みIDは一時情報の破棄後・再起動後にも再利用できない。TTSは発話内のチャンクを固定し、同じplayerへ戻す。ライブASRの方式変更にはマイクの再開が必要で、画面に表示する。音声captureとPCMのAEC処理は維持した。

native会話はAnthropic Messagesを追加し、system・履歴・終了理由・usage・実modelを処理する。public textの完成応答だけを共通action loopへ戻す。native tool delegationと生成中表示は今回の提供範囲に含めない。[公式Messages契約](https://platform.claude.com/docs/en/api/messages/create)でヘッダーと終了理由を照合した。

画像・音楽はLARMとReplicate Predictionsへ接続した。生成run ID、設定snapshot、remote ID、状態をsingle writerで永続化し、再起動後は既知IDをGETで照合する。生成POSTを再送しない。中止はサービスの確認を得た場合だけ完了扱いにする。成果物を保存してから原子的に採用し、採用済み成果物だけをUIに返す。取得先・形式・容量を制限し、配信URLへAPIキーを転送しない。[公式HTTP API](https://replicate.com/docs/reference/http)の生成・照会・取消契約に合わせた。

## 検証結果

| 確認 | 結果 | 証拠 |
|---|---|---|
| メインfrontend契約 | 143/143ファイル、433ケース合格 | [main-frontend.log](main-frontend.log) |
| 隔離worktree frontend契約 | 142/142ファイル、431ケース合格 | [worktree-frontend.log](worktree-frontend.log) |
| Registry | 16合格 | [registry.log](registry.log) |
| native response変換 | 2合格 | [native.log](native.log) |
| Media永続化・照会・取消・成果物制限 | 6合格 | [media.log](media.log) |
| 音声Route・発話固定・確定ID拒否 | 3合格 | [voice.log](voice.log) |
| terminal自動回答の取消・期限境界 | 1合格 | [terminal-route.log](terminal-route.log) |
| 既存PCM echo回帰 | 1合格。TTS一致PCMの除去と独立した重なり音声の保持 | [echo-reference.log](echo-reference.log) |
| 実queueとfollowup | 4合格、live 1件を明示的にignore | [queue.log](queue.log) |
| LARMライブラリ | 82合格、live 5件をignore | [larm.log](larm.log) |
| TypeScript／lint／frontend build／IPC | 合格、IPC 8合格 | [types.log](types.log)、[lint.log](lint.log)、[build-ipc.log](build-ipc.log) |
| 変更moduleのsize | 63module合格。既存baselineは保持し、新設moduleだけ初期登録 | [validation.json](validation.json) |
| リポジトリ全体check:local | size gateで停止。先行するformat、lint、personal-state-coreのfmt/clippyと106試験は合格 | [check-local.log](check-local.log) |
| リポジトリ全体Clippy／size | 不合格。対象外moduleの既存ratchet超過・不足／削除baseline等が残る | [clippy.log](clippy.log)、[module-size.log](module-size.log) |

queue内では標準cloud、native、JSON-options、接続撤回、送信同意撤回、初期fallback、認証失敗、実行中の選択変更、全体期限の9ケースを確認した。テスト件数はシナリオ数と別であり、9件を独立したRust test件数として加算していない。期限fixtureの遅延は400ms/step、全体1秒。全体1.8秒未満で停止するassertを使い、厳密な実性能値とは扱わない。

実DB・実キー・課金API・マイク収録は使っていない。隔離SQLite、localhostサーバー、mock IPCを使用した。PCM回帰は合成信号の試験であり、実機AECの受入証拠にはならない。Providerの現実の品質・料金・遅延や全model固有入力の互換性は未評価。今回の変更moduleには最終Clippyログに新規指摘がなかったが、リポジトリ全体ゲート合格を意味しない。

## 統合と再現

並行実装されたterminal質問経路が旧会話APIを呼んでいたため、Route snapshot・取消・deadline・採用auditを新APIへ接続した。terminalの既存権限・原文引用・workspace・成果採用は維持した。専用worktreeの基準commitにはこの並行実装がないため、この統合差分は [terminal-integration.patch](terminal-integration.patch) に分けて保存した。テストを含む四ファイルの差分であり、独立して旧基準commitへ適用するpatchではない。context行を省いた差分のため、適用には `git apply --unidiff-zero` を使う。

コマンド、保存したログの元SHA-256、変更ファイルのSHA-256、実行範囲は [validation.json](validation.json) に記録した。Rustは既存cacheを使うため `CARGO_TARGET_DIR=/Users/y.noguchi/Code/SAAA/src-tauri/target` を指定した。一部ログはコンパイラの既存警告を省いた要約で、manifestに明示した。全体gateの失敗ログは全文を保存した。

## 残る段階

P6は許可されたsource/Viewの取得、local binding、削除・配送・tokenizerの契約を保った推論adapterの接続と、consumer別索引generation移行が必要。現在は非対応理由を表示し、既存経路と索引を維持する。

P7は既存実装エージェント設定への案内まで。Agent／外部検索をPurpose Bindingから既存executorへ委譲する接続と、権限を保持した取消・成果採用の受入が必要。

P8は対応構成ごとの実API確認、再起動、性能と使用量、実機TTS-only／再生中の人声の両方を確認する必要がある。これらが揃うまで、初期提供や全計画の完了・リリース品質を認定しない。
