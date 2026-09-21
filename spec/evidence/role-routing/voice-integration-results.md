# Role Routing 音声統合 — 実装中の疎通証跡

実施日: 2026-09-21

この記録はVC実装途中の実測であり、実マイク・TTS・UIまで含むVT11完了証跡ではない。

## 設定

- 通常ライブラリビルド + `provider-diagnostics` の診断バイナリを使用
- policy version: 3
- Role Routing: enabled
- frontend: `local-conversation-frontend`
- reasoner: `local-reasoner`
- LFM出力: `say` / `think` の2項目JSON
- 実reasoner model: `qwen3.8`

## 実機疎通

実行:

```text
src-tauri/target/debug/harness_llm_diagnostic <control-base-url> --frontdesk
```

最初のLFM単独試験では、具体的な旅行計画依頼を`think=false`としたため不合格。この結果を受け、会話文を作るLFMと、思考開始要否だけを判定するQwenを並走させた。

再試験結果:

| 入力 | 判定 | LFM応答時間 |
|---|---:|---:|
| 挨拶 | `think=false` | 1975ms |
| 話の途中 | `think=false` | 2188ms |
| 具体的な旅行計画依頼 | `think=true` | 2250ms |
| Qwen思考中の追加入力 | `think=false` | 1128ms |

最終コードでのQwen最終回答は3893msで受信し、空でない60文字の日本語回答を確認した。Qwen補助判定は2500msで打ち切り、reasoner slotが利用できない場合もLFM応答を止めない。

## 自動試験

- LFM契約・音声receipt・Qwen reasoning request・旧DB列移行: 4件成功
- 最終回答によるLFM発話preemption: 1件成功
- Role Routing: 160件成功。並列時にCodex sidecarの短時間timeoutが4件発生したが、該当10件を直列再実行して全件成功
- ambient voice UI: 2件成功

## 未検証

- 実マイクからASR、LFM、Qwen、実TTS、UI再読込までの一連のVT11
- Qwen最終回答が実際の再生中LFM音声を停止する500ms実測
- 30発言・10往復の実マイク反復
