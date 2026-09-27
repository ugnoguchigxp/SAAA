# 再レビュー改善5点 統合実装計画（Terra向け）

作成日: 2026-09-21。状態: 実装接続済み。横断の実環境受入と学習効果測定は [証跡](../evidence/review-five-improvements/progress.md) / [結果](../evidence/review-five-improvements/results.md)。対象: 再レビューで指摘した5点。実装担当想定: Terra。

## 1. 目的と適用範囲

「理解した制約の下で仕事を続け、根拠ある成果を届け、経験を次の判断へ反映する」という [Personal AI Concept](saaa-personal-ai-concept.md) に近づける。今回の5点について、本書を追加実装・受入の正本とする。既存の [委任仕事計画](saaa-delegated-work-completion-plan.md)、[委任残件計画](saaa-delegated-work-repair-terra-plan.md)、[World供給計画](saaa-world-delivery-completion-plan.md)、[適応改善計画](saaa-adaptive-improvement-completion-plan.md) の権限・Scope・忘却・予算契約は継承する。過去のカードの完了記録は、新しい受入条件の達成を意味しない。

| 改善 | 現状の具体的な不足 | この計画の完成状態 |
| --- | --- | --- |
| V: 成果検証 | `steward/verifier.rs` は `test_report_obtained` をjob IDの存在でPassにし、readも同様の迂回がある | 実行と対象に結び付いた成果を検証し、証拠不足でGoalや後続stepを進めない |
| N: 否定指示 | `steward/intake.rs` の否定判定が引用記号等を条件とし、モデル指定の抜粋しか見ない | 全文と対象操作を照合し、禁止・撤回・曖昧な依頼を既存grantで自動実行しない |
| A: 学習の昇格 | workerと通常IPCは候補学習まで。評価・承認・activateの関数はあるが通常利用の接続がない | 実測評価を取り込み、根拠を確認して有効化・撤回し、4領域の通常選択に反映する |
| G: 自然なWorld参照 | `runtime/context/world/question.rs` は引用符付き4定型文のみ受理 | 言い換えや会話中の明確な指示対象から、Scope内のグラフを根拠付きで参照する |
| W: 通知の起床 | 完了・保留解除が45秒の定期drainまで待つ場合がある | commit済み完了・通知可否の変化で起床し、次の発話や定期tickを待たず配送判断する |

コード確認時点の指摘であり、着手時に再確認する。テスト数やリリース準備を改善の目的にしない。ただし上記の動作を実証する検証は各カードの完了条件とする。任意の仕事の完全自動化、万能な自然言語判定、実測前の学習効果保証は対象外。

## 2. Terraへの実行規則

1. 最初にgit差分と関連コードを読み、他の作業を維持する。以下のpathは特記なしに `src-tauri/src/` 相対。新設と明記したpath・型・コマンドは提案であり、現存APIとして呼び出さない。
2. 一度に1カードを実装する。1カードは原則1つの動作変更、実装ファイル最大5個程度と関連テスト。超える場合は同じIDにa/bを付け、契約・producer・consumer・UIを分割してから進める。全体の一括書換えは禁止。
3. 先に期待する動作と失敗条件を書き、その境界を実装する。関数があるだけ、fixtureでだけ動く、通常経路で未使用、という状態で完了にしない。
4. 永続状態は既存SQLite/実行台帳を正とする。加算migrationは着手時の最新番号+1。既存migrationを書き換えない。外部I/Oやモデル呼出しはDB transaction外。
5. 関連外のリファクタリング、権限拡大、gate閾値の緩和、失敗を成功へ読み替えるfallbackは禁止。通常の実装判断は自律的に進める。仕様矛盾・必要権限の不足・破壊的移行が必要な場合のみ具体案と影響を示す。
6. 各カード完了時に `spec/evidence/review-five-improvements/progress.md` と `results.md`（新設）へID、差分、実行コマンド、成功件数、残件を記録する。未実施を成功に数えない。

推奨順序: **F-00 → V-01〜06 → N-01〜06 → W-01〜05 → G-01〜06 → A-01〜08 → F-01**。計33カード。Aの評価契約設計は先行可能だが、成功ラベルの採用はV完了に依存する。実装を別担当へ振る場合も同じファイルの同時編集を避ける。

### 共通の検証・失敗時対応

Rustカードはテスト名を `rf5_v_` / `rf5_n_` / `rf5_w_` / `rf5_g_` / `rf5_a_` で開始し、例えば `cargo test --manifest-path src-tauri/Cargo.toml rf5_v_` で対象を実行する。これは新規テストの命名規約であり、現時点で存在するコマンド結果ではない。0件成功を受入にしない。カード記録に実際の件数を残す。

UI変更は対応する `tests/` 内のテストを `bun test <実際のファイル>` で実行し、`bun run typecheck` を通す。IPC変更では `bun run ipc:generate` 後に差分確認し `bun run ipc:check`。各テーマの最後に `bun run size:check`、関連既存回帰を実行する。全体チェックの既存失敗は今回の回帰と区別し、変更前後の根拠を残す。

失敗したカードは未完了に戻し、原因を修正して同じ条件で再実行する。外部環境不足ならローカル境界検証を続け、実接続受入は未実施と明記する。fixtureで実装接続を証明しても、実データでの性能・学習改善を証明したとしない。

## 3. 共通準備

### F-00 現状と所有境界の固定

- 対象: 上表の5入口、既存関連計画、証跡ディレクトリ（新設）。依存: なし。
- 作業: HEADとdirty差分、現在の全terminal producer、状況更新producer、4領域のchoose呼出し、World入力経路を一覧化する。各テーマの回帰例を最小fixtureで保存する。既実装部分は再利用し、本書の不足が既に解決済みならコードと通常経路の証跡で判定する。
- 受入: 各producer→台帳→consumerの対応表と、5点の再現または解消済みの証拠がある。対象外ファイルを書き換えない。

## 4. V: 成果の実証と正しい完了

### 契約

`settled` はrunnerの終了であり、Goalの成功ではない。hostが生成・検証した `ExecutionEvidence`（新設概念）を使い、schema version、task/job/run、recipe IDと版、対象digest、終了状態、成果参照とdigest、生成元を束縛する。モデルがJSONに `exitCode: 0` と書くだけでは証拠にならない。最新rowを無条件に採るのでなく、terminal eventのrunとTaskの採用runを一致させる。

`test_report_obtained` は許可recipeの実行結果・終了コード・結果要約が読み出せること。失敗テストの有効なレポートもPassにできる。`tests_pass` は同じ検証に加えてrunnerの成功条件を満たすこと。readは許可対象の取得記録と読み出せる成果を必要とする。欠損・不正・別runはMissing/Unknown、実行したテストの不合格はFail。既存状態写像を維持し、理由を保存する。

### V-01 証拠型と検証器

- 対象: `steward/verifier.rs`、`execution_contracts.rs`、新設 `steward/evidence.rs`。依存: F-00。
- 作業: 上記の型、version検査、ID整合、成果参照・digest検査、理由コードを実装。ファイル参照は許可root内に限定し、path文字列だけで許可しない。
- 受入: 空JSON、job IDのみ、空digest、モデル自由文、別run、対象違い、読めない成果はPassにならない。有効レポートとread取得記録は各条件に応じて判定できる。

### V-02 host側の証拠生成

- 対象: `runtime/pi/runner.rs`、`coding/repository.rs`、登録test/read recipeの実行境界。依存: V-01。
- 作業: 実際のtool/runner結果から証拠を保存する。成果を保存してから参照をcommit。対応adapterごとに同じ契約へ変換し、証明できないadapterは不足を返す。adapter追加はV-02a/bに分割する。
- 受入: 実行出口から生成した証拠で成功・失敗が区別される。モデルが成功を宣言してもhost結果失敗なら覆らない。成果保存失敗時は成功証拠を残さない。

### V-03 terminal eventとverifierの接続

- 対象: `steward/driver.rs`、`repository.rs`、`verifier.rs`。依存: V-02。
- 作業: terminal eventのrun IDを検証へ渡す。`job.is_some()` のPass分岐を削除。outcomeをevent/run/verifier版で冪等保存し、重複通知で判定を二重採用しない。
- 受入: settled+証拠欠損ではdoneも依存step開始も0件。遅い旧run結果、再送、別Task証拠も採用されない。

### V-04 Goal進行と学習ラベル

- 対象: `steward/plans.rs`、`repository.rs`、`adaptive_improvement.rs` の記録入口。依存: V-03。
- 作業: 検証Passのみ依存解除・Goal完了へ進める。technical completionとverified successを別に保存し、Missing/Unknownを成功・失敗の学習ラベルへ勝手に変換しない。
- 受入: 2step計画の1step証拠不足で2stepが始まらない。失敗レポート取得Goalは完了できるがtests_pass Goalは完了しない。学習datasetに根拠のない成功が入らない。

### V-05 既存結果と表示の整合

- 対象: 新規migration、`steward/report.rs`、`src/features/coding/StewardPanel.tsx`。依存: V-04。
- 作業: 旧証拠をlegacy_unverifiedとして区別し、以後の学習から除外。過去完了を無断で再実行・再通知しない。進行中の依存解除には新検証を要求する。UIと報告で不足理由、取得済み成果、確認待ちを表示。
- 受入: 旧DBを開いても過去の仕事が再実行されない。不足証拠の仕事が「完了」と表示されない。ユーザー確認は別receiptとして記録し、実テスト成功へ偽装しない。

### V-06 通常経路の受入

- 対象: 関連統合テスト・証跡。依存: V-05。
- 作業: 通常work_propose→実profile→証拠→Goal→報告を確認。
- 受入: 成功、テスト不合格、証拠欠損、旧run再送、撤回後結果の5ケースを通す。少なくとも1つの実対応profileで成果取得を確認し、対応を表示する他profileにも同じ契約を適用する。

## 5. N: 全文に基づく禁止・撤回の保護

### 契約

モデルは依頼候補を提案できるが、切り取った抜粋を実行許可にできない。永続化されたユーザー原文の全文・内容digest・source版をhostで取得し、`Allowed / Denied / Ambiguous / NotRequest` を対象operation/resourceごとに決める。既存grantは権限上限であり、その発話で実行を求めた証拠ではない。明確な拒否は拒否、解釈が分かれる場合は確認待ち。禁止語の有無だけであらゆる文章を拒否する実装にはしない。

### N-01 source全文のbinding

- 対象: `steward/authority.rs`、`intake.rs`。依存: F-00。
- 作業: 抜粋と全文を分けて取得。文字数だけの版識別があれば内容digestへ移行。quote範囲の妥当性・role・conversation・訂正/忘却を照合。
- 受入: 同じ文字数の原文変更で古いbindingが失効。肯定部分だけの抜粋でも全文の否定条件を検査できる。tool本文をsourceにできない。

### N-02 操作単位の意図判定

- 対象: 新設 `steward/request_intent.rs`、`intake.rs`。依存: N-01。
- 作業: 直接禁止、撤回、引用、仮定、複数節を分類。明確な肯定依頼のみAllowed。必要なら既存の構造化解釈を補助利用するが、モデル判定だけでdenyを解除しない。不明な文はAmbiguousへ。
- 受入: 「テストを実行しないで」「do not run tests」はDenied。「変更しないで、テスト結果を読んで」はwrite Denied/read Allowed。「『実行して』という文を説明して」はNotRequest。「実行しないでとは言っていない」は自動実行せず確認。「テストして」は他条件成立時Allowed。

### N-03 全受付経路への適用

- 対象: `intake.rs`、`admission.rs`、既存互換受付。依存: N-02。
- 作業: covering_grant確認より前に共通判定。Deniedはproposal/taskを実行可能状態にしない。Ambiguousは対象操作を示す確認へ。古い確認画面で否定を上書きしない。
- 受入: 同じ5ケースをgrant有無、引用範囲有無で確認。誤ったwork_propose引数でも禁止操作のdispatchが0件。明確な既存権限内依頼は不要な再確認なしで進む。

### N-04 撤回とdispatch直前の競合

- 対象: `steward/dispatch.rs`、`authority.rs`、撤回を保存する既存service。依存: N-03。
- 作業: dispatch CAS時に最新source版・禁止/撤回状態・grant版を再検査。撤回は既存のcancel経路へ接続。既実行の効果を取消済みと誤表示しない。
- 受入: 受付後・dispatch前の撤回で新規実行0。dispatch後ならcancel要求と残存状態を表示。訂正・忘却と競合しても古いsourceで開始しない。

### N-05 確認UIと音声source

- 対象: `StewardPanel.tsx`、source transcriptの既存確定経路、確認IPC。依存: N-04。
- 作業: 禁止された操作、確認対象、確定sourceを表示。音声暫定文で開始せず、確定transcript版を使う。ユーザーが後で明示的に変更した依頼は新しいsourceとして扱う。
- 受入: 「実行して…いや、しないで」の確定文で実行しない。古い確認ボタンは拒否。新しい明示依頼は以前の禁止語だけを理由に永久拒否されない。

### N-06 通常入口の回帰

- 対象: intake/dispatch統合テスト・証跡。依存: N-05。
- 作業: モデルが誤って提案したtool callを通常のhost入口に渡し検証する。
- 受入: 日本語・英語、抜粋による否定切落とし、read/write混在、仮定、撤回競合を通す。単独の文字列helperテストだけで完了しない。

## 6. W: 完了・保留解除からのイベント起床

### 契約

Wakeは再評価の合図であり、権限・配送可否を決めない。台帳commit成功後にsignalし、drainが最新の通知設定・状況・outboxを読む。既存45秒tickは取りこぼし復旧として残す。schedulerやdrain loopを二重に作らない。

### W-01 起床境界と取りこぼし防止

- 対象: `steward/pump.rs`、`schedule/tick.rs`。依存: F-00。
- 作業: 既存Wakeのpending/Notifyを整理し、通知と待機開始が競合してもpendingを回収する。単一drain、burst合流、drain中の再signalを扱う。
- 受入: 待機直前・処理中・連続signalで永続pendingが放置されない。signal自体は仕事を作らない。fake clockでtick前にdrainされる。

### W-02 terminal commit後のsignal

- 対象: F-00で列挙したterminal producer、`runtime/pi/runner.rs`、coding recovery。依存: W-01、V-03。
- 作業: settled/failed/interrupted/outcome_unknownのcommit成功後だけ既存Wakeへ接続。producerが多ければadapter別カードへ分割する。
- 受入: 正常終了・異常終了・復旧結果で次turnなしに処理される。rollbackでは成功報告なし。重複eventでも報告messageは1件。

### W-03 状況と通知設定の変化

- 対象: `situation/tick.rs`、`situation/speech.rs`、通知設定更新service。依存: W-01。
- 作業: meeting解除、mic/audio終了、通知ON/OFFなど配送可否に関係する確定状態の変化でsignal。毎sampleを起床にせず、遷移で合流。mutexを保持したままdrainしない。
- 受入: 保留中の報告がhold解除後に再評価される。まだ別のhold理由があれば配送しない。通知OFFへの切替も最新状態で尊重する。

### W-04 再起動と期限による保留解除

- 対象: `steward/outbox.rs`、`pump.rs`、`schedule/tick.rs`。依存: W-02、W-03。
- 作業: 起動時に永続outbox/cursorから回復。期限だけで解除されるholdは最も近いdue timeのtimerを既存loopへ追加。アプリsleep中の配達を約束せず、復帰時に再評価する。
- 受入: commit後signal前のクラッシュを復旧できる。スケジュール機能OFFでも委任報告の回復が動く。音声delivery_unknownを無条件に再生しない。

### W-05 遅延と重複の統合受入

- 対象: worker統合テスト・証跡。依存: W-04。
- 作業: event commit→drain開始→report commitの時刻を採取。新しい会話turnを送らずに確認する。
- 受入: アプリ稼働・idle・holdなしのローカル受入でdrain開始1秒以内、text report commit2秒以内を目標として測定し、超過なら原因を記録して修正。これはモデル実行時間・音声再生時間を含まない。100件burst、worker再起動、連続hold遷移で欠落0・text重複0。

## 7. G: 自然な質問からScope内のグラフへ

### 契約

定型parserは互換として保持し、自然文のquery理解を追加する。許可するintentはRelevance/Influence/Correlation/Dependencyの4種。構造化結果はintent、topic/source span、参照候補、confidenceであり、SQL・任意Scope・命令を返させない。hostでScope・entity存在・候補数・予算を検証する。「これ」は同じScopeの直近の一意な対象だけに解決し、複数候補なら尋ねる。

### G-01 検索意図と曖昧性の型

- 対象: `runtime/context/world/question.rs`、新設query理解型。依存: F-00。
- 作業: Requested/Ambiguous/NotRequested/Unavailableを区別。source版、intent、候補と理由を持たせる。既存4定型文は同じ型へ変換する。
- 受入: 「Xは今の目標に関係ある？」「Xが変わると何が影響を受ける？」「XとYの関連を知りたい」「Xの前提は？」を表現できる。一般挨拶は検索要求にしない。

### G-02 自然文解釈の通常経路

- 対象: 新設query理解service、既存構造化モデル呼出し境界。依存: G-01。
- 作業: 定型高速経路の後に、保存済みユーザー入力から構造化intentを取得。初期上限は追加モデル呼出し1回/turn、応答2KiB、deadline 1500ms、候補5件。失敗時は通常runtime-state回答へ戻し、検索済みと主張しない。
- 受入: 引用符・疑問符なし、口語、日英の言い換えを解釈。malformed JSON、timeout、未知intent、scope指定注入をhostが拒否。字句の別名を数個追加しただけで完了しない。

### G-03 entityと指示語の解決

- 対象: World query service、Scope内entity取得、会話source参照。依存: G-02。
- 作業: entity名・既存alias・直近の明示focusを照合。別Scopeの履歴やモデルの架空IDを採用しない。source訂正/忘却でcacheを無効化。
- 受入: 一意な「これ」は解決。同名2候補は確認。存在しないtopicは未知として回答。Scope切替後に旧topicを持ち越さない。

### G-04 最新frameへの接続

- 対象: `runtime/context/world/app_frame.rs`、frame生成の共通呼出し、provider接続。依存: G-03。
- 作業: 非同期解釈をtransaction外で行い、完了後に最新source版とScopeを再確認してframeを作る。複数adapterの再生成で同じ解釈を無駄に再実行しない。cache keyにsource内容・Scope版・理解器版を含める。
- 受入: 遅い解釈中のScope変更・forgetで古いgraphを送信しない。各対応providerと確定音声入力が共通結果を受け取る。

### G-05 回答根拠と確認導線

- 対象: `runtime/context/world/host_answer.rs`、既存claim検証、回答UI。依存: G-04。
- 作業: 解決したentity、relation、sourceを回答に結び付ける。相関を原因と断定しない。曖昧時は候補名を短く示して選択させ、選択後もScope再検査。
- 受入: graphにない関係は断定しない。推論と記録済み事実を区別。解釈失敗で通常の現在状態回答が壊れない。

### G-06 会話経路の受入

- 対象: query/frame統合テスト・証跡。依存: G-05。
- 作業: 4intent×各5言い換えと、曖昧・指示語・Scope・忘却・timeoutの境界ケースを固定する。
- 受入: 一意で根拠がある入力は正しい参照、曖昧入力は確認、越境0。通常の実モデル解釈を少なくとも1経路で確認し、adapterへの供給は対応全経路で検証する。固定fixtureだけの解釈精度を実精度としない。

## 8. A: 実測から通常の選択へ至る学習昇格

### 契約

既存 `ai_artifacts` / `ai_activations` と `evaluate_paired` / `apply_evaluation_gate` / `approve_shadow` / `activate` / rollbackを再利用する。UIから集計値だけを送って昇格させない。評価記録はdataset digest、artifact digest、domain/scope、candidate fingerprint、baseline、評価器版、group分割、測定根拠を持ち、hostが集計する。学習と評価のgroupを分離し、未観測の反実仮想を成功として捏造しない。

既存gate（200例、recipe例30、独立group20、成功差の信頼区間下限>0、protocol/source/scope違反0等）は維持。resource改善の5%と他resource悪化10%は単位を固定した比率で計算し、現在の生の差を閾値へ渡さない。基準値0・欠測を扱う規約を評価前に保存する。実測が足りなければ不足を表示して昇格しない。

### A-01 評価記録とライフサイクル

- 対象: `adaptive_improvement.rs`、新設評価module、新規migration。依存: F-00。
- 作業: immutable evaluation record、candidate→evaluated→shadow→eligible→active→retiredの遷移条件、版CAS、承認receiptを定義。巨大な既存moduleへ全処理を追加しない。
- 受入: artifact差替え・評価差替え・重複import・古い承認を検出。既存DB/候補は保持し、証跡がないものを自動eligibleにしない。

### A-02 実測評価の作成・取込み

- 対象: 新設評価serviceとローカル評価runner。依存: A-01、V-04。
- 作業: 通常画面から評価対象と予算を登録し、保存済みの再実行可能なケースでbaseline/candidateを比較する。副作用のある処理は隔離環境と既存許可範囲に限定。不可能なケースは除外理由を残す。評価bundleは生の対応観測・根拠digestを含め、hostが由来と完全性を検証する。
- 受入: 両側の実測がないpairは採用しない。train/eval同group、範囲外source、旧成功ラベル、偽造集計値を拒否。通常利用からbundleの生成または検証付き取込みができる。

### A-03 集計・gate・shadow

- 対象: `evaluate_paired`、`apply_evaluation_gate`、shadow記録service。依存: A-02。
- 作業: group bootstrapを再利用してhost集計。resource値を明示した比率に正規化し、seed/単位/母数を保存。shadowでは通常選択を変えず候補順位と有効性を記録する。shadow終了条件は評価仕様として固定し、最低30有効判断・protocol/scope違反0を初期条件にする。
- 受入: 同じbundleとseedで同じ結果。閾値境界・欠測・NaN・基準0を検査。不足は理由付き停止。shadow観測を実行していない候補の成功実績に読み替えない。

### A-04 通常IPCの管理入口

- 対象: 新設adaptive管理IPC、`role_routing/ipc.rs`、Tauri command登録、生成bindings。依存: A-03。
- 作業: list/detail、評価開始/取込み、承認、有効化、rollbackを通常commandとして公開。scope/domain・現行revision・評価digestをhostで検査。承認とactivateは別状態だがUIで連続操作可能にする。
- 受入: 開発fixture環境変数なしで利用可能。任意scoresやstateを受け付けない。二重クリック・並行承認で有効版が競合しない。IPC契約チェック成功。

### A-05 レビューと操作UI

- 対象: 新設 `src/features/settings/AdaptiveImprovementSection.tsx`、API client、Settings接続。依存: A-04。
- 作業: 4領域の候補、現行版、評価母数、差と区間、scope、未達理由、shadow状態を表示。承認・有効化・rollbackを用意し、無効なボタンの理由を示す。
- 受入: 通常設定画面だけで候補→評価→shadow確認→承認→有効化→rollbackを操作できる。データ不足でも理由を把握でき、直接DB編集を要しない。

### A-06 4領域の選択への接続確認

- 対象: provider/tool/plan/notificationの既存choose呼出し。依存: A-04、V-04。
- 作業: 各入口がdomain/scope/fingerprintとactive版を使うことを確認。ユーザー明示override→適格な学習版→既存rulesの優先順と、事前権限filterを維持。decisionに採用artifact版を保存する。
- 受入: 4領域それぞれで有効化前後の順位差とdecision記録を確認。許可候補を増やさない。overrideは学習に勝ち、無効artifactはrulesへ戻る。

### A-07 忘却・失効・復帰

- 対象: 既存forget連携、activation/rollback、管理UI。依存: A-06。
- 作業: source忘却、候補集合変更、評価根拠失効で関連active版を選択不能にする。rollbackは次の判断から反映し、実行中の仕事を無断で切り替えない。
- 受入: 失効版をcacheからも採用しない。再起動でactive版と失効が一致。explicit overrideを消さずrulesへ復帰できる。

### A-08 通常画面からの統合受入

- 対象: IPC/UI/選択統合テスト・証跡。依存: A-05〜07。
- 作業: fixtureフラグなしで管理の全経路と4領域選択を通す。隔離した決定的データで接続を検証し、実測bundleで評価生成・不足または合格の判定も確認。
- 受入: gate合格ケースだけがactiveになり、実際の選択へ反映。未達ケースは昇格不可。実データが足りないときは「接続実装完了・改善効果未実証」と記録し、学習が有効になったと報告しない。

## 9. F-01 横断受入と完了判定

依存: V/N/W/G/Aの全カード。対象: 通常UI・実行service・証跡。

1. read/testの委任を自然文で依頼し、許可範囲で実行する。別の話題へ移り、以後の発話なしで証拠に基づく結果が届く。
2. 同じgrantがあっても「実行しないで」で新規実行しない。肯定部分だけの抜粋をモデルが送ったケースも同じ。
3. レポートのないsettled runではGoal未完了・後続step未開始・学習成功ラベルなし。
4. meeting中は報告を保留し、解除後にevent起床する。再起動・同一event再送でtext報告は重複しない。
5. そのGoalとの関係を口語で質問し、現在Scope内のWorldを参照する。曖昧な指示語は確認し、忘却後のsourceを利用しない。
6. 通常設定画面から実測評価を確認し、適格な候補だけ有効化できる。4領域の判断に版が記録され、rollback後は新規判断がrulesに戻る。

成果物は実装差分、migration、必要な境界テスト、通常経路の証跡、残制約一覧。カード表には `未着手 / 実装中 / 実装済み・受入待ち / 完了 / 外部条件待ち` を使用し、外部条件待ちを完了に含めない。F-01完了後、旧計画の該当箇所に本書と証跡をリンクし、実装と効果測定の状態を別々に更新する。

## 10. Terraへ渡す開始指示

> この計画のF-00から開始してください。まず現在の差分と実装を確認し、既存成果を維持してください。カードは依存順に1つずつ実装し、5実装ファイルを大きく超えるカードはproducer/consumer等で分割してください。各カードの受入条件を通常経路まで満たしてから進捗を更新してください。モデルの成功文、job ID、fixture専用入口、未観測の評価値を完了根拠にしないでください。実装を接続しただけの状態と、実際の成果・改善効果を確認した状態を区別し、失敗・未実施と再実行結果を証跡に残してください。
