# 経験を次の選択・計画・通知へ反映する実装計画

作成日: 2026-09-21。状態: 設計、実装未着手。担当想定: Terra。

## 1. 解消する弱点と完成状態

目的は「結果やfeedbackを記録するが、次回の仕事の進め方が変わらない」を解消すること。ユーザーの明示訂正は次の対象判断へ、検証済みの経験は評価と昇格を経て以後の選択・計画・通知へ反映する。変更理由を説明でき、取り消せ、元の記憶を忘れたら影響する学習成果も失効する。

完成条件はdatasetやshadow artifactの生成ではなく、**同じ条件で次の実際の行動が改善されること**。モデル選択だけでなくTool、有限の実行手順、通知方法の四領域を対象にする。基盤LLMのfine-tuning、任意のagent graph生成、無断の実トラフィック探索は導入しない。

上位は [Personal AI Concept](saaa-personal-ai-concept.md) §12〜15、[Adaptive Learning Concept](saaa-adaptive-learning-memory-concept.html)。[Role Routing学習契約](saaa-role-routing-learning-contract.md)のL0/L5/L6にあるshadowまでの制限を、本書で限定的なproduction適用まで拡張する。L1/L2のラベル区別、L7の削除、権限・Scope境界は維持する。旧執事フェーズのRole Routing凍結は同フェーズ内の制約であり、本書は完了後の別フェーズとする。

## 2. 前提と現行実装

[重要Context](saaa-required-context-completion-plan.md)、[委任仕事](saaa-delegated-work-completion-plan.md)、[全経路への状態供給](saaa-world-delivery-completion-plan.md)を統合受入まで終える。学習が壊れた基本経路を隠して補う構造にしない。AI-00〜05のイベント・純粋集計は先行できるが、実行選択への適用は前提完了後。

| 現行接続点（`src-tauri/src/` 相対） | 再利用と不足 |
| --- | --- |
| `tool_selection/{feedback,rules,ranking,service}.rs` | 明示訂正から選択を変える経路がある。二つ目の訂正台帳を作らず連携 |
| `role_routing/{selection,ranker,repository_turns}.rs` | rules選択、実turnの観測、shadow rankerの基礎 |
| `role_routing/learning/{repository,scheduler,schema}.rs` | dataset/成果物基礎。production policyへ昇格する経路が必要 |
| `steward/` / `schedule/` | DWで追加するplan/verifier/reportの結果と判断を接続 |
| `memory/personal_state/journal.rs` | 忘却からdataset/artifact失効まで追跡 |
| `runtime/conversation_inputs_roles.rs` | 実Provider選択へ反映する入口。複数actor recipeの実行能力は別途照合 |

着手時点で学習repositoryのdirty境界がrowidかevent sequenceかを再確認する。rowidをevent時点と同一視しない。現在rowの値を過去時点の特徴として読まない。

## 3. 改善対象と禁止境界

| 領域 | 学習・訂正で変えてよいもの | 変えてはいけないもの |
| --- | --- | --- |
| Provider/recipe選択 | 適格な実行済みrecipe間の順位。品質・待ち時間・費用のtrade-off | cloud許可、費用上限、未対応actorの実行、モデル名だけによる優遇 |
| Tool選択 | 適格候補の順位、確認済みの用途別選好 | 不許可Toolの追加、実行grant、引数検査の省略 |
| Task計画 | 登録済み有限plan recipeの選択、許可step順、不要と検証された再試行の抑止 | Goalの書換え、委任拡大、任意step生成、verifierの緩和 |
| 通知 | 許可された表示方法、まとめる時間、非緊急報告の遅延 | mute/会議holdの解除、重要期限の隠蔽、通知ゼロで高評価を得ること |

最終選択順は **最新の明示指示・制約 → Scope/権限/資源のhard filter → 有効な明示訂正 → 適用可能な学習policy → 固定rules**。学習policyはauthorityを持たない。送信/実行直前にもhard filterを再検査する。

複数actorを順に実行するrecipeは、実行Runtimeがその全stepをサポートする場合だけ適格。現状のfirst actorだけを送る経路に複数actor recipeを選ばせない。不足するrecipe executorはAI-08で既存Runtimeへ接続し、選べるだけの実装を完成としない。

## 4. データ・学習・適用の契約

### AI-C1 判断と結果の単位

新規予定 `role_routing/learning/contracts.rs` に `DecisionObservation` を定義し、既存専門台帳への参照で構成する。全領域を新しい実行正本へ複写しない。

必須項目は `decision_id / domain / scope / event_seq / policy_revision / candidate_fingerprint / eligible_candidates / selected / selection_mode / selection_probability? / source_refs / outcome_revision`。判断前featuresと判断後outcomesを分ける。featuresに最終正誤、未来の状態、後付けユーザー反応を入れない。

technical success、Goal verifier結果、明示user acceptance、訂正、遅延、実費、通知の有用性は別列。沈黙/別話題は成功にも失敗にもしない。LLM reviewerの評価は `model_inferred` であり正解ラベルと混ぜない。未知費用を0円にしない。

同じGoalのstepを独立成功例として水増ししない。root lineage、同じ評価fixtureの派生、同一sourceをgroupでまとめる。field変更/訂正も新eventとして記録し、判断時点のsnapshotを再構成可能にする。

### AI-C2 明示訂正は夜間学習を待たない

「このProjectでは別のToolを使って」「非緊急はまとめて知らせて」などは、対象decision/Scope/期間が一意なら既存訂正経路または領域別policy overrideへ記録する。曖昧な対象は確認し、ユーザー全体へ勝手に一般化しない。

自然文解析中は原文をRCの必須Contextとして維持し、訂正対象の古いpolicy適用を保留する。新policy確定まで古い行動を繰り返さない。訂正の取り消しも履歴付き新revisionとする。明示訂正を訓練に取り込むことと、即時制約として守ることを分ける。

### AI-C3 再現可能な増分処理

開始時に `upper_event_seq` を固定し、その時点のimmutable event snapshotだけでdatasetを作る。page保存、dirty消費、cursor更新は同一transaction。処理中の新feedbackはdirtyに残す。永続cursorがなくなるまで同一境界を続け、batch一頁を全体完了にしない。

DB/ファイル成果物はmanifest、feature版、labeler版、source参照、canonical digestを持つ。生成時刻やランダムIDをdigestの意味的入力にしない。同じ境界・実装版で同じdataset digestを得る。

既存夜間window/idle設定を利用して単一workerを起動する。foreground優先、page間yield、取消、最大実行時間、再起動時のpaused復帰を実装する。新daemon、外部cron、別SQLiteを作らない。datasetをrepositoryへ保存しない。

### AI-C4 初期の学習方式

領域ごとに別artifactを持つ。初期は用途・origin・Scope種別・有効候補を条件とする集計rankerを使い、必要な領域だけL2正則化logistic regressionを追加する。十分なデータのない条件はrulesへ戻す。グローバル平均だけでProject固有の訂正を消さない。

観測された候補の結果だけを学習し、未選択候補を失敗ラベルにしない。過去ログの集計は選択バイアスを含むため因果的な優劣と呼ばない。品質ラベルのない例は品質学習へ入れない。候補/model/recipe版変更はcold startとする。

既存L6の最低条件（品質例200件、対象recipeごと30件、独立group20件）を品質rankerの下限として維持する。validation/testはgroupと時間で分割、各3group以上、必要な両classがなければ昇格不可。データ不足は `insufficient_data` であり成功ではない。

通知と計画にもdomain固有の適格ラベルを定義する。通知は明示評価・期限内配達・割込み回数を併記し、無通知による見かけの改善を排除する。計画は同一Goal verifierと同じ予算条件で評価する。

### AI-C5 昇格ゲートとproduction適用

artifactは `candidate → evaluated → shadow → eligible → active → retired/invalidated`。新規予定activation表にdomain、scope範囲、artifact ID、policy revision、候補fingerprint、設定epochを保存し、同じdomain/Scopeでactive一つをDB制約で保証する。

| ゲート | 条件 |
| --- | --- |
| 構造・権限 | source/型/digest適合、hard constraint違反0、Scope漏洩0、知らない候補の実行0 |
| 比較可能性 | 固定rulesと同じtask pool・同じ候補/モデル版・同じbudget。未選択結果を推測で埋めない |
| 品質 | paired比較でtask success差の95%信頼区間下限が-2ポイント以上 |
| 改善 | 事前に選んだ主指標の改善が確認できる。品質なら差の95%CI下限>0、時間/費用なら改善率の95%CI下限>=5%、他の既知資源軸の悪化<=10% |
| 計画 | verifierを変えず、完了率を落とさず、再試行/手順数/時間の主指標が改善 |
| 通知 | 期限内配達率を落とさず、明示的不満/不要割込みの主指標が改善。必須報告の欠落0 |
| shadow | protocol error0、無効source使用0、固定rulesとの不一致理由を記録。単なる一致率は改善証拠にしない |

比較は読み取り・破棄可能workspace・記録済み外部応答で行う。group単位paired bootstrap、固定seed、10,000再標本化を評価スクリプトで実装し、主指標と比較集合は結果を見る前に固定する。区間が広ければ保留。学習/調整/最終評価データを使い回さない。

Settingsでユーザーが許可したdomainとScopeだけproduction適用する。適応機能の有効化は一度の設定とし、以後はその範囲でゲート合格artifactをCAS昇格する。昇格を新たな外部権限の取得にしない。未許可domainはshadowのまま。

active適用は新しいdecisionから。進行中Taskの計画や実行者を途中で差し替えない。実際の選択・実行receiptにpolicy revisionと採用理由を残す。別の「学習専用会話Runtime」は作らない。

### AI-C6 失効・rollback・探索

forget/Scope revoke/source削除/明示訂正の影響をsource参照から辿り、同一writer transactionでdataset/artifact/activationを失効させる。範囲不明なら当該profileを保守的に失効。古い重みから情報だけ取り除けたとは主張せず再学習する。

dispatch前にactive版を再検査し、無効なら最新の明示訂正を守るrulesへ戻る。rollbackはsource・候補・権限が現在も有効な前版だけ。該当なしならrules。夜間workerの故障が通常会話を止めない。

hard違反は即時停止。通常の品質悪化はdomainごとの事前登録した監視窓で判定し、少数例の上下で頻繁に切替しない。初期窓は直近50 eligible結果、最低20件に達しない間は品質悪化を統計断定せず明示訂正と構造違反を優先する。rules比較を実施できない実トラフィックから因果的劣化を断定しない。

本番でランダム探索はしない。未知候補のデータは別の明示評価タスクで収集する。selectionProbabilityは実際に確率選択した評価だけへ記録し、固定rulesに架空の確率を入れない。

## 5. Terra向け作業カード

カード順に実装。5実装ファイルを超える場合は枝番へ分割し、同じ契約を維持する。AI-08〜11は共通選択API確定後、領域ごとに完了させる。

| ID | 対象と実装 | 合格条件 |
| --- | --- | --- |
| AI-00 | 現状、HEAD/dirty、source/event、四領域baseline、評価poolを固定 | 利用中のpolicyと測る改善を事前登録。既存shadowをactiveと呼ばない |
| AI-01 | DecisionObservation、領域別label、event snapshotとmigration | 未来情報なし、沈黙はunknown、HTTP成功とGoal成功を分離 |
| AI-02 | 即時訂正・対象参照・scope/期間・取消を共通policy選択へ | 夜間処理なしで次の対象判断が変わる。別Projectへ漏れない |
| AI-03 | 上限event sequenceによる増分materializerとcursor | page途中crash/new feedback/同じsourceの訂正を取りこぼさない |
| AI-04 | app内worker起動、foreground取消、再開、実行予算 | scheduler関数が存在するだけでなくsetupから実際に動く |
| AI-05 | domain別集計、必要なlinear trainer、immutable artifact | 再実行digest一致、不足データで昇格0、未選択を負例にしない |
| AI-06 | paired評価runner、shadow実入力、主指標/CI算出 | fixturesの期待値、group split、漏洩検出。自己採点を正解にしない |
| AI-07 | activation/CAS、設定範囲、rollback、dispatch再検査 | 合格artifactだけactive。source失効と同時の使用0 |
| AI-08 | Provider/recipeの実選択へ適用、必要な既存executor接続 | モデルに投げた実actor/recipeが学習選択と一致。first actorだけの偽実行0 |
| AI-09 | Tool Selectionのhard filter/明示訂正後に適用 | 実invoke参照と選択revisionが一致。grant拡張0 |
| AI-10 | DWの登録済みplan recipe選択とverifier結果へ適用 | 同じGoalで次回の手順が改善。進行中planは不変 |
| AI-11 | DW通知policyへ適用、hold/deadline制約との合成 | 次回の通知方法/集約が変わり、必須配達とmuteを守る |
| AI-12 | forget/訂正/候補版変更から失効と再学習 | ロールバックで削除情報復活0、policyが失効した直後も通常会話可能 |
| AI-13 | Settingsの四領域ON/OFF・理由・改善指標・戻す導線 | 専門用語なしで何が変わるか分かる。OFFでも明示訂正は残る |
| AI-14 | 四領域の前後比較、実UI/実Provider統合、性能 | §6の全条件。全領域shadowだけの最終報告をしない |

## 6. 完全克服の受入条件

| シナリオ | 合格 |
| --- | --- |
| AでTool訂正→B→Aへ戻る | Aの次回選択だけ変わる。実invokeまで追跡可能 |
| 同じGoalの失敗/成功とverifier結果を蓄積→学習→次回 | 改善recipeを選び、同じ成功条件で測定された改善を示す |
| 非緊急通知の明示訂正→会議→解除 | 次回の通知がまとまり、必要な期限通知は落ちない |
| 同じ要求でProvider/recipe改善artifactを有効化 | 実際のdispatch先が変わり、比較ゲートを満たす。行動台帳だけの変更ではない |
| source忘却中に学習/dispatch/rollback | 無効artifact使用0。新revisionは削除済みsourceを含まない |
| dataset不足、候補更新、夜間失敗、設定OFF | 理由付きrules fallback。通常会話と委任の継続を妨げない |
| 悪意あるfeedback引用、曖昧評価、条件変更 | 新権限/誤label/Scope一般化0。必要な場合だけ対象確認 |

四領域それぞれで「記録→集計→評価→昇格→実際の次回選択→結果→失効/rollback」を通す。controlled評価には人工データを使用できるが、実ユーザーの長期改善と偽らない。最終比較は未使用のtask poolと実adapterで行い、実Providerを使う領域はその実結果を別記する。

データ不足でもpipelineの実装完了は記録できる。ただし四領域の改善ゲートを通るまで「弱点完全克服」としない。未達は必要なデータ/adapter/実評価を具体化し、閾値を下げて完了させない。

性能目標: 既存L8のpage100件write p95 50ms以内、foregroundから背景推論cancel要求100ms以内、背景処理中TTFA p95悪化10%以内。online ranker追加p95 5ms以内。既存の数値ゲートを緩めず、未達はbatch縮小・背景停止で会話を優先する。

検査: 新規Rust `ai_`（0件不可）、既存role_routing/tool_selection/steward/schedule/forget試験、評価スクリプトの統計・split・digest試験、`bun run ipc:check`、`bun run size:check`、`bun run check:local`、`bun run spec:check`。必要時にIPC生成と独立crate/service検査を追加する。

## 7. 成果物と開始指示

`spec/evidence/adaptive-improvement/{baseline,progress,evaluation-protocol,results}.md` に、各領域のbaseline、対象数、除外理由、paired差/CI、active revision、実dispatch/計画/通知の差、rollback結果を残す。個人datasetや会話本文はrepositoryへ入れない。

Terraへの開始指示例:

> AI-00から順に実装してください。既存feedbackと学習台帳を使い、四領域についてshadow保存から実際の次回判断への適用まで完成させてください。明示訂正は即時、経験は比較評価後に反映し、Scope・権限・忘却・rollbackを守ってください。改善を測れない領域を完成扱いせず、残る条件を報告してください。
