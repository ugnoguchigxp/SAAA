# 初回の動作確認と次の判断

**固定Contextの実装と隔離試験は成立。実モデルでの20ターン受入は未合格。** この記録を初回の停止地点とする。長期メモリ全体の完成とは扱わない。

## 実際に確認できたこと

Memory OFF／ONそれぞれで、旧配置と新配置の20ターン、検索→取得→回答、途中の訂正を含む送信・保存・speech本文の流れが完了した。新配置は固定digest一つ、Runtime一件、現在入力一件。残り回数は毎step最新。辞書pendingの追加・確認・承認／棄却、取消、先行TTS、再開の既存回帰も確認した。fixtureの固定回答はモデル理解を証明しない。

source削除とScope失効は、最初の回答deltaより前に発生させ、公開本文・保存回答・speechに失効根拠の本文が出ないことを確認。容量不足はRequiredを落とさず送信前に拒否する。Toolを無制限に求めるfixtureでも6回で止まり、案内文を一度だけ読み上げる。部分回答後の切断はjobを終端し、Tool後でも別回答を再生成しない。

stable指定でmacOSアプリをbuildして、bundle起動、画面表示、IPC、会話・snapshot読み込み、speaker runtimeの準備が通った。[最終smoke結果](desktop-smoke-final/summary.json)。物理スピーカーでの再生、人の割り込み、体感の良否はこのsmokeからは分からない。ASR/AECの音声処理は変更していない。

## 実モデルで残った問題

[legacyの記録](live-legacy-status.json)は14ターンを完了して15ターン目、[stableの記録](live-stable-status.json)は8ターンを完了して9ターン目で「Ornithの行動結果がJSON契約に合いません。」となった。HTTP自体の成功と、会話のJSON契約・job成功は別。receiptのHTTP outcome=successを実会話の合格に読み替えない。

legacyの10・11ターン目では「音声も短く確認」「Profile／Episodeは次段階」が実回答へ反映された。stableはその訂正まで到達していない。stableの実回答には、依頼以上の長い説明や内部のRuntimeラベルの説明もあり、初回の会話品質を合格にはできない。固定fixtureの合格でこの結果を埋めない。

実モデルのusageは全試行で欠落。実cache read、cache hit、費用の改善は不明。時刻から得た遅延は参考値のみ。新旧で失敗地点・成功件数が異なり、改善率を出さない。

## 試験中に直した問題

- 失効・終了・認証失敗したLARM接続を次の依頼で再利用しない。同じ設定で接続を取り直し、Provider交換や設定初期化をしない。
- Memory ONでTool上限時の案内文だけ、完了済みrunのWorldアクセスが拒否されていた。通常の根拠検証付きspeech経路をrunが有効な間に使い、閉じたrunを再許可しない。
- 公開した部分本文の後で失敗しても、別の案内文や再生成結果に置き換えない。JSON制御文のchunk分割による流出も防ぐ。

## 次に着手する順序

1. 現行Ornith／LARMのJSON出力契約を、実際の不正応答とProviderが対応する形式指定で確認・修正する。今回のContext比較には形だけの寛容なJSON補修を加えていない。
2. 同じ隔離条件で実モデル20ターンを両mode再実施し、訂正後の保持、短い回答、Tool往復を回答本文で評価する。実機の短い読み上げと割り込みも確認する。
3. Provider側usage／cache診断を確認する。未対応ならunknownを維持し、構造上の安定化と効果測定を分ける。
4. その結果を見て、Snapshot／公開EpochとCheckpoint＋Tailへ進む範囲を決める。Profile・Episodeはこの段階には含めない。

既定化は保留。限定試用は `SAAA_CONVERSATION_PREFIX_MODE=stable` を起動環境に指定、切り戻しはlegacy（または変数削除）。保存済み会話・設定・辞書を消す操作は不要。
