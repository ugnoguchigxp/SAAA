# 会話からのTTS辞書訂正 検証記録

日付: 2026年10月1日。対象: 会話Tool、共有辞書保存、索引キャッシュ、辞書画面の同期。

## 実装

`lookup_tts_pronunciation` / `set_tts_pronunciation` を現行会話キューへ提示・接続した。未登録の明示訂正は追加し、同じ読みは書き直さない。異なる既存読みは確認待ちを作り、次の本人の返答で変更または維持する。今後の変更が明示された場合は直接変更できる。

SQLiteを正本とし、保存処理を画面とToolで共有する。変更時は一覧・索引を準備してからcommitし、その後キャッシュを公開する。同一値は書込み・索引更新を行わない。通常TTSのwarmキャッシュは辞書全件SELECTを実行しない。TSV自動取込は再開しない。

確認はホスト発行ID、会話・元ターン・実行中入力に結び付ける。公開済み質問の直後のユーザー入力に限定し、取消・別入力・再起動で失効する。モデルが意味を判定する責務と、Runtimeが実行資格・競合を検査する責務は分けている。

画面は変更通知を初回読込み前に購読する。最新要求の応答を採用し、未保存セル・focusを保持する。競合はセル付近で解決する。文字・読み方・削除の列、上部の選択読みテストは維持した。

## 自動検証

| 検証 | 結果 |
| --- | --- |
| 実際の共有サービス・保留・照合器を使うdomain test | 13件合格 |
| 本番Toolアダプタの隔離DB検証 | 15ケース合格。scope、未公開確認、token再利用、競合、取消、不正引数を検証 |
| warmキャッシュで辞書テーブルを削除した後の参照 | 同じArcを取得。辞書DB読込みなし |
| 会話キューintegration | 3件合格・実モデル1件は通常実行で明示ignore。辞書の10ターンを含む |
| stableコンテキスト方式の会話integration | 3件合格・実モデル1件ignore |
| 辞書画面と会話画面 | 11件合格。通知、競合、削除、逆順応答を含む |
| Rust辞書lib test | 9件合格 |
| TypeScript型検査・対象lint | 合格 |
| Vite本番build | 合格。既存chunkサイズ警告あり |
| 関連会話gate `bun run e2e:conversation-queue` | 合格 |

通常action形式に加え、実モデルが返した `action=lookup_tts_pronunciation` 形式を提示済み二つの辞書ツールだけで正規化する。fixtureもこの形式で照会する。Toolの引数と更新資格は通常の検査を通る。辞書Toolの途中応答が不正JSONだった場合は、既存の総step上限内で形式の修正を求める。fixtureに途中の不正応答を加え、登録が一度だけ完了することを検証する。

## 実モデル受入

設定済みLARMへ、隔離DBを使って9ケースの連続会話を複数回実行した。最終実装では、新規登録と同一読みの照会が実Toolで成功した。途中の試行では異なる候補の `confirmation_required` も確認した。

9ケース通しは未合格。Providerの `larm_authentication_failed` / `larm_provider_terminal` が試験を止めた。最終試行は `live-question` の認証エラーで停止し、以降の確認返答・維持・一時指定・引用・明示変更は実モデルでは最終検証できていない。前段の試行で見つかった行動形式の別表記、不適切IDとJSON形式崩れについて、限定した正規化、IDの明示指示、失効時の再照会、既存step上限内の形式復帰を追加した。意味判断品質を自動fixtureの合格から保証しない。

実アダプタの15ケースと、両コンテキスト方式の10ターンfixtureでは確認・維持・競合・取消が通っている。Provider設定や認証情報の変更による回避は行っていない。認証の安定後に明示live testで9ケースを再実行する必要がある。

## 性能の観測値

macOS上、debug build、in-memory SQLite、単発測定。音声生成・ネットワーク待ち・プロセスheap測定は含まない。上限保証や本番速度の予測には用いない。

| 件数 | 主キー照会1,000回 | 初回一覧＋索引構築 | warm置換1,000回 | 保存＋一覧＋索引構築 |
| --- | ---: | ---: | ---: | ---: |
| 1 | 9.10 ms | 0.020 ms | 0.545 ms | 0.050 ms |
| 5,000 | 4.09 ms | 3.62 ms | 0.521 ms | 3.54 ms |
| 10,000 | 3.94 ms | 6.82 ms | 0.521 ms | 7.28 ms |

warm置換の対象文は「用語00000の話」。読み照会は対象一件だけで、モデルへ辞書全件を渡さない。保存時の全件索引再構築は件数に応じて増える。heapが2 MB以内であるとの保証は今回行っていない。

## 全体gateと範囲の限界

`bun run check:local` は既存・並行変更の対象外ファイル44件の整形差分で停止した。全体合格とは扱わない。対象のコード整形と差分空白検査は別途行う。module-sizeの新規対象を登録し、追加した競合テスト、辞書adapter、既存CSSの整形による行数変更の対象baselineだけ明示レビューした。既存の別モジュールの値はリセットしない。

設定済みProviderは読み取りだけに使い、すべての辞書受入試験は隔離DBで行う。音声は送信文字列を取得して検証し、物理スピーカーは再生しない。マイクからの自然な訂正、TTSのみでASR発話が生じないこと、重なった本人発話が届くこと、実際の発音の品質はこの試験では未検証。音声入出力・AECの実装は変更していない。

## 文書との整合

正本の「SAAAの全体コンセプト」に、TTS辞書Toolが現行会話へ接続された範囲と、実音声受入が未確認であることを反映した。一般Tool発見・生成能力などの未接続扱いは保持した。

## 再実行

- `cargo test --manifest-path src-tauri/Cargo.toml --test tts_dictionary_conversation_contract -- --nocapture`
- `cargo test --manifest-path src-tauri/Cargo.toml --features conversation-queue-e2e --test conversation_queue_e2e`
- `SAAA_CONVERSATION_PREFIX_MODE=stable cargo test --manifest-path src-tauri/Cargo.toml --features conversation-queue-e2e --test conversation_queue_e2e`
- `bun test tests/tts-dictionary-page.test.tsx tests/conversation-queue-page.test.tsx`
- 実モデル: 設定済みharnessのaddressを `SAAA_LARM_CONTROL_URL` に与え、既存 `LARM_API_TOKEN` を使い、同じintegration testの `live_model_pronunciation_corrections_and_confirmation -- --ignored --nocapture` を実行する。認証情報を文書やログへ出さない。
