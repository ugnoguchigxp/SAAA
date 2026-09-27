# LARM接続・認証失敗の回復確認（2026-09-27）

## 観測

17:52〜17:53の監査ログでは、ASRの `larm_invalid_provider` / `larm_provider_terminal` と、Ornithの `Provider authentication failed. Check the configured credential.` が別々の要求で発生していた。画面のASRエラーとOrnith実行表示は同一ジョブの状態ではない。

設定済みLARMへの診断要求でも、HTTP 201 / status=ready と全5プロバイダーの readiness=ready / claimable=false の組み合わせを確認した。その後、同じ接続先で claimable=true と claim HTTP 200 を確認した。接続先の状態は調査中に変化しており、サーバー内で拒否状態になった原因までは特定していない。診断中の一度のclaim HTTP 400は診断スクリプトの余分なIdempotency-Keyによるものなので、製品障害の根拠には含めない。

## 修正

- readyであっても準備未完了・claim不可なら拒否を継続し、それぞれ `larm_provider_not_ready` / `larm_provider_not_claimable` で区別する。接続エラーには日本語の説明を付ける。
- 会話モデルの認証拒否時は、その要求が使用していたセッションと一致するキャッシュだけを破棄する。並行して作成された新セッションは破棄しない。既存の実行中リースは途中で閉じず、次の要求は新しい接続を取得する。
- 同じ認証情報のまま認証失敗を3回再試行しない。Ornith失敗は既存の結果処理へ渡す。
- 途中認識が接続エラーを表示した後、後続認識に成功しても残っていたASRエラーを消去する。古い要求の成功で新しい失敗や別のマイク障害を消さない。
- ASRエラーと会話処理エラーを別表示にする。キューの過去の失敗をReactの独立したerror状態へコピーせず、最新の状態から表示する。
- ASR停止・設定初期化・モデル変更は行っていない。

## 検証

- larm-session: 62テスト成功（55 unit + 4 contexts + 3 budget）。通常実行ではLAN向け3テストを除外。
- LAN向け `live_saaa_selector_two_llm_session` を明示実行し成功。製品のSession処理でSAAAを接続・claimし、QwenとOrnithの実HTTP応答200、releaseを確認した。音声会話全体の実機試験ではない。
- 会話キュー・失敗後結果処理・進捗処理の既存結合テスト成功。
- 認証失敗の追加E2Eは、Ornith 401が1回で終わり、新規接続を取得してQwenが失敗を説明し、その保存本文をTTSへ渡すことを確認した。
- 画面テストは失敗後の進捗表示終了、成功後の古いエラー消去、ASRと会話エラーの併記を確認した。

認証情報やclaimトークンは記録せず、診断で作成した接続はDELETEで解放した。保存済み設定と会話履歴は変更していない。

追試: 認証回復E2E成功、ASR/IPC 18件成功、画面テスト成功、TypeScript型検査成功。進捗スケジュールの既存テストには作成時刻と予定時刻の採取差（1 ms）による不安定さがあり、処理前後の実時刻で10秒後を挟んで検証するよう修正した。
