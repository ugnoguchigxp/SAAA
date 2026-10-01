# 固定Contextの比較結果

構造上はstableを支持する。Memory OFF／ONとも、同じ提示Tool・Policyで固定digestが一つに揃った。実cache利用や実会話品質の合格は示していない。

## 決定的fixture（Memory OFF）

| mode | 論理要求 | HTTP試行 | 固定digest | usageあり | cache値欠落 | HTTP失敗・取消 | 初回content中央値ms | 初回公開中央値ms | 完了中央値ms |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| legacy | 68 | 69 | 68 | 2 | 68 | 8 | 1 | 3 | 2 |
| stable | 68 | 69 | 1 | 2 | 68 | 8 | 1 | 3 | 2 |

[集計](fixture-comparison.json)、[legacy計測](legacy-requests.jsonl)、[stable計測](stable-requests.jsonl)。主試験は20ターンだが、件数には既存の会話・辞書・取消とusage／fallback／partial／上限／失効ケースの全stepも含む。失敗は意図した異常系。68 logical requestsに69 HTTP attemptsが対応し、JSON fallbackの追加試行を一つの成功だけへ丸めていない。receipt欠落は両方0。

## 決定的fixture（Memory ON／Worldあり）

| mode | 論理要求 | HTTP試行 | 固定digest | usageあり | cache値欠落 | HTTP失敗・取消 | 初回content中央値ms | 初回公開中央値ms | 完了中央値ms |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| legacy | 68 | 69 | 68 | 2 | 68 | 8 | 1 | 4 | 4 |
| stable | 68 | 69 | 1 | 2 | 68 | 8 | 1 | 4 | 3 |

[集計](memory-on/comparison.json)。WorldとScopeは動的参照領域にあり、fixed digestへ混ぜない。豊富な本人World graphの答えの正確さを測るfixtureではない。

両fixtureのusageありは、usage-only SSEとJSONの各1試行。SSEのcacheReadTokens=0はfixtureが明示した値。残る欠落は0と区別する。cacheReadRatio=0はこの合成1標本だけの値であり、実Providerにcacheが無いという意味ではない。

## 実モデル（途中終了、受入は未合格）

| mode | 論理要求 | HTTP試行 | 固定digest | usageあり | cache値欠落 | HTTP失敗・取消 | 初回content中央値ms | 初回公開中央値ms | 完了中央値ms |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| legacy | 22 | 22 | 22 | 0 | 22 | 0 | 14715.5 | 17395 | 19154 |
| stable | 11 | 11 | 1 | 0 | 11 | 0 | 12799 | 12947 | 17602 |

[集計](live-comparison.json)。legacyは14／20ターン、stableは8／20ターンの成功後にJSON契約不一致で停止。HTTP outcomeは成功でもqueueは不正JSONを拒否する。表のHTTP失敗0を会話成功と読まない。実接続の検索後にはlarm_provider_terminalの案内も返り、Tool往復の品質を合格にはできない。

usageは全てmissing。cache readはunknownで、明示0ではない。固定digestがlegacy22種類、stable1種類だったことまでは観測した。成功件数、Tool回数、再接続、履歴長、cache温度が揃わないため遅延の改善率を判断しない。値は探索的中央値であり、p95・非劣性・費用削減の根拠には使わない。

## 計測の意味

固定digestは送信System本文のSHA-256。messagePrefixBytesは、同構成・同接続の直前要求との「完全一致したmessageのserialized byte数」の合計下限であり、token prefixやProvider cacheではない。日時とpendingは可変末尾、現在入力は最後に一度だけ置く。

firstContentMsはSSEで初めて非空contentを受けた時刻。answerの公開前にJSONのaction文字列等が来ることがあり、firstVisibleMsと別。JSON完了応答にSSE TTFTを捏造しない。usageは本文・speechへ送らず、request／attempt／visibleのIDで結ぶ。生usage、認証header、個人会話本文は計測に含めない。

集計の再実行:

```sh
bun scripts/conversation-context-report.ts spec/evidence/memory-context-first-trial/legacy-requests.jsonl spec/evidence/memory-context-first-trial/stable-requests.jsonl
bun scripts/conversation-context-report.ts spec/evidence/memory-context-first-trial/live-legacy-requests.jsonl spec/evidence/memory-context-first-trial/live-stable-requests.jsonl
```
