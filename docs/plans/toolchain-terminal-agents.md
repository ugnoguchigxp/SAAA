# SAAAの専用端末・実装エージェント連携計画

作成日: 2026年10月4日 JST

状態: 2026年10月5日、初期専用端末経路を実装。実CLIと2端末の起動・同じセッションでの再開・完了候補受信、および隔離DBからhost検証・報告までの往復を確認した。全体gateの未通過事項と実機確認の範囲は[受入記録](../../spec/evidence/terminal-agent-implementation/2026-10-05/README.md)に分けて記載する。

## 0. 実装で確定した契約

- 独立runner・MCP・Hook・進捗表示はSAAA本体の同じ実行ファイルを、GUI初期化前に別モードで起動する。tmuxと追加sidecarは不要。端末には読み取り専用の表示プロセスを開く。
- 正式な状態は既存coding台帳とterminal拡張テーブル。私有runディレクトリにイベントをfsyncしてからUnix datagramでwakeし、hostがinboxとcursorを原子的に採用する。2秒の確認は再起動後の取りこぼし回復用で、LLMの常駐pollingではない。
- Claude Codeは印刷モードのAskUserQuestionをHookでdeferし、保存した回答を同じsessionの同一質問へ適用する。Bash等の操作は許可ホストへ送り、SAAA画面の明示承認を同じtool inputへ1回だけ適用する。Codexはworkspace-write・approval neverで動かし、共通相談MCPを使う。Codexのsandbox外操作を自動承認する経路は設けない。
- 自動回答は設定で有効化した場合だけ、元の人間の依頼に明記された答えを引用して送る。プロジェクト資料を広く調べて新しい判断をする機能は後続。質問本文・CLI出力は未信頼の資料とし、許可はモデルに任せない。1質問1試行、job全体3判断、1判断30秒以内。foreground会話が割り込む。
- 同じ目的の確認失敗への修正依頼は、設定で有効化した場合だけ最大2回。確認コマンド・workspace・権限を増やさず、同じ保存sessionを再開する。
- 完了は、CLI停止・正常結果・完了候補・手動条件なし・ユーザーが保存した確認コマンドの成功・差分証跡をhostがそろえた時だけ採用する。確認コマンド未登録ならユーザー確認待ち。画面からの明示受入は別の人間の判断として記録する。
- 起動記録が途中、runnerの終了記録がない、確認中に再起動した場合は再送・確認再実行をしない。停止は所有プロセス、または保存した起動時刻とコマンド・process groupが一致した対象だけに適用する。元の結果不明は解消したように扱わない。
- 操作は既存の「作業」画面へ統合。実装先フォルダー選択、端末表示、質問回答、許可判断、停止、結果の受入を置く。追加指示は会話からcoding_continue、質問回答はcoding_answerを通す。
- 「設定 → 実装方法」で専用端末・CLI・必要ならCLI絶対パスとモデルを選び、確認処理を保存する。Pi／SDK／Providerの保存設定は保持する。接続確認は配置とCLI版の検出であり、認証・モデルの実応答を保証しない。

以下のP0〜P6と試験一覧は当初の計画を残したもの。方式・試験配置・対応範囲は本節と受入記録の確定内容を優先する。

SAAAへの依頼からCodex CLI／Claude Codeを起動し、専用端末に進捗を表示する。質問やBlockerを受けたら、任された範囲で回答できるものはSAAAが回答し、判断が必要なものはユーザーへ尋ねる。結果を検証してから作業完了を報告する。デフォルトの端末を選ぶ設定画面も実装する。

## 1. 正本と初期範囲

製品コンセプトの正本は[SAAAの全体コンセプト](https://chatgpt.com/space/page_9fc5877949748191b556705128f6a2f5)。本書は接続契約と実装順を扱い、コンセプト文書を複製しない。

当初の「同じ端末にユーザーとSAAAが入力する」要望は後続段階へ送り、2026年10月4日のユーザー決定に従って、初期版はSAAA専用端末とする。ユーザーは端末で進捗を見られ、追加指示、質問への回答、許可判断、停止はSAAA側から行う。この変更は正本にも反映済み。

### 対応すること

- macOSでKittyまたはGhosttyを開き、Codex CLIまたはClaude Codeに作業を依頼する。
- 端末を閉じても管理セッションを識別でき、表示を再接続できるようにする。
- 構造化された質問・失敗・終了イベントを受け、永続化後にSAAAの処理を起こす。
- 根拠のある回答、ユーザーへの相談、回答後の再開、取消、完了検証、報告。
- 設定の保存、CLIと端末の検出、対応能力と未準備状態の表示。
- SAAA再起動、重複イベント、回答競合、送信結果不明を扱う。

### 後続段階に送ること

人とSAAAによる端末への同時入力、対話型TUIのキー操作、Windows／Linux、Pi CLIの端末連携、複数実装エージェントの並列実行は初期版に含めない。既存のPi／Codex SDK経路は維持する。CLIの画面を文字解析して全Blockerを判定する方式は採用しない。

### 守る境界

既存設定・Provider・認証を初期化したり、起動失敗を理由に別のものへ置換しない。ユーザーのCLI設定やプロジェクトの設定を無断で書き換えない。CLIの承認、sandbox、Hookの信頼確認を迂回しない。DB移行試験はコピーまたは隔離DBで行う。既存の未コミット変更を巻き戻さない。

基本は既存Rust／Tokio／SQLite／Reactを使う。tmux案は採用せず、同じアプリ実行ファイルの独立runnerと表示モードを採用した。大型SDKや別の永続ジョブ基盤は導入しない。認証方式の全面変更や権限拡大が必要になった場合は、具体的な差分を整理してからユーザー判断に戻す。

## 2. 調査結果と残る確認

以下は当日の作業ツリーと、バージョン表示・配置確認の結果であり、実サービスの動作認定ではない。

| 項目 | 確認できたこと | 実装前に確かめること |
| --- | --- | --- |
| Claude Code | ローカルCLIは2.1.289。公式資料に印刷モード、構造化出力、質問の保留と再開がある | 採用版で質問を提示する許可ホスト、Hook設定の注入、回答・保留・再開が往復すること |
| Codex CLI | ローカルCLIは0.155.1。公式資料に構造化実行とHooksがある | 採用版でHookの信頼確認、設定経路、承認要求、相談ツール、セッション再開が往復すること |
| Codex SDK | package.jsonは0.144.4を固定している | 外部CLIと別の版・設定・実行経路として識別すること |
| Kitty／Ghostty | /Applicationsに両アプリがある | macOSで引数と作業場所を渡せること、表示の閉鎖・再接続が作業に影響しないこと |
| tmux | PATHと代表的な配置では未検出 | 利用可能な絶対パス、必要機能、導入案内または配布方法。今回インストールはしていない |

CLIは別々のAdapterとして扱う。Claude CodeのHook仕様をCodexへ流用しない。利用可能という判定は、実行ファイル検出、バージョン確認、連携準備、認証確認、実機受入を分けて保存する。版が変わったら対応能力の確認を失効させる。

### 現行コードへの接続箇所

| 既存箇所 | 現状と計画への影響 |
| --- | --- |
| src-tauri/src/coding/{contracts,settings,repository,service}.rs | coding_start／inspect／continue／cancel、job／run／event台帳がある。既存方式はPiとCodex SDK。CLI方式とイベント起点の判断契約を追加する |
| src-tauri/src/coding/{commands,recovery}.rs | probe、停止、復旧がある。外部runnerの生存確認とセッション照合を追加し、既存SDKのprobeへCLIを流さない |
| src-tauri/src/steward/ | wake、coding_eventsの消費、検証、報告outboxを再利用する。現在の終了中心のイベント処理に質問・保留・判断を加える |
| src-tauri/src/runtime/conversation_check/queue_runtime/conversation_answer.rs、queue_tools.rs | 現行会話のツール許可にcodingが含まれていない。backend追加だけで完了とせず、最新の会話キューから呼べるようにする |
| contexts/conversation/queue.context.toml、context_compiler.rs | 委任ツールと権限契約を反映し、既存の生成経路で出力を更新する。外部イベントを最新ユーザー発話として偽装しない |
| src/features/coding/、src/features/settings/SettingsPage.tsx | 実装方法・job表示へ端末設定、質問、再開、準備状態を追加する |
| src/lib/generated/coding.ts、IPC生成・検証 | Rust契約から生成する。frontendだけに独立した状態や型を作らない |

codingの現行契約では明示的な依頼・追加指示を重視し、Stewardの公開委任はread／test_run中心である。端末連携の自動回答は、ユーザーが任せた目的・操作範囲を保存する新しい契約で実装する。既存の全ツールを自由に自動継続できるようにする変更は行わない。

## 3. 接続構成

初期案は、SAAAが管理する構造化実行と、専用端末への進捗表示を組み合わせる。Claude Codeは印刷モード、Codexは非対話実行を使い、同じ仕事の続きは保存した正確なセッションIDで再開する。初期版の端末はCLIの対話型TUIではなく、runnerが整形した作業ログと状態を表示する。

| 部品 | 責務 |
| --- | --- |
| SAAAのToolchain入口 | workspace、目的、制約、完了条件、委任権限を確定してjobを保存する |
| coding controller | run採用、判断の送信、取消、復旧を既存台帳の下で管理する |
| terminal agent runner（新設案） | CLI起動、構造化出力、Hook／相談ツールの受信、イベントの一時保管、プロセスの終了確認 |
| Claude／Codex Adapter（新設案） | CLI固有の起動、イベント、回答、保留、再開を共通契約へ変換する |
| 独立runnerと進捗表示モード | 同じアプリ実行ファイルを別プロセスとして動かし、端末の閉鎖と実行を分離する |
| Kitty／Ghostty | 専用セッションへ読み取り専用で接続し、進捗を表示する |
| SAAAの判断処理とSteward | 根拠を調べて回答・相談・検証し、既存の報告経路へ渡す |

runnerはRustの小さなcrateとして本体へ組み込み、同じ署名対象の実行ファイルを別モードで起動する。既存アプリが再起動しても識別できるよう、別プロセスで管理する。runnerからSAAAのSQLiteへ直接書かず、SAAAのWriterが状態を採用する。

通信は私有ディレクトリ内のUnix socketを第一候補とし、job／runに結び付く認証とプロセス識別を持たせる。ディレクトリ・socketのアクセスを制限し、CLIやHookの報告をそのまま権限命令として信用しない。入力・出力のサイズを制限し、大きな結果は成果物参照へ分ける。

tmuxはSAAA専用socketと設定で起動し、通常のユーザーセッションを操作しない。表示は読み取り専用接続を使う。起動は引数配列で行い、依頼文をshell文字列に連結しない。CLI終了後にユーザー入力が汎用shellへ流れる構成にしない。tmuxの端末出力は表示用であり、質問・完了判定の正本にはしない。

### この構成を選ぶ理由と代替案

対話型CLIに文字やEnterを送る方式は、質問の位置、画面更新、途中入力に左右される。専用端末が許容された現在は、構造化実行の方が回答と再開を確認しやすい。将来の直接入力には別途入力調停を設計する。

SAAA内の子プロセスだけでも起動できるが、アプリ終了と作業終了が結び付きやすい。独立runnerとtmuxは復旧・表示再接続に有利な一方、配布・停止処理が増える。P0でtmuxの要件が成立しなければ、独立runnerとログビューアを保持する構成を比較して本計画を更新する。未検証のまま別方式へ自動フォールバックしない。

## 4. HooksとBlockerへの対応

Hookはイベントを短時間で配送する入口とし、ユーザーが答えるまでHookを待たせない。受信を保存した後にwakeする。SAAAの画面が閉じていることと、runnerが動いていること、SAAAの判断処理が起動していることは別に表示する。

| シグナル | Claude Code側 | Codex側 | SAAAの扱い |
| --- | --- | --- | --- |
| 仕様の質問 | AskUserQuestionに対するPreToolUse経路 | 相談用MCPツールと構造化イベント。Claudeと同じイベント名を前提にしない | 質問IDと選択肢を保存し、回答可能性を判定 |
| 操作の許可要求 | PermissionRequestと許可ホスト | PermissionRequest等、採用版の対応経路 | 委任権限を照合。未許可ならユーザーへ |
| ツール失敗 | PostToolUseFailure | PostToolUseの結果またはrunnerの失敗情報 | 一回の失敗だけでBlockerと断定しない |
| 応答終了 | Stop、構造化結果、プロセス終了 | Stop、構造化結果、プロセス終了 | 完了候補として検証へ。目標達成の証明にはしない |
| 実行エラー | StopFailure等とrunnerの終了情報 | runnerの構造化エラーと終了情報 | 接続失敗・実装失敗・結果不明を区別 |
| 一般的なBlocker | 共通のSAAA相談ツール | 共通のSAAA相談ツール | 理由、必要な判断、試したこと、根拠参照を保存 |

全Blockerを検出する万能なHookは前提にしない。実装エージェントには、解決に外部判断が必要な場合にSAAA相談ツールを使う契約を渡す。相談ツールを呼ばず「対応できない」と終了した場合も、完了検証で未完了として拾う。無出力の検出はハング候補であり、内容上のBlockerと区別する。

Claude Codeの保留は印刷モード限定の仕様である。AskUserQuestionを提示できる許可ホストを実際に設定し、必要な質問をdeferしてCLIの停止を確認する。回答後、同じ許可ホスト・セッションIDで再開し、保留中の質問に保存した回答を渡す。対話型CLIにも同じ保留が使えるとは扱わない。[Claude Code Hooks](https://code.claude.com/docs/en/hooks)、[CLI reference](https://code.claude.com/docs/en/cli-reference)。

Codexには独自のHooksと信頼確認がある。Hook設定が読まれたことと、実行が許可されたことを別に確認する。Claudeのdeferと同じ復帰を仮定せず、相談ツールから保留状態を返し、CLIが止まったことを確認してから保存したセッションの続きを起動する。ネイティブの許可要求は、採用版で検証できた経路だけを有効にする。[Codex Hooks](https://learn.chatgpt.com/docs/hooks)、[codex exec](https://learn.chatgpt.com/docs/developer-commands#codex-exec)。

Hookの未実行・失敗・タイムアウトを「問題なし」に変換しない。必須能力が欠ける実行モードは準備未完として開始を止める。実行中の接続喪失ではrunnerがイベントを保管し、新しい判断を適用せず、有限時間で保留または停止する。既に許可された実行中操作の取消が間に合わない可能性は状態と証跡に残す。

## 5. 自動回答の権限と実行状態

### 保存する委任契約

job開始時に、workspace、目的、完了条件、変更可能範囲、許可する操作、禁止事項、参照可能な根拠、追加指示の権限、期限・回数上限を保存する。外部への公開、資格情報、費用の追加、依頼範囲の拡大などを「実装に必要」という理由だけで承認しない。ユーザーが定めた具体的権限を確認する。

SAAAの回答は、質問に適用できる仕様・既存のユーザー決定・対象プロジェクトの根拠と結び付ける。未決の仕様、根拠不足、権限変更、繰り返す失敗はユーザーへ戻す。質問本文、推奨案、理由、選択肢を示し、既存決定を再質問しない。

自動回答・再試行は回数と期限を制限する。初期候補は同一質問への自動回答1回、同一原因への再試行2回、job全体の自動判断3回。P0／P3の検証で調整し、設定と実行開始時のsnapshotに保存する。料金の実測がないCLIについて、厳密な金額上限を保証した表示はしない。

既存の会話・推論キューを再利用し、foreground会話を優先する。イベント処理のために別の常駐LLMループを追加しない。イベント起点の判断には専用sourceと権限を持たせ、人間の発話IDやStartTurnInputを偽造しない。モデルによる回答案はhostが権限・根拠・対象質問を検査してから送信する。

### 状態を分ける

| 表示上の状態案 | 判定と次の処理 |
| --- | --- |
| 準備中 | 設定・権限・連携準備を確認。成功後にrunを起動 |
| 実行中 | 現在採用中のrunが動いている |
| SAAAが確認中 | 質問が保存され、根拠と権限を調べている |
| ユーザーの回答待ち | 未決質問がある。実行プロセスが保留終了したかも別に表示 |
| 再開待ち | 回答済みだが送信・新runの採用が未確定 |
| 結果を確認中 | CLIは終了したが、目標達成・検証が未確定 |
| 完了／未完了／失敗／取消／結果不明 | hostの検証または停止確認と証跡に基づく終端 |

jobの意味上の状態とrunのプロセス状態を混同しない。保留による正常終了を作業完了として採用しない。既存runのCHECK制約と単一active index、Stewardの終了イベント処理を一緒に変更する。保留runの終了理由を明示し、必要ならdeferred等のrun状態を追加する。実プロセスの停止確認前にactive枠を解放しない。

初期版は現在の単一実行制約を維持する。新しいrunを採用した後、古いrunのHookや結果が届いても現在の仕事を完了させない。回答の確定にはquestion ID、job revision、decision IDを使い、SAAAとユーザーからの二重回答・取消後の回答を拒否する。

## 6. 永続化・配送・復旧

既存coding_jobs／coding_runs／coding_eventsを基点に、専用端末セッション、受信inbox、保留質問、判断outbox、委任権限を保存する拡張を加える。新設テーブル・型名は実装時に既存schemaと照合して確定する。frontendやrunnerに第二の正式なjob状態を置かない。

1. runnerはイベントにjob ID、run ID、provider session ID、event ID／sequence、question ID、種別、payloadを付け、一時保管を確定する。
2. SAAAは現在のセッションと送信者を照合し、既存Writerでinboxとcoding eventを同一transactionに保存する。
3. commit後にackしてwakeする。ackまでrunnerはイベントを保持し、再送を許す。hostは重複を採用しない。
4. 判断はoutboxへ保存してから配送する。runner側でもdecision IDを重複排除する。
5. 「送信済み」と「CLIが受理した」を分ける。結果が不明なら照合し、追加指示を無条件で再送しない。

イベント処理中にDB transactionを開いたままLLMやCLIの応答を待たない。Stewardのcursor前進と判断・報告予約を原子的に行い、再起動で質問や報告を落とさない。runnerのspoolは受信回復用であり、SAAAの意思決定台帳の代わりにはしない。保管量・保持期間に上限を設け、受信済みの重いログは成果物領域へ移す。

再起動時は、保存したsessionとrunnerのhandshake、プロセスidentity、spoolの配送位置を照合する。PIDだけで停止や再開を行わない。状態を確認できなければ結果不明にしてユーザーへ表示する。CLIがまだ動いている間にresumeを起動しない。

端末を閉じる操作は表示のdetachとし、job取消にしない。SAAAの停止ボタンは採用中runを取り消し、runnerと子プロセス群の終了を確認してから取消を確定する。SAAA自体の正常終了では既存停止方針との整合を確認し、無断で新しい「終了後も自動判断する」挙動を追加しない。クラッシュ後にrunnerが残った場合は保管・保留・再接続の契約で扱う。

## 7. 設定と操作画面

「実装エージェント」と「デフォルト端末」は別に選択する。既存Pi／Codex SDK設定を保ち、端末CLI方式を追加する。CLIの種類はCodex／Claude Code、端末はKitty／Ghostty。保存済みの選択を自動で入れ替えず、未設定なら初回利用時に選択できるようにする。

- 端末・CLI・tmuxの検出結果、使用する絶対パス、バージョン、連携準備状態を表示する。
- 端末未導入、CLI未認証、Hook未信頼、非対応版を区別し、必要な操作を案内する。
- 軽い検出確認と、費用・プロジェクト操作を伴う実接続確認を分ける。
- job開始時に設定をsnapshotし、実行途中の設定変更で接続先が変わらないようにする。
- 作業画面に「端末を開く」「追加指示」「質問に回答」「停止」を用意する。
- 質問は台帳から表示し、SAAAの回答理由とユーザー判断を履歴に残す。

内部のHook payload、socket、tmuxの識別子は通常の操作画面へ露出させない。必要な診断情報は別の詳細表示にする。CLIのログ・成果物に含まれる資格情報を報告や永続監査へそのまま複写しない。

## 8. 完了検証と報告

CLIのStop、exit code 0、最終メッセージは完了候補である。完了条件ごとに変更差分、対象ファイル、hostが取得したテスト結果、未解決質問、採用runとの対応を確認する。実装エージェントの「テスト成功」という自己申告だけでPassにしない。

hostで実行可能な確認は対象workspaceに対して行い、実行時点・結果・成果物の識別を保存する。検証失敗なら範囲内で修正を依頼し、上限を超えれば未完了として相談する。手動確認しかできない条件はAwaitingUser、証拠が不足する条件はMissing／Unknownのまま示す。

報告は既存Stewardのoutbox、会話表示、Situationによる通知・TTSの経路へ接続する。質問、失敗、検証済み完了を区別し、同じ報告を再起動で重複させない。通知のためにASR／TTSの契約を変更しない。TTS中も人の声が届くことと、TTSだけではASR発話にならないことを回帰確認する。

## 9. 実装順と各段階の出口

後続段階は前段階の契約を使う。CLI固有の制約が判明したら対応表と正本との整合を更新し、未対応を成功に見せない。

| 段階 | 作業 | 次へ進める条件 |
| --- | --- | --- |
| P0 接続の小規模検証 | 隔離workspace・データで両CLIの起動、質問、回答、保留、再開、終了を確認。session単位のHook設定と信頼・許可ホストを検証。Kitty／Ghosttyとtmuxの起動引数、版、配布を決める | 両CLIそれぞれの能力表、実際のイベント記録、未対応時の挙動、実行ファイルの版・識別が記録される。質問や許可経路が成立しない方式は採用しない |
| P1 契約・台帳・設定 | 委任権限、CLI方式、端末設定、question／decision／inbox／outbox、run保留状態、IPC、移行を実装 | 隔離DBで旧設定・旧jobを保持でき、二重回答・古いrun・重複配送が拒否される |
| P2 runner・表示・復旧 | Rust runner、socket、spool、プロセスidentity、tmux専用セッション、端末起動、再接続、停止を実装 | fake CLIで表示閉鎖、host再起動、ack喪失、取消を再現できる。runnerのパッケージングと署名を確認する |
| P3 Claude Code Adapter | 構造化実行、許可ホスト、Hooks、相談、defer、回答適用、正確なsession再開を実装 | 自動回答とユーザー待ちの両方で同じjobを継続でき、失敗・終了・保留を区別できる |
| P4 Codex CLI Adapter | exec構造化出力、Hooksの信頼、相談ツール、承認、終了確認後のresumeを実装 | 同じ共通契約を通り、Claudeと異なる対応範囲を準備状態へ反映できる |
| P5 Toolchain・判断・報告 | 現行Ornith入口、workspace解決、専用event origin、有限の自動回答、ユーザー相談、Steward検証・報告を接続 | テキスト／音声の依頼から起動でき、根拠に基づく回答・判断依頼・検証済み完了まで台帳とUIが一致する |
| P6 実機受入・仕上げ | 2CLI×2端末、クラッシュ・停止・再起動・設定変更を試験。UI・通知・音声の回帰と全体gateを確認 | 下記受入条件を満たし、版と実行証跡を記録する。未対応条件を明示する |

最初の着手はP0とする。Hookを受信できたことだけで、自動回答・完了検証まで実装可能と認定しない。P0の受入記録はdocs/plansに本計画と結び付けて残す。

## 10. 検証計画

### 資格情報なしで実行する決定的な試験

fake CLI／fake runnerと隔離DBを使い、以下を固定イベント列で検証する。新しい試験ファイル名は提案であり、現在存在するという意味ではない。

- terminal_agent_contract.rs: 設定snapshot、委任権限、保留と完了の区別、CLI能力不足、回答競合、古いrunの排除。
- terminal_agent_recovery.rs: 保存後ack喪失、重複イベント、送信結果不明、host再起動、spool再配送、PID再利用、取消後の遅延イベント。
- terminal_agent_delegation.rs: 既存仕様で答えられる質問、未決仕様の相談、権限外操作の拒否、自動判断上限、検証失敗、報告の一度だけの配送。
- terminal-agent-settings.test.tsx等: デフォルト端末の保存、未設定・未導入・未信頼の表示、選択の無断置換がないこと。
- 現行会話キューのfixture: ユーザーの依頼だけが委任開始になり、イベントで人間の発話権限を偽造しないこと。取消・回答が実際のjobへ届くこと。
- 移行fixture: 既存Pi／Codex SDK設定とjobが変わらず読めること。保留状態を加えたCHECK制約とactive制約が矛盾しないこと。

実装後に実行する対象gate:

```sh
bun run s11tnext:check
bun run ipc:generate
bun run ipc:check
bun run typecheck
bun test tests/coding-steward.test.ts
cargo test --manifest-path src-tauri/Cargo.toml --test terminal_agent_contract --test terminal_agent_recovery --test terminal_agent_delegation
bun run e2e:conversation-queue
bun run check:local
```

新設frontend試験も対象gateへ追加する。IPC生成後の差分は確認し、無関係な生成差分を混ぜない。新設Rust試験は配置・target登録を実装する。全体gateに既存失敗がある場合は変更起因と区別し、未検証・失敗を成功と記録しない。

### 実CLI・実端末の受入

実サービスを呼ぶ試験は明示的なliveモードへ分離し、専用workspaceとSAAAデータ領域を使う。ユーザーの通常DB・設定を使って移行や破壊的復旧を試さない。版、設定fingerprint、イベント列、検証結果を保存する。

| シナリオ | 期待結果 |
| --- | --- |
| 同じ小さな変更を2CLI×2端末で依頼 | 選択したCLI・端末で実行し、hostの確認が通った後だけ完了を報告 |
| 既に決まっている仕様を質問 | SAAAが根拠とともに一度回答し、同じjobが継続 |
| 仕様が未決の質問 | 作業を保留し、ユーザーへ選択肢と推奨案を表示。回答後に再開 |
| 委任範囲外の操作を要求 | 自動承認せず、必要な判断を示す。CLIのgateを迂回しない |
| テスト失敗、APIエラー、無出力 | 完了にせず原因を区別。有限の再試行後は相談・失敗・結果不明を表示 |
| CLIが成功終了するが完了条件が不足 | 結果確認から未完了・ユーザー確認へ進み、完了報告を出さない |
| 端末を閉じて再度開く | 表示が再接続し、依頼の再送や二つ目のCLI起動が起きない |
| SAAAがクラッシュ、イベントが重複 | 復旧照合・再配送で質問と結果を一度だけ採用。確認できないものは結果不明 |
| 停止と回答が競合 | 取消後に再開せず、対象プロセスの停止確認後に取消を確定 |
| 保存済み端末やCLIが消える・版が変わる | 選択を保持し、準備不足を表示。別の端末・Providerへ無断変更しない |
| 報告のTTS再生中にユーザーが話す | 人の声はASRへ届く。TTSだけの再生はユーザー発話として採用しない |

## 11. 実装完了の判定

両CLIで依頼→質問／相談→回答→再開→結果検証→報告が通り、Kitty／Ghosttyの選択が保存・使用されることを必須とする。追加指示・停止・復旧・二重配送の試験も完了条件に含める。単に端末が開く、Hookが来る、CLIが終了する段階では本機能を完了扱いにしない。

採用バージョン、能力表、実機受入記録を残し、利用できない経路はUIで準備未完または非対応として示す。将来の端末直接入力は、この初期版の受入後に別計画で扱う。

## 12. 参照資料

確認日はいずれも2026年10月4日。実装時には採用するローカル版との一致を再確認する。

- [Claude Code Hooks](https://code.claude.com/docs/en/hooks): 質問、許可、失敗、停止、印刷モードでの保留。
- [Claude Code CLI reference](https://code.claude.com/docs/en/cli-reference): 構造化実行、許可ホスト、sessionの再開。
- [Codex Hooks](https://learn.chatgpt.com/docs/hooks): 対応イベント、Hook設定、信頼確認。
- [Codex configuration reference](https://learn.chatgpt.com/docs/config-file/config-reference): features.hooksと設定経路。
- [Codex developer commands](https://learn.chatgpt.com/docs/developer-commands#codex-exec): 非対話実行と構造化出力。
- [Kitty invocation](https://sw.kovidgoyal.net/kitty/invocation/)、[Ghostty command設定](https://ghostty.org/docs/config/reference#command): 起動方式と引数。
- [tmux manual](https://man.openbsd.org/tmux)、[Control Mode](https://github.com/tmux/tmux/wiki/Control-Mode): セッション、表示接続、制御出力。
- src-tauri/src/coding/README.md、src-tauri/src/steward/README.md、AGENTS.md: 現行の実装・権限・復旧契約。
