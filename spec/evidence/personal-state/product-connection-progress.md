# P1 製品接続の実装と実機確認

2026-09-13。既存dirty差分を残してSAAAの未接続部分を実装した。P1全体は未完了で、default OFFを維持する。

## 実装した経路

- `crates/larm-session/src/personal_state.rs`: 公開 `larm-personal-state.v1` API。短期Provider Bearerで配送・receipt照会・canonical計測・v2 View・generation attempt・取消・多層忘却へ接続する。
- `product_binding.rs`: 既存Agent Connectionを利用し、claimのsubjectとcapabilityのAllocation/runtime/release/lease/tokenizer/template/予算を照合する。owner–subject対応はmacOS Keychainで固定し、tokenはRust内の短期メモリに留める。手入力Certificationや固定env tokenを製品の認可根拠にしない。
- `product.rs` / `product/outbox.rs`: 各送信前に処理IDとgeneration依存をSQLiteへ記録する。原文・request本文はoutboxに複製しない。配送応答の紛失時は同じincarnationを照会する。View応答の紛失時はreceiptを照会し、未確認Viewを再consumeしない。future破棄時も利用停止とcleanup所有権を残す。
- `product_cleanup.rs`: attempt停止と、attempts/views/runtime/snapshots/registry/sources/auditの全phaseのabsenceを照合する。HTTP 200、transport切断、一部成功だけで完了にしない。機能OFFで新generationを拒否しても、認証済みcleanupは続けられる。
- SQLite v18: `personal_remote_operations`、`personal_product_binding`、phase名と状態だけを保持する `personal_remote_results` を追加。旧v17のSQLは変更していない。忘却後に到着したreceiptから識別可能なdigestが再保存されることを防ぐ。
- `personal_state_harness`: `quality-eval-harness` featureでビルドする有限受入プログラム。隔離したDBに合成sourceを保存し、実workerと永続projectionを通す。goldはモデルへ渡さない。製品経路を通すコードは追加済みだが、下記の実機ブロッカーにより64×3の受入は未完了。

設定画面では遠隔消去の7段階を表示し、一部の消去だけで完了表示にならない。診断には原文・token・対象digestを保存しない。

製品経路は旧v1模擬Adapterへfallbackしない。旧HTTP fixtureは回帰試験として残している。

## 検証

新規の製品HTTP試験は以下を確認する。

1. 配送応答紛失後のreceipt照会、v2 View、caller attempt、全phaseのcleanup。
2. 別subjectのreceiptを登録前に拒否。
3. snapshot absenceがpendingならcleanupを未完了として保持。
4. 配送中のforget後は登録・generationへ進まず、遅着receiptのdigestを保存しない。
5. 製品gate OFFで新generationを拒否し、既存cleanupを維持。
6. source保存後、応答受領前にfutureを破棄しても、incarnationからcleanup可能。

`bun run check:local` と desktop smokeの実行結果は `product-connection-results.json` に記録する。実モデルの品質・性能はこれらのローカル試験で認定しない。

## 実機で確認した事実

LARMの作業ツリーに製品用APIが追加済みだった。既存daemonは旧版のため、localhost:9811で合成data用の一時daemonを起動した。metadata/source/journalは本番と分離した。LARMの機能flagの本番既定値は変更していない。

- 起動時にPersonal State journalのUUID型へ接頭辞付きdaemon IDを渡して失敗する不具合を発見した。journal自身がboot UUIDを発行する既存機能を使うよう、LARM `main.ts` の呼出しを修正した。起動とtypecheckを確認した。
- 一時daemonの起動時orphan回収が既存workerの停止を報告したため、既存workerのhealthを回復させ、その後の一時daemonはorphan回収猶予を86400秒に設定した。別consumerが管理するruntimeを自動回収する方式は検証用にも使用しない。
- `/tmp` の空き容量は認定free floor未満だった。下限を緩めず、1TB以上空いている `/home/ugnoguchi/.cache/saaa-personal-state-canary` に合成sourceの保存先を変更した。
- `qwen-general` / `qwen-general-current` の常駐27Bでは、配送→登録→canonical計測→v2 View→実generationを通過し、attemptが`completed/http_200`となった。
- 忘却はattempts/views/snapshots/registry/sources/auditがabsent、runtimeだけがstop_unknown。全体は`result_unknown`、`absenceVerified=false`だった。合格にしていない。
- 原因はエンジンのslot actionが`--slot-save-path`なしではHTTP 501になること。エンジン改修は不要。ただしlauncherのspeed profileは明示slot-save-pathを転送していなかったため、明示指定時だけcache reuseを有効化せずに転送する修正と回帰試験を追加した。
- 本来の `qwen-worker-quality` の再検証では、別の27B推論が実行中だったためスロット確保がtimeoutした。この試験のpending ConnectionはDELETE 204で解放した。

一時daemonは停止済み、合成ConnectionはDELETE 204で解放済み。合成sourceファイルは0件、本番healthは正常。未完了forgetのjournalは再確認のため保持している。この実機確認は公開APIのconformanceであり、SAAA native harnessの実行結果ではない。

## 管理者による適用待ち

変更は `host-runtime-changes.patch` に収録した。gnosisで `/etc/systemd/system/llama-server.service` とリポジトリのservice定義との差分はslot-save-pathの1行のみである。保存先は作成済み。launcherの修正はソース作業ツリーに反映済みだが、稼働中プロセスには再起動まで反映されない。

このSSH接続の`sudo -n`は`interactive authentication is required`となり、こちらから適用できない。実行中・待機中の推論がないことを確認して管理者が適用する。

```sh
sudo install -m 644 /srv/ai/apps/local-LLM-harness/deploy/local-node/systemd/llama-server.service /etc/systemd/system/llama-server.service
sudo systemctl daemon-reload
sudo systemctl restart llama-server.service
```

適用後は同一forget IDの再送でruntime absenceまで確認し、製品SAAA harness、snapshot OFF/ON、race、rollback、64件×3反復、cold/warm各100件と資源計測を行う。restartだけでforget完了と判定しない。

日本語goldは人手確認待ち。v2では局所条件8件の入力IDを期待するrequest IDへ揃えた。期待値の意味レビューとモデル実測は未完了。比較表は `gold-review.md`。
