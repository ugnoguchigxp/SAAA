# VOICEVOX expressive TTS progress

状態: 部分実装。live 受入は未実施。

| カード | 状態 | メモ |
| --- | --- | --- |
| VE-00 | 記録のみ | 既存 working tree は保全。本カードの所有対象は音声設定・speech request・予約語 parser・catalog 正規化・会話 SystemContext |
| VE-01 | 実装済み | Cloud/Harness の optional 韻律 field。旧 JSON は新 key を出さない |
| VE-02 | 実装済み | `SpeechRequest`。`voicevox-core` だけ追加 field を送る。Harness 解決と LARM 一時設定へ保存値をコピー |
| VE-03 | 正規化のみ | rich/simple catalog と不正 catalog の拒否。HTTP・IPC・timeout は未接続 |
| VE-04 | 未着手 | LARM 短期 Session の catalog lifecycle |
| VE-05 | 未着手 | 設定 UI |
| VE-06 | 実装済み | 先頭予約語と本文中 `[$...]` の除去、韻律差分と clamp |
| VE-07 | 未着手 | EventHub fan-out 前の projection と永続化 |
| VE-08 | 未着手 | `SpeechWork` への expression 同梱 |
| VE-09 | 文面のみ | `respond.context.toml` と `.s11tnext/conversation-respond.txt` を更新。behavior eval は未追加 |
| VE-10 | 未着手 | 保存済み試聴と typed error |
| VE-11 | live 未実施 | offline Gate 全体は未実行。live は外部待ち |
