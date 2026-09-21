# WorldModel実装残件の完了結果

更新日: 2026-09-21。対象: Terra残件計画 T00〜T24。

## 判定

**実装残件完了／製品受入未完了**。

共通Frame v2、五要素graph、Situation、Coding、委任、期限、自然文抽出、訂正・forget、全adapter、検証済み現在状態回答、Scope UIまでを接続した。offlineでは6経路×7遷移の42ケースを実際の最終wireまたは能力契約で検証し、全件合格した。

製品受入には、稼働中の実Providerを使う30例、実UI、実音声、TTFA比較、プロジェクト全体の既存size/Clippy違反の解消が残る。最終確認時もLAN Provider `192.168.0.130:8080` は接続不能だった。ローカルservice `127.0.0.1:44449` はHTTP 200だが、`enabled=false`、`ready=false`、`workerAlive=false`だったため、fixture合格をlive合格として扱わない。

## 最終検証

| 対象 | 結果 |
| --- | --- |
| 6経路×7遷移 | 42/42合格。訂正・forget後は旧`Speculative Decoding`要素を送信せず、Scope変更はmodel wire前に拒否 |
| reasoning MCP Tool継続 | host Tool能力を持たない契約のためN/A。`unsupported-capability`を必須理由として記録 |
| World remaining runner | T01〜T21 prefix、0件・欠落・重複・失敗、42セル欠落をfail-closedで検査 |
| 性能 | debug、2,000 ledger、100投影、Coding 8件、期限8件、30標本。追加p95 **5.235001ms**、基準30ms以内 |
| Frontend | 326件合格 |
| TypeScript | typecheck合格 |
| IPC | 5件合格 |
| Rust packages | 合格。World remainingは36件合格、42セル一意、error 0 |
| 担当module size | World関連の新規fileを登録し、担当fileのratchet違反なし |

性能のraw sampleは`wr_t23_source_frame_additional_p95`の`WORLD_REMAINING_PERF`出力に保存した。測定範囲はprepare、serialize、revalidateであり、実ProviderのTTFAではない。

## 実装中に追加で解消した不具合

- 忘却済みの根拠sourceに依存する五要素assertionが再投影される問題を修正した。query時に全source依存が現在availableであることを確認する。
- Codex app-serverはthread作成後、turn/start直前にFrameを更新する。thread作成待ちでTTLを使い切ったFrameを送らない。
- DynamicLan fixtureはprofile revision、claim、semantic health、credential、releaseを実契約どおり通す。共通HTTPだけを試す代替にはしていない。
- provider fallbackとsession resumeは新しいprovider sessionと最新履歴を使い、前attemptのreceiptへ新bodyを結び付けない。
- route matrix runnerは前回reportを先に無効化し、42セルが一意に揃わない限りcompleteにしない。

## live受入への引継ぎ

稼働接続先を用意した後、親計画WD-13の各経路5例、計30例を実行する。実モデル回答のclaim誤り0、source mismatch 0、正常例をhost縮退だけで通さないこと、実音声のASR確定後ScopeとTTS hold、実UIのA→B表示、TTFA悪化10%以内を確認する。
