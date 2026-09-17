# Product readiness evidence

このdirectoryには、公開可能な製品受入結果だけを保存します。認証情報、接続先、会話本文、音声、個人path、完全なHTTP logを保存しません。

各runは別directoryにし、scenario ID、commit/build digest、working tree状態、OS、匿名化した環境label、所要時間、成否、安定した失敗code、反復数、検証日時を記録します。dirtyなworking treeの結果は一般配布の根拠にしません。失敗後の再実行は元の結果を残し、原因と変更内容を添えます。

実施手順と判定基準は `spec/docs/product-readiness-acceptance-runbook.html`、全体状態は `spec/docs/product-readiness-status.html` を正本とします。
