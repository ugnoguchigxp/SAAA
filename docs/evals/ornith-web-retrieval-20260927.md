# Ornith Web調査の修正（2026-09-27）

## 調査で確認したこと

本番DBは読み取りのみ使用した。`conversation-web-tool-result` では検索5件の取得とページ本文の取得成功が記録されており、Web検索全体が動いていない状態ではなかった。

- 最新の天気問い合わせでOrnithが「鎌倉市 天気 今日 2025年」を検索していた。実行日は2026-09-27で、調査担当のコンテキストに現在日時が明示されていなかった。
- 別の問い合わせではOrnithが検索結果から「セマンティックレイヤー」の可能性を説明していたが、後段のQwenが別の意味に書き換え、以前の天気の話まで混ぜた。取得した結果がユーザーに届くまでに失われる実例だった。
- コード上はツール実行が最大2回だった。検索と1ページ取得で使い切り、失敗したページの代わりを取得できなかった。
- 実行したassistantの行動JSONを履歴に追加していなかった。また、ツール結果JSONをシリアライズ後に6000文字で切っており、引用符のエスケープ等で膨らんだ結果が壊れる場合があった。この2点は回帰テストで検証した実装上の欠陥であり、過去の全失敗原因と断定はしない。

## 変更

1. 調査開始時のローカル日時・UTCオフセットを実行時コンテキストに注入。
2. ツール実行枠を6回にし、その後に回答用の1ターンを確保。残り回数をモデルへ通知。空結果・取得失敗・関連情報不足では検索語変更や別ページ取得を行うよう指示。
3. 行動と結果を対で履歴に保存し、取得層が上限を設定したWeb結果JSONを途中で切らずに渡す。
4. Web検索・本文取得を既存のキャンセル対応ディスパッチへ接続。公開HTTP(S) URLを既存の取得先検証へ渡す。
5. 後段Qwenが確定済みのOrnith本文を変更した場合、監査ログに記録し、保存とTTSにはOrnith本文を使う。Qwenの中継失敗でも取得済み回答を失わない。保存前の本文検証とキャンセル確認は維持。

保存済み設定・モデル・ユーザーの会話履歴は変更していない。

## 検証

- 会話キューE2E成功。実ワーカーと隔離DBで「検索→1件目の取得失敗→別ページ本文取得→回答→保存→TTS」を実行。
- 8000文字を超えるエスケープ済み結果JSONがパース可能な状態でOrnithへ渡り、本文末尾の根拠が読めることを確認。
- Qwenが別の文章を返しても、保存本文とTTSにはOrnithの確定した結論・出典が残ることを確認。
- 検索後にモデルが失敗するケース、認証失敗からの再接続、不正な内部思考の排除、キャンセル、待機音声の既存ケースも成功。
- 明示実行の `conversation_queue_web_live` 成功。実際のRust検索プロバイダーが5件を返し、その結果URL `https://doc.rust-lang.org/stable/book/` から本文2682文字を取得（retrievalStatus=relevant）。この試験はHTTP検索・HTML取得の実サービス確認で、実モデルを含むマイクからの通話試験ではない。
- `git diff --check` 成功。

コマンド:

```sh
cargo test --manifest-path src-tauri/Cargo.toml --features conversation-queue-e2e --test conversation_queue_e2e --test conversation_queue_followup_failure --test conversation_queue_web_live
cargo test --manifest-path src-tauri/Cargo.toml --features conversation-queue-e2e --test conversation_queue_web_live -- --ignored --nocapture
```
