# Personal State P1 実装・契約確認記録

> この文書は当初の調査・実装時点の記録。製品API公開後の追加実装と現在の残課題は [製品接続の進捗](product-connection-progress.md) を参照。以下の「未接続」「作業範囲を確認中」は当時の状態であり、今回の追加実装には適用しない。

確認日: 2026-09-13。P1全体は未完了。実ユーザーsourceの配送、実モデルgeneration、LARMの設定変更は行っていない。default OFFを維持する。

## 今回の実装

| 範囲 | コードと確認内容 | 到達点 |
| --- | --- | --- |
| P1-01 | `crates/personal-state-core`。型、reducer、CAS前提、認可、依存閉包、coverage、時刻投影、View binding | 決定的coreの17試験が通過 |
| P1-02 | `src-tauri/src/memory/personal_state/{schema,store,sources,journal}.rs`、schema.sql、SqliteWriter | additive v17。正本本文の重複なし。全件ページング、版更新、UTF-8範囲、同一txのjob/patch保存、忘却閉包。別journalを照合してDB復元時の再出現を拒否 |
| P1-03 | `crates/larm-session/src/contexts.rs`、`managed.rs`、`generation.rs` | 登録、View、3 header、materialization照会、one-shot、required/期限/binding、登録absence照会。ローカルHTTP fixtureで登録からcleanupまで検証。追加のinference/registration経路では複数required Source、新規View、request digest、重複attempt拒否、drop時unpinを検証。製品用Delivery実装は未接続 |
| P1-04 | `conversation.rs`、`output.rs`、Runtime、既存coding/UI tool writer、session_store | base-only/required Source Viewの各推論を別attemptとして記録。Task同梱候補は回答と同じtxで採用し、未知の出典や別requestへの拡張を拒否。保存/tool実行/最終通知で再検査。forget時cancel/TTS停止・画面再読込・遅延イベント破棄。2Bの既存shadow境界は維持 |
| P1-05 | `jobs.rs`、`worker.rs` | idle 30秒、同時1、30秒/1呼出し、2,000 token、初回＋3 retry、lease/input epoch CAS、部分抽出candidate、全文最終化、coverage。request限定の候補は元入力の安定IDに結び、共有/別requestへ投影しない。外部processは保守的に全実行時間を共有slotで直列化 |
| P1-06 | `commands.rs`、`PersonalStateSection.tsx`、diagnostics | owner向け状態/根拠/原文preview、forget、pending/cleanup表示。全体診断には本文を含めない。配送未設定をreadyと表示しない |
| P1-07 | 64件の日本語draft corpus、`personal-state-eval.ts`、`personal-state-performance.ts` | 3反復・固定gold・範囲の照合とcold/warm各100件の集計gate。humanReviewed=falseなので品質認定不可。personal-state:liveで64件×3回の有限収集と子process timeoutを実装。製品harness未接続・実機の全受入行列は未実施 |

migration前の最新schemaは他作業による16だったため17を追加した。旧migrationは書き換えていない。実ユーザーDBは開いていない。開始時HEADは `cc8c66c586ed98ab0dd493b4fe796678e577da36`。既存dirtyと並行中のcoding/pi差分は保持している。新規モジュールのサイズbaselineを登録した。

## 固定できたLARM契約

原本はgnosis `/srv/ai/apps/local-LLM-harness`、HEAD `631c15ad84eacb757770c92b21d647743cfeae6c`。release/configにdirty差分があるため、このHEADを実稼働認定版とは扱わない。SSHは原本の読み取りにのみ使用した。

- `packages/core/src/context.ts`、`apps/daemon/src/context-controller.ts`: POST contexts、POST context-views、GET context-operations、GET contexts、DELETE contexts、View schema v1。
- Registrationには本文を含めずsourceHandle/digest/bytes/tokens/tokenizer/classificationを渡す。公開descriptorの各値を要求と照合する。
- `x-larm-allocation-id`、`x-larm-context-view-id`、`x-larm-capability`。base-onlyはView headerを付けず別purposeで記録。
- DELETE成功でも機能OFF中はno-opとなる。全ページのabsenceとsource/snapshot cleanupを分ける。HTTP future取消を遠隔停止の確認とは扱わない。
- hostの `apps/daemon/src/context-source-cli.ts` はprovision CLI。Macからの製品用配送・canonical計測・削除APIであるとは確認できていない。
- 20Mはsource quota。旧文書例の262,144 native / 32,768 reserve / 4,096 margin / 225,280 inputは本番保証として使用しない。2026-09-16の認定releaseは131,072 context / 4,096 output reserve / 1,976 safety margin / 125,000 inputであり、SAAAも125,000入力・4,096出力を上限とする。

## 残る依存と完了条件

| 依存 | 実装所有者・接続先 | 完了条件 |
| --- | --- | --- |
| source配送・canonical計測・消去 | LARM deployment側の公開契約。SAAA側 `managed::Delivery` | 同principalの許可済み配送、handle/attestation、正確なchat-template計測、孤立source照合、source/base snapshot消去を提供。未公開HTTPやSSH bridgeを推測しない |
| principal/Allocation/release binding | 既存Agent ConnectionとSAAA Provider Adapter | ローカルowner IDから認証principalへの対応、Allocation lease・semantic認定・tokenizer/予算・失効更新を実データで固定。現在のCertificationは保管/照合型であり製品の自動接続実装ではない |
| 遠隔取消・snapshot | LARM公開契約とSAAA cleanup reconciliation | HTTP取消の送信時間と遠隔停止を別記録。baseだけの依存も含めsnapshot再利用を確実に禁止。未確認はpendingを維持 |
| 全推論経路 | SAAA Runtime/Provider Adapter | 複数Source/View、request限定抽出、Task同梱patchのSAAA処理は追加済み。認定済みProviderおよび外部coding subprocessとのbindingは上記製品契約へ接続する必要があり、全経路対応ではない |
| 実機受入 | SAAA側の有限live runnerとレビュー担当 | goldを人手確認し、64件×3回を実施。C1〜C15の残る競合注入、snapshot OFF/ON、cold/warm、ASR/TTS・資源測定を実施し証跡を保存 |

`Adapter::configured` のDeliveryは現在UnavailableDeliveryである。機能をONにしても動作する完成品とは報告しない。不足契約が確定するまでモデルなしの検証を進める、という計画第2節の境界を維持する。

## 忘却と復元の保証範囲

DB内のpayload・派生会話本文・transitionの内容・manifestを消去し、opaque ID/時刻のjournalをDBと別ファイルへfsync後に成功を返す。v17のDB復元時にjournalがない、壊れている、principalが違う場合はopenを拒否する。管理外backup、OS snapshot、SSDの物理secure erase、遠隔停止・source消去はローカル試験で保証していない。

## 検証

最終の `bun run check:local` が成功。Rust全体565件（既存ignored 17件）・integration 5件、frontend 353件、Personal State core 17件、SQLite/HTTP統合16件が通過。clippy、format、frontend build、IPC、typecheck、lint、size、specも通過した。desktop smokeは前回追加時に通過しており、今回の追加経路の実機推論は行っていない。最終結果は `offline-results.json` を参照。モデルなしのfixture成功をlive性能や意味品質の認定へ読み替えない。

```sh
bun run check:local
bun run test:rust-packages
bun run desktop:smoke
bun run spec:check
bun run personal-state:eval results.json report.json
bun run personal-state:performance samples.json report.json
```

最後の2コマンドは実測結果の採点器。追加した `bun run personal-state:live deployment.json evidence.json` は有限のharness実行器で、製品SAAA harnessの接続は未完了。仕様は `live-runner.md`、LARMへの依存は `larm-delivery-dependency.md` を参照。固定corpusの本文は合成データのみで、実ユーザー本文・credentialを証跡へ出力しない。

## 追加実装の保証範囲

- request限定状態はSAAA入力message IDをscope keyとする。retryは同じ入力IDに結ぶ。別requestの条件を暗黙に共有へ変更しない。
- 会話の未反映原文をrequired Sourceへ移し、各tool往復もimmutable request digestと新Viewで扱う。認定token/byte上限を超える必須入力は停止する。
- 既存coding/UI toolの出典が厳密でない場合、最大64MiB/4096範囲のprimary会話原文を保守的な依存集合として記録する。これはモデルの入力予算やLARM20M token quotaとは別のローカル追跡上限。超過時は明示エラーにし、追跡を切り詰めない。
- Taskの候補採用は全件抽出完了の証拠とせず、workerのcoverageを勝手にcompletedへ変えない。
- 実接続と実機認定を止める条件はLARMの製品配送/計測/消去と認証binding。LARMリポジトリへの追加実装を本タスクに含めるかユーザーへ確認中。
