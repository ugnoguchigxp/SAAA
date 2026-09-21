# Role Routing 音声統合 — コードレビュー・疎通証跡

実施日: 2026-09-22

この記録は本番と同じLARMセッション、LFM応答パーサー、Qwenストリームを使ったLAN実測である。実マイク・実TTSを含む手動E2Eの代替ではない。

## 設定

- 通常ライブラリビルド + `provider-diagnostics` の診断バイナリを使用
- policy version: 5
- Role Routing: enabled
- frontend: `local-conversation-frontend`
- reasoner: `local-reasoner`
- frontend timeout: 8000ms（実測約2.5秒に対する有限のLAN揺らぎ余裕）
- LFM出力: `say` / `think` の2項目JSON
- 実reasoner model: `qwen3.8`

## 実機疎通

実行:

```text
src-tauri/target/debug/harness_llm_diagnostic <control-base-url> --frontdesk
```

LFMの`think=true`は常に保持する。並走するQwen分類は、LFMが見落とした場合に`false`から`true`へ昇格できるが、LFMの思考依頼を取り消せない。LFMはQwenへ会話を委譲せず、その後の発言にも応対し続ける。

再試験結果:

| 入力 | 判定 | LFM応答時間 |
|---|---:|---:|
| 挨拶 | `think=false` | 1913ms |
| 話の途中 | `think=false` | 1967ms |
| 具体的な旅行計画依頼 | `think=true` | 2137ms |
| Qwen思考中の追加入力 | `think=false` | 1144ms |

最終コードでのQwen最終回答は4407msで受信し、空でない79文字の日本語回答を確認した。Qwen補助判定は2500msで打ち切り、reasoner slotが利用できない場合もLFM応答を止めない。

レビューでは、Role Routing経由のQwenだけがLFMと別セッションになる条件分岐も修正した。現在は`provider`ルートでも、音声のreasoning requestかつDynamic LAN providerなら同じLARMセッションを再利用する。

## 自動試験

- LFM契約・音声receipt・再送・Qwen reasoning request・旧DB列移行: 6件成功
- 最終回答によるLFM発話preemption: 1件成功
- Role Routing: 165件成功
- frontend: 365件成功
- Rust clippy（lib、warnings deny）: 成功
- Rust全体試験では1511件成功後、既存テスト不整合2件を修正。残る1件はコード失敗ではなく、固定fixtureが要求するBun 1.3.14に対して実行環境がBun 1.4.2であるための環境検証失敗

## 未検証

- 実マイクからASR、LFM、Qwen、実TTS、UI再読込までの一連のVT11
- Qwen最終回答が実際の再生中LFM音声を停止する500ms実測
- 30発言・10往復の実マイク反復
