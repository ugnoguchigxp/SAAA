# Role Routing 学習・夜間処理契約

状態: 設計。R3でdataset生成・集計・shadow実行まで実装する。学習モデルの訓練と本番適用は後続段階。
[全体計画](saaa-role-routing-plan.md) / [実行契約](saaa-role-routing-execution-contract.md)

## L0. 学習する対象

候補は「actor」だけでなく「実行recipe」。推定対象は状況に対する各recipeの有用性。モデルの名前や使われた頻度をそのまま正解にしない。APIのHTTP 200、toolの正常終了、ユーザーの満足、客観的な課題達成を別々の指標にする。

R3の成果物はデータ契約、定期抽出、統計ranker、offline replay、shadow artifact loader。基盤LLMのfine-tuning、自由なagent graph生成、実トラフィックの探索、本番ranker自動昇格は含めない。ML artifactの形式と評価条件は本節で固定し、後続で作り直さない。

## L1. 記録とデータセットの単位

1例は`decisionId + labelRevision + featureVersion + labelerVersion`。単なる1メッセージを1例にしない。単位内に選択前の状況、当時の候補集合、選択行動、選択後の観測を持つ。

```json
{
  "schemaVersion":1,
  "exampleId":"ex_fixture_01",
  "decisionId":"decision_fixture_01",
  "groupId":"conv_fixture_01",
  "decisionEventSeq":101,
  "labelRevision":1,
  "policyVersion":1,
  "featureVersion":"rr-features-v1",
  "labelerVersion":"rr-observed-v1",
  "features":{"origin":"voice","trigger":"answer_challenge","priorActorRole":"reasoner","hasActiveWork":false,"inputBytesBucket":"short","contextHealth":"green","toolNeed":"unknown","worldState":"unknown","remainingSteps":3,"cloudAllowed":true},
  "eligibleCandidates":["direct","advanced_rethink"],
  "selectedCandidate":"advanced_rethink",
  "selectionMode":"rules",
  "selectionProbability":null,
  "labels":{"technicalSuccess":true,"userAcceptance":null,"verifiedTaskSuccess":null,"correctionRequested":false,"latencyMs":12000,"costMicros":null},
  "labelSources":["verified_outcome"],
  "sourceRefs":[{"kind":"routing_decision","id":"decision_fixture_01"}],
  "eligibleForTraining":false,
  "exclusionReason":"quality_label_missing"
}
```

featuresの種類・順序はversionごとに固定。v1はorigin、trigger、priorActorRole、hasActiveWork、inputBytesBucket(short<=256/medium<=4096/long)、contextHealth、toolNeed(none/read/mutate/unknown)、worldState(idle/busy/unknown)、remainingSteps、cloudAllowed。actor/model version、recipe ID、resourceGroup、過去の成功統計は別metadata。各統計はdecisionEventSeqより前の観測だけで構築する。

禁止特徴: 選択後のユーザー反応、最終正誤、後から修正したworld状態、将来のlatency、選択後に採用したmodel名を予測用特徴に混ぜること。必要な現在actorはdecision時点の値を使う。

本文・workspace path・tokenはdatasetに書かない。ローカル補助ラベラーは元記録をscope内で読むが、datasetにはsourceRefと構造化ラベルだけを残す。groupIdはexport時にdataset内saltで匿名化する。内部来歴の元IDはSQLiteに保持する。

## L2. ラベル生成

| 観測 | 生成label | 備考 |
| --- | --- | --- |
| step成功、root回答採用 | technicalSuccess=true | 正答とは限らない |
| 明示的「その回答で解決しました」+対象一意 | userAcceptance=true | 客観的正解とは分離 |
| 「結論が違う」+対象一意 | correctionRequested=true | verifiedTaskSuccess=falseとはしない |
| テスト/計算/構造制約をhost検証 | verifiedTaskSuccess=true/false | 検証器版と証拠IDを保存 |
| 「本当に？」、沈黙、別話題 | userAcceptance=null | 負例にしない |
| ユーザー中止 | technicalSuccess=null | cancellation reasonを別保存 |
| 条件追加で旧結果破棄 | outcome=superseded | 初期の品質訓練から除外 |
| reviewerの指摘だけ | model_inferredラベル | 観測正解とは別列、初期訓練から除外 |

ラベルの根拠はrr_feedbackとrr_events、検証済みtool結果。複数の矛盾するexplicit labelは最後の明示訂正までの履歴を保持し、`label_conflict`で訓練対象外にする。対象回答が一意でない場合は保留。モデル推定とuserラベルを平均しない。

初期の品質値q: verifiedTaskSuccessがある場合その0/1、なければ明示userAcceptanceの0/1。両者が矛盾すれば除外。技術成功だけの例は品質学習に使わない。latencyは`min(latencyMs / rootTimeoutMs,1)`、costは価格版と上限が既知の場合のみ正規化する。欠損値に0円・0msを代入しない。候補比較で欠損軸がある場合は全候補でその軸を外し、残りの重みを再正規化する。

## L3. 夜間ジョブと起動条件

`role_routing/learning/scheduler.rs`をアプリsetupから一つ起動し、shutdownで中断する。外部cronやCodex automationには依存しない。`learning.enabled`はユーザー設定で有効化。実装時の初期値は02:00〜05:00、端末のローカル時間、idle 300秒、1回600秒、batch 100件。wall clockとmonotonic clockはtraitで注入する。

60秒ごとのtickで、enabled、日付未完了、夜間windowまたは前夜missed、idle、foregroundなし、Meeting録音なしを検査。条件不成立はskip理由だけを記録し、エラーにしない。スリープ後は次のidleでmissed jobを再開。日付キーは起動時のlocal date+timezoneと保存し、DSTや時計の巻戻りで同一jobを二重生成しない。

処理段階: extract → label → assemble → evaluate → publish_shadow_manifest → completed。明示状態はqueued/running/paused/completed/failed。pause理由はforeground/shutdown/time_budget。学習モデル訓練はこのv1 pipelineに含めない。

読み取り境界は開始時のrr_events最大seq。各pageで`last_seq < seq <= upper_seq ORDER BY seq LIMIT 100`。transactionはpage単位。rootIdをdirty queueに入れ、境界以前の入力・結果・feedbackだけを再構成する。既存rowを書き換える操作も必ずrr_eventを同時追加する。eventにはその時点のlabel/status/usageの構造化snapshotと不変sourceRefを保存する。夜間抽出はこのsnapshotから境界時点の値を復元し、upper_seqより後に更新された現在rowを境界内の状態として読まない。row.updated_atだけで増分処理しない。root終端eventがupper_seq以前にない例はpendingとして除外し、後の終端eventで再抽出する。

ローカルラベラーが本文を読む際はsource version/digest/epochを照合し、抽出中に変更・削除されたsourceはskipしてdirtyを残す。短いpage transactionであっても一貫性を失わないよう、page内の参照とcursor更新の対応を確認する。

後日feedbackが追加された場合は新eventから同じdecisionのlabelRevisionを増やす。過去datasetはimmutable、次版で最新labelを採用する。削除時はL7のdirty処理と失効を行う。1ページの保存とcursor更新を同じwriter transactionにし、書込失敗後はcursorを進めない。

foregroundが始まったら次のpage前に中断し、実行中の任意local labelerへcancelを伝播。1.2B/Qwenのslotが空くまで待たせて会話開始を遅らせない。CPU集計はpage単位でyield。高コストなnightly処理をモデル起動時のcritical pathに入れない。

## L4. 追加tableとファイル

全tableはC6と同じmigration入口へ、R3時点のschema version+1の加算migrationとして登録する。出荷済みR1 migrationを後から書き換えない。夜間処理用の別SQLiteは作らない。

| table | 列 | 制約 |
| --- | --- | --- |
| rr_learning_jobs | id, job_key, stage, status, upper_seq, cursor_seq, dataset_id, error_code?, created_at_ms, updated_at_ms | PK id、UNIQUE job_key、cursor<=upper、stage/status CHECK |
| rr_learning_dirty | root_id, cause_seq | PK root_id、root FK CASCADE、cause_seqは最大値へupsert |
| rr_examples | id, dataset_id, decision_id, label_revision, feature_version, labeler_version, features_json, labels_json, eligible, exclusion_reason?, created_at_ms | PK id、UNIQUE(dataset_id,decision_id,label_revision,feature_version,labeler_version)、decision FK CASCADE |
| rr_example_sources | example_id, source_kind, source_id, source_version, scope_key | 複合PK、example FK CASCADE |
| rr_datasets | id, upper_seq, feature_version, labeler_version, manifest_json, digest?, state, created_at_ms | PK id、state=building/ready/invalidated/failed |
| rr_ranker_artifacts | id, dataset_id, algorithm, feature_version, candidate_fingerprint, weights_json, metrics_json, digest, state, created_at_ms | PK id、dataset FK、state=candidate/shadow/invalidated/retired |

rr_examples.dataset_idとrr_learning_jobs.dataset_idはdatasets FK。rr_learning_jobs.dataset_idはextract前のdataset作成と同一transactionで設定する。source参照は複数の既存tableを指すため多相IDとして保持し、削除hookで検査する。rr_example_sources(source_kind,source_id)、rr_learning_jobs(status,updated_at_ms)をindexにする。

dataset directoryはアプリdata_directory/role-routing/datasets/{id}/。manifest.json、examples.jsonl、metrics.jsonを.tmpへ出力し、hash確認後rename。manifestを最後に確定する。DBがreadyかつdigest一致の場合だけ読む。実機評価の報告はrepositoryのspec/evidenceへ出すが、個人dataset本体をrepositoryへ書かない。

manifest項目: schemaVersion、datasetId、eventUpperSeq、featureVersion、labelerVersion、generatorVersion、policyVersions、exampleCount、excludedCounts、splitGroups、fileHashes、createdAt。本文や秘密を含めない。

## L5. R3の集計rankerとshadow

初期は同じtrigger×origin×recipeの組で観測集計を作る。eligible例が20件未満ならscoreを出さずrulesへ。20件以上は品質qのBeta(1,1)事後平均`(successes+1)/(n+2)`と観測latency/costの中央値を返す。これは選択バイアスを除去した因果推定ではないため、R3ではshadow専用。少数例で「このモデルがベスト」と表示しない。

shadowはreal decisionと同じ候補・featuresを受け、別の推薦を保存するだけ。Providerを二重起動しない。real選択との一致率、対象不足率、遅延overheadを測る。一致率の高さを品質改善の証拠としない。

## L6. ML artifact契約と後続の訓練

artifactはJSONでschemaVersion、algorithm、featureVersion、candidateFingerprint、trainingDatasetId、weights、normalization、missingValuePolicy、metrics、digestを持つ。任意Python pickleや実行scriptをアプリでロードしない。R3で実装するloaderは`rules-v1`、`empirical-v1`、`linear-v1`の検証と純粋なscore実行まで。linear-v1はfeature vectorの順序・normalizationを固定し、候補ごとにbias+dot(weights,features)、sigmoidで品質推定0〜1へ変換する。fixture weightsで契約試験を行う。

後続訓練の最初の候補は、選択されたrecipeの観測品質を予測するL2正則化logistic regression。固定seed、固定featureVersion、同一datasetで再現する。未選択候補のlabelを0として訓練しない。新モデル版は別candidate fingerprintとしてcold startし、同じrole名だからと旧成績を無条件に継承しない。

学習開始の最低条件はeligible品質例200件、対象recipeごと30件、独立した会話group20件。これは十分な精度を保証する数ではなく、訓練しない条件を明確にする下限。不足時はinsufficient_dataでskip。

splitはconversation group単位で最新decision時刻順に70/15/15。root lineageや同一評価fixture派生は同group。validation/testにそれぞれ最低3group、label class両方がなければ昇格不可。学習・評価の特徴は各decision時点の値だけ。

実トラフィックの未選択結果は分からないため、昇格には安全な再現タスクで固定ルールと候補rankerを同じモデルpoolで比較する。読み取り・合成fixtureだけを使用し、mutation toolは記録済み結果のmockへ差し替える。IPS等は実際の探索確率がないログに適用しない。

昇格条件の設計値: hard constraint違反0、難課題の誤ったfrontend完了0、固定baselineに対するtask success差の95%信頼区間下限が-2ポイント以上、latency/costの少なくとも一方が改善し他方の悪化<=10%、shadowにprotocol error0。少数データで区間が広い場合は保留。R3は自動昇格を実装せず、証跡を作るところまで。

## L7. 削除・失効・再開

会話削除、memory forget、scope revokeではsource参照からexamples/datasets/artifactsを失効させる。元記録を消してから次の夜間までartifactを有効のままにしない。同一writer transactionでartifact stateをinvalidatedにし、rankerはdispatch前にstateを確認してrulesへ戻る。filesystem削除はjournalにより再試行し、失効済みartifactを読み直さない。

既存forget journalを拡張する削除hookを使い、独立した削除の正本を作らない。厳密な影響範囲を特定できない古いartifactは当該profile全体を失効させる。モデル重みから個別の情報を除去できたと主張せず再訓練を必要とする。

再起動時running jobはpausedへ。manifest.tmpをready扱いしない。完成manifestとDB stateが不一致ならhashを検査し、publishを冪等に再試行する。成功未確認のfile書込やDB writeをcompletedにしない。

## L8. 運用・試験・失敗時の扱い

実装予定command `run_routing_learning_once`はアプリ内の同一writerを使い、手動で1回のtickを依頼するだけ。enabled/offやforeground状態を無視するforce flagは付けない。テストはin-memory DB、FakeClock、FakeForeground、fixture labelerを使い、認証なしで完結する。

nightly失敗はユーザーの通常会話を失敗させない。Settingsに最後の成功、処理件数、除外件数、paused/failed理由を表示する。入力本文をエラーへ出さない。retryは次tick、最大3回/日、永続protocol errorは翌日まで停止。日付変更による永久retryを防ぐため連続失敗日数も表示する。

性能gate: page100件のDB write p95<=50ms、foreground通知からbackground推論cancel要求まで<=100ms（仮想時計試験）、nightly稼働時の実機TTFA p95悪化<=10%。非達成ならbatchSizeを下げるかnightlyをpauseし、会話を優先する。

## L9. 実行結果の帰属と統計の再現

root最終回答の明示評価は、その回答を作成したdecisionへ結び付ける。review→reviseというrecipe全体の評価も同じrootのrecipe decisionへ記録し、独立した成功例をstep数だけ水増ししない。中間reviewerには正答labelを与えず、検証されたissueの数・採用/棄却を補助指標として記録する。rr_feedback.target_decision_idがnullなら品質訓練に入れない。

datasetのfeaturesとlabelsのJSONはkey順を正規化し、examplesをdecisionEventSeq、decisionId、labelRevisionで安定sortする。digest対象に生成時刻・ランダムなdatasetIdを含めない。manifestには別途job時刻を残す。同じ抽出境界・source版・generator/labeler版ならexamples digestが一致する。

R3のempirical scoreは`qualityWeight*q - latencyWeight*l - costWeight*c`。unknown軸は全候補から外して重みを再正規化する。品質ラベルがない候補にはscoreを付けず、rulesを使う。C3のhard filterと継続性判定をscoreの後で省略しない。linear-v1も品質推定をqに入れるだけで、モデル出力を実行命令として使わない。
