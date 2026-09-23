# ASR・初回応答の実装固定

`critical-path-freeze.json` は、チャットのマイクからASRまでと、`conversation.respond` の初回応答に使う実装のSHA-256を記録する。対象ファイルの追加・削除・変更があると `bun run freeze:check` が失敗する。CIとデスクトップアプリのビルドでもこのチェックを実行する。

| 領域 | 固定する経路 | 変更時の確認 |
| --- | --- | --- |
| `asr` | マイク取得、音声セッション、ASRルート・ストリーム | `bun test tests/ambient-voice-session.test.tsx tests/voice-capture-races.test.ts tests/voice-asr-packet-sender.test.ts`、`cargo test --manifest-path src-tauri/Cargo.toml voice::streaming_asr` |
| `initial-response` | 起動時の設定・主会話読込、会話入力、応答ルート、provider呼び出し | `bun run quality:check`、`bun run desktop:smoke` |

変更を明示的に依頼された場合だけ、対象のテストを通した後に `bun run freeze:accept:asr --reason "変更理由"` または `bun run freeze:accept:initial-response --reason "変更理由"` を実行する。基準ファイルの差分、理由、テスト結果を一緒にレビューする。設定画面からのモデル・接続先の変更はこの固定の対象外とする。

この固定はSAAA内の意図しない実装変更を検出する。LARMやモデルの稼働状態は別途、実環境の診断と監査で確認する。
