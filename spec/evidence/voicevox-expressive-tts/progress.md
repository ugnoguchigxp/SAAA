# VOICEVOX expressive TTS progress

状態: 部分実装。live 受入は未実施。

| カード | 状態 | メモ |
| --- | --- | --- |
| VE-00 | 記録のみ | 既存 working tree は保全。本カードの所有対象は音声設定・speech request・予約語 parser・catalog 正規化・会話 SystemContext |
| VE-01 | 実装済み | Cloud/Harness の optional 韻律 field。旧 JSON は新 key を出さない |
| VE-02 | 実装済み | `SpeechRequest`。`voicevox-core` だけ追加 field を送る。Harness 解決と LARM 一時設定へ保存値をコピー |
| VE-03 | 実装済み | 保存済み provider / Harness から catalog を取得する IPC。close 失敗は成功にしない |
| VE-04 | 実装済み | Harness は保存済み address と credential で短期 Session を作り、終了時に close する |
| VE-05 | 接続済み | Cloud TTS が `voicevox-core` のとき話者・style・韻律 UI を出す |
| VE-06 | 実装済み | 先頭予約語と本文中 `[$...]` の除去、韻律差分と clamp |
| VE-07 | 接続済み | Delta は fan-out 前に projection。空 Delta は出さない。永続化直前にも同じ関数を通す |
| VE-08 | 接続済み | `SpeechWork` が expression を持ち、Cloud / LARM の合成前に韻律差分を適用する |
| VE-09 | 文面のみ | `respond.context.toml` と `.s11tnext/conversation-respond.txt` を更新。behavior eval は未追加 |
| VE-10 | 未着手 | 保存済み試聴と typed error |
| VE-11 | live 未実施 | offline Gate 全体は未実行。live は外部待ち |
