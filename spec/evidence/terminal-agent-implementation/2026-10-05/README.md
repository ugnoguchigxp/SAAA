# 専用端末エージェント連携の受入記録

確認日: 2026年10月5日 JST。対象は[実装計画](../../../../docs/plans/toolchain-terminal-agents.md)の初期専用端末経路。製品コンセプトの正本は[Page](https://chatgpt.com/space/page_9fc5877949748191b556705128f6a2f5)。新たなProvider方式やSDKの完成認定とは分ける。

## 実装と確認の結果

設定に専用端末方式、Kitty／Ghostty、Claude Code／Codex、CLIパス、モデル、完了確認処理、自動回答・有限修正を追加した。操作は「作業」画面、実装依頼・追加指示・文字での質問回答は現在の会話Toolchainを使う。正式な状態はhostのcoding台帳で、外部イベントを人間の発話として登録しない。

独立runner／MCP／Hook／表示は同じSAAA実行ファイルの別モード。tmuxを導入せず、端末を閉じてもrunnerは独立する。イベントは私有spoolへ確定後にwakeし、hostが重複排除して採用する。回答前にCLI終了を確認し、保存sessionだけを再開する。許可は同一tool inputへの人間の1回の判断に限定する。

| 確認対象 | 結果と範囲 |
| --- | --- |
| ヘルパーcrate | 10件成功。私有保存・イベント並行書込・引数・取消・再送拒否・MCP即時応答・同一許可の1回適用・bin symlinkとNode探索 |
| coding全体 | 38件成功、4件は明示実行が必要な既存live／統合試験として通常実行ではignore |
| host往復 | ignore指定試験を明示実行して成功。両fake CLIについて、実SAAA native runner→実Hook/MCP→台帳→人間の回答→正確なsession再開→result.txt→hostのgrep→Steward pump→会話報告まで通過。再drainで報告を重複emitしない |
| frontend全体 | 143/143ファイル、433件成功。端末選択時の旧設定保持と、CLI終了前の回答禁止・許可のscoped送信を含む |
| TypeScript・frontend lint・build・文脈生成・IPC生成 | 成功。frontend buildには既存のbundleサイズ警告がある |
| IPC・task queue・会話進捗・TTS復旧／streaming・音声型契約 | 対象6 test targetすべて成功。集計はverification.json |
| 実CLI×実端末 | Claude Code 2.1.289／Codex 0.155.1 × Kitty／Ghostty、4通りすべてで実SAAAバイナリから表示起動・同一session・完了候補・正常終了を確認。live-matrix.json |
| Claude native質問 | AskUserQuestion→Hook defer→tool_deferredで停止→Blue回答→同一session再開を実CLIで確認 |
| Claude native許可 | /usr/bin/trueだけを隔離workspaceで依頼。既定denyと質問保存→取消→同一sessionで明示承認→1回消費→正常終了・完了候補を確認。final-canaries.json |
| Codex相談MCP | host内の非破壊・閉じたツールを正しいMCP annotationで提示し、相談→host取消→保存回答→exact resume→完了候補を実CLIで確認。codex-host-cancel-canary.json |

実CLIの4通りは表示・プロトコル・再開の小規模受入であり、4通りすべてに本格的なコード変更と目的判定を行った証明ではない。コード変更・host検証・台帳・UI回答・報告の決定的な往復は資格情報なしの統合試験で検証した。実SAAA会話モデルによる自動判断、通常業務の長期運用、実マイクでのTTS割込み、配布署名・notarizationを今回のlive証跡には含めない。音声ロジックは変更していない。

## 制約と失敗時の扱い

- 自動回答はopt-inで、元の人間の依頼に明記された答えだけ。同一質問1試行・job全体3判断・30秒以内。資料を調べて新しい判断をする段階は含まない。権限はモデルに承認させない。
- Codexはworkspace-write／approval never。sandbox外操作の承認経路は非対応で、相談または失敗として返す。Claude側は許可ホストを使う。
- 確認処理未登録、手動条件が残る、候補不足、確認失敗は完了にしない。修正の自動依頼はopt-in・最大2回。確認中の再起動では確認を再実行せず、ユーザー確認へ戻す。
- 最長実行30分、確認処理120秒/件、最大8件。入力・イベント・spoolに上限がある。通常CLI出力1行は60KB以下を受け入れ、超過やspool上限では正常完了にせず停止する。私有ログの自動削除は行わない。
- アカウントのCodex既定モデルgpt-6.1-solと隔離指定gpt-5.4は拒否された。保存設定は変更せず、隔離canaryに明示したgpt-5.6-lunaで成功した。モデル可否はアカウント依存。設定の接続確認は配置・版の検出であり、認証やモデル応答を保証しない。
- 起動・結果不明は無条件に再送しない。保存したPIDだけでは停止しない。停止確認できない対象は手動確認を要求し、元の結果不明を成功へ書き換えない。

## 全体gateと再確認

全Rust library試験も実行したが、旧会話経路の異常系3件が8分以上終了せず、所有する試験プロセスだけを停止した。schema versionを43のまま期待する試験は今回の44へ更新し、個別実行で通過した。残る旧typed memory試験は単独実行でも、廃止されたcontinue_workと現行TTS辞書ツールの一覧差で失敗する（existing-isolated-existing-tests.txt）。当該期待値・提供ツールの変更は本機能の変更箇所ではない。全library suite成功とは記録しない。

全体size gateとClippy ratchetには変更前からの未登録・古いbaseline・未使用コード等の失敗が残る。新設端末モジュールを登録し、今回の契約拡張だけをmodule-registration.jsonに記録した。新設モジュールの警告は解消し、最終production Clippyに端末モジュールの警告はない。本端末経路の登録はmodule-registration.jsonで追跡し、全体gateの残る失敗は別記する。

受入の再実行中、共有target/debug/saaaがhelperを持たない古い内容に置き換わり、fixtureが待機失敗した。確認済みバイナリの固定コピーを使用して成功を再確認した。最終native SHA-256をverification.jsonに記録した。ビルド容量不足も発生したため、古い・使用中でないincrementalキャッシュだけを整理して再確認した。ユーザー設定・CLI設定・資格情報は初期化していない。

再現コマンドと所有・配送契約は[src-tauriの実装README](../../../../src-tauri/src/coding/terminal/README.md)。完全なCLIログには依頼や端末環境が含まれるため、ここには小規模受入の結果のみを保存した。
