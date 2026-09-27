# 会話反復と読み上げ本文の整合性調査（2026-09-27）

## 確認できた事実

保存DBを読み取り専用で調査した。17:28台の反復には7個の異なる発話IDがあり、ASR結果からそれぞれユーザーメッセージが作られていた。同一IDのキューが無限に再実行された記録ではない。

2件目の録音は17:28:12.657に確定し、ASR結果は17:28:13.109に返っている。最初のTTS再生開始は17:28:15.210であり、反復全体をTTSの回り込みだけで説明することはできない。

当時はBluetoothでVoiceProcessingを使わない保存設定によりブラウザー録音へ移行していた。ただし、この事実だけから音響エコーを原因とは断定しない。録音・再生PCMとProviderの非発話確率は当時の監査記録に残っておらず、聞こえた音声とログの意味内容の不一致は遡及検証できない。

## 変更

- 再生開始・終了・AEC状態変化をASRの停止や強制確定条件にしない。ユーザー発話の終端はVAD/ASRまたは明示的な停止が決める。
- 全文保持のASR経路でもProviderの非発話判定を尊重する。従来は全文保持フラグが非発話判定まで無効にしていた。文字列一致による一律の重複排除は行わない。
- Qwen回答を会話本文と音声ジョブとして同一トランザクションで保存する。TTSは保存済みメッセージから読み、保存前に回答を音声ワーカーへ送る経路を削除する。
- 保存時に読み上げ用本文を確定し、画面専用の出典リンクだけを別添する。空本文・制御タグを拒否する。
- TTSに渡した本文とメッセージIDを監査記録へ追加する。ASRには録音のSHA-256、サンプル数、RMSを記録し、音声の重複送信と認識結果の反復を区別できるようにする。生音声は保存しない。
- 保存済み設定やProviderは変更していない。

## 検証

- `bun test tests/conversation-asr-continuous.test.ts tests/qwen-realtime-asr-ipc.test.ts`: 17件成功。HTTP・Realtimeで再生中の入力継続と、再生状態変化による強制確定がないことを確認。
- `bun test tests/conversation-queue-page.test.tsx`: 1件成功。
- `bun run typecheck`: 成功。
- `cargo test --manifest-path src-tauri/Cargo.toml --features conversation-queue-e2e --test conversation_queue_e2e --test conversation_queue_followup_failure --test conversation_queue_progress_contract`: 10件成功。実ワーカーの通常経路、10秒待機、非発話文字列の拒否、未保存回答の読み上げ防止、重複投入、キャンセル、Provider失敗を確認。DB・Providerは隔離したfixtureを使用。
- `git diff --check`: 成功。
- ライブラリー単体テスト全体のコンパイルは既存のworld/context等の未解決importで失敗。今回のASR非発話ケースは上記の結合テストでも検証済み。

## 残る確認

修正版を起動した実機で、当時の反復が解消するか、実際に聞こえる音声が保存本文と一致するかの受入確認は未実施。追加した監査記録により、再発時は発話ID・録音の識別値・読み上げ本文を対応付けて調査できる。AECの物理的な効果をモックテスト成功で代替しない。
