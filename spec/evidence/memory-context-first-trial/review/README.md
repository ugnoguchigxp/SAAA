# Context基盤のコードレビューと修正記録

2026年10月1日。対象は初回メモリー試験で追加した会話Contextの組み立て、公開と保存、通信計測、比較用の集計処理。見つかった以下の9点を修正し、対応する回帰を確認した。stableは引き続き明示指定で使い、既定のlegacyは変更していない。

## 修正した問題

| 優先度 | 問題と影響 | 修正と確認 |
| --- | --- | --- |
| P1 | Memory OFFでは、Scopeの関係が差し替わっても投影本文が同じなら古い根拠で回答を公開できた | Scope全体を取得時に保持し、公開前と保存時に比較する。fingerprintにも含める。activeなScopeのままcurrentからparentへ変更する試験は修正前に失敗を再現し、修正後は表示・読み上げ・保存前に拒否できた |
| P1 | 回答の保存に失敗すると一般のqueue再試行に流れ、公開済み回答の再生成やToolの再実行が起こり得た | 保存の失敗を終端の失敗にする。隔離DBで回答保存だけを拒否するtriggerを使い、jobの試行回数が1回であることを確認した |
| P1 | JSONの重複actionキーを通常のparserが最後の値として採用し、先にanswerを公開した後でToolを実行できた | 最上位の重複キー、非object、後続objectを拒否する専用parserを追加。重複actionの試験でToolが実行されないことを確認した。既に送信した仮回答を遡って取り消す仕組みではない。最終回答として保存せず再生成もしない |
| P2 | 履歴の引用中にMEMORY_PROJECTIONという文字列があるだけで、任意の履歴が必須扱いになった | 必須かどうかはContextEntryのhost側の属性で渡す。引用文字列を含む大きい任意履歴は除外でき、必須のWorld・Tool結果は除外できないことを確認した |
| P1 | 正しいJSONでもキー順が違うとstream抽出が始まらず、Memory ONではrun終了後の読み上げが拒否された | 検証済みの最終本文をrunが有効な間に既存の読み上げ経路へ送る。legacy／stable、Memory OFF／ONで一度だけ読み上げることを確認した |
| P2 | 読み上げが開始前に失敗した場合、保存後のspeech jobがqueuedのままになり自動再生へ進めた | 開始前でもerrorがあるreportをinterruptedとして保存する。最初の失敗を成功や未実行に置き換えない |
| P2 | SSEでusageを返す指定を送っておらず、計測の欠落をProviderだけの問題として扱っていた | 計測対象のSSEにstream_options.include_usageを付け、JSONへのfallbackでは付けない。fixtureが要求内容とusage-only eventの取り込みを検証する |
| P2 | 中断時のusage分類と時間の基準が揃わず、stream抽出できない最終回答では公開時間が欠けた | Drop時にmissingをdisconnectedへ分類し、Providerが返した0は保持する。通信と公開で同じ開始時刻を使い、公開イベント送信成功後に一度だけ記録する。キー順の違う回答にも公開時間があり、content時刻より早くならないことを確認した |
| P2 | 集計処理が非object、不可能なtoken値、同じexportの重複を適切に扱えなかった | 非objectを拒否し、同一行を重複集計しない。負数・非整数・cacheがinputを超える値を比率から除外する。欠けたdigestを固定Prefixの種類として数えない |

usage指定の根拠は[OpenAIのusage説明](https://help.openai.com/en/articles/10478918-reviewing-api-usage-and-costs)。指定を追加しても、実際のProviderがusageやcache token数を返すことは保証しない。

JSON不正の診断は種類・位置・bytes数だけを記録し、本文は記録しない。変更箇所の複雑な取消し状態の型には名前を付け、対象のClippy警告も解消した。新規4ファイルだけをサイズ基準に登録し、既存の基準値は変更していない。

## 検証結果

| 確認 | 結果 |
| --- | --- |
| Context比較と既存queue E2E、Tool後のProvider失敗 | 5件合格、実モデル2件はignore |
| Memory ONのContext比較 | 1件合格。legacy／stable各20ターンと異常系 |
| conversation_check unit | 10件合格。Compiler、JSON、stream抽出、queue進捗、replay |
| chat_completions unit | 6件合格。SSE、chunks、Drop receipt |
| task_queue | 6件合格 |
| tts_dictionary | 9件合格 |
| TTS chunker | 8件合格 |
| memory context_window | 20件合格 |
| offline report | 3件合格 |
| TypeScript型検査 | 合格 |
| stable指定のmacOS desktop smoke | 合格。build・bundle・起動・画面・IPC・DB・snapshot・speaker runtime準備 |
| feature付きlib Clippy | build成功。今回変更した箇所に警告なし。別の既存コードには警告が残る |
| 対象ファイルの整形と差分検査 | 合格 |
| 今回変更・追加したmoduleのサイズ | 合格 |
| プロジェクト全体のサイズ検査 | 不合格。別の既存moduleの超過・未登録・stale baseline・include分割が残る |

合格は合計68件。0件しか一致しなかった3つのfilter実行は合格へ加算せず、validation-results.jsonに区別して残した。音声入力やPCM処理は変更していない。今回の読み上げ回帰はfixtureによる本文の確認であり、実スピーカー再生中のマイク試験ではない。

[比較結果](comparison.json)と[Memory ON比較](memory-on/comparison.json)では、stableの固定Prefixは1種類、legacyは72種類。HTTP receiptの欠落は両modeとも0件。usageを返すfixtureは2試行だけであり、この比率から実キャッシュの効果を推定しない。firstVisibleMsはhostの公開イベント送信成功までの時間であり、画面描画の完了通知ではない。

## 互換性と未完了の受入

Scopeを含むfingerprintへ変更したため、旧版で保存した音声の再開用digestは一致せず、再生を拒否する場合がある。古い根拠を現在の根拠とみなす移行は行わない。DB schema、保存済み設定、Provider構成は変更していない。

このレビューでは実モデルの20ターン試験を再実行していない。前回の実モデル試験はlegacyの15ターン目、stableの9ターン目でJSON契約に合わず停止しており、受入は未合格のまま。今回のfixture合格やusage要求の修正を、その受入や実cache hitの確認として扱わない。前回の記録は上書きせず、今回の証跡をこのreviewディレクトリに保存した。

詳細な実行結果とログのdigestは[validation-results.json](validation-results.json)、対象コードのdigestは[source-manifest.json](source-manifest.json)、アプリ起動結果は[desktop-smoke/summary.json](desktop-smoke/summary.json)を参照する。Context取得と評価は今回1回ずつ、累計5回ずつ。
