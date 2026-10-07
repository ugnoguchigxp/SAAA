# メモリー計画の実装記録

2026年10月7日。対象は[調査・実装計画](./report.md)の現在状態、公開Snapshot、失効、ContextStill Episode連携である。SAAAは原記録と現在状態を保持し、ContextStillが生成したEpisodeを利用する。SAAA側にEpisodeの生成・保存正本を追加していない。

同日のユーザー指定により、ContextStillによるSQLiteの自動取得とvibe memory化は既存の前提とし、計画の作業・完了条件から外した。前回追加した取得Adapterの実装と検証履歴は、計画対象外の補足記録として保持する。

[Rustのドメインcrate分割計画](../../../../docs/plans/rust-domain-crate-migration.md#15-メモリー計画との同時進行)と並行する。この記録の現行module名は移行元を示し、今後の配置を固定するものではない。移行時は当日の作業ツリー・追加SQL・未追跡テストと登録を含めて引き継ぎ、Snapshot・参照依存・応答保存前の失効判定を維持する。crate移行は未実施であり、以下の検証結果を抽出後の接続成功には流用しない。

## 実装した経路

| 計画 | 実装 |
|---|---|
| 段階0〜1 | 通常会話のPersonal State読取を接続。Preference・Habit・PersonalFact・Observationを既存台帳へ追加。引用、発話者、確定度、時間、ScopeをHostで検証し、LLMが指定したActiveをそのまま採用しない。対象に関係する現在状態と直前の会話だけを予算内で抽出入力へ渡す |
| 段階2 | 訂正、原文編集、忘却、Scope変更、許可取消しを原典版と依存へ伝播。モデル応答の受理時と同じDB transactionで保存する直前に、SnapshotとEpisode参照の適格性を検証する。忘却時は記憶を使った保存済み派生回答も除去する |
| 段階3 | 確定AssertionからProfile・Project・Topicの本文を決定的に公開。本文bytesと根拠manifestを分離。同じ本文でも根拠の変更で進行中応答を失効させる。関連Pendingがある旧版を保留し、未処理入力が予算を超えた場合は記憶が不完全である旨をモデル入力へ明示する |
| 段階4の試験実装 | 既存durable jobにConsolidation段階と再開checkpointを追加。モデル待機はWriter transactionの外。複製原文の支持を重複計上せず、統合結果は推論Candidateに保持。既定でオフ、実測による通常採用は未完了 |
| 段階5 / E4 | 個人Scope付きEpisode検索、時間filter、IDとsourceKeyによる詳細取得、選択済みEpisodeの原典版だけを読むfetch_episode_sourceを会話toolへ接続。直近の回答で使った参照を同じfocus Scopeの次ターンへ引き継ぐ。一回の原文取得はUTF-8で8192 bytes、検索・詳細・原文を共通の3回予算で制限 |
| E0・E2・E3 | Scope別の利用判定、既存Distillerによるpersonal_episode生成、原典の引用照合、蒸留前・保存前・検索時の失効検証と派生コピーの回収を実装 |

現在の値を変える型付き訂正では、旧主張の原典版をEpisode派生用として退役する。履歴会話自体は保持する。同値の再確認では原典を退役させない。曖昧な競合は既存値を勝手に置換せず、候補とDisputeで保留する。自然言語の訂正対象や時間解釈を完全に理解できる保証はない。

保存済み派生回答も、原典編集、Episode取得許可の変更、原典版の退役、Scopeの失効、所属変更で除去する。忘却以外の変更後も、会話履歴や後のEpisode取り込みから旧回答が再利用される経路を止める。

参照の引き継ぎでは、同じfocus Scopeの直近16メッセージにある回答から、Episode ID・sourceKey・原典版を最大5件選ぶ。Episode本文を別保存せず、既存の回答と参照台帳を使う。モデルには「同じ話題の継続に必要な場合だけ使う参照」として渡し、新しい事実にはしない。各ターンで全原典の版と許可を検証し、保存時に新しい回答の依存へ引き継ぐ。focus変更・失効・直近履歴の範囲外では利用しない。

通常会話が保存するISO形式の日時と、テスト等の数値形式を同じepoch millisecondsへ変換する。旧版でISO文字列を整数へ切り詰めていた記録は、内容を保った新しいSource版へ移し、旧版を利用停止・再抽出する。ローカル抽出モデルにも発話者・原典の時刻・全文を読めたかを渡す。移行は二回実行してもSourceとjobを重複させない。

## ContextStillの責務

personal_episodeは出来事の開始・終了、時刻精度、Topic・Entity、正確な根拠引用を保持する。成果や教訓のない出来事に架空の結果を補わない。個人記憶はrepo/globalの検索、Finding/Knowledgeの生成へ混入させない。

失効した同期原記録、Episode、FTS、参照、job、job event、断片台帳を除去し、残る許可済み原典は再同期する。新規の管理バックアップには私的原記録と派生Episodeを含めず、復元後は現行許可から再同期する。外部で作成済みのコピーや過去のバックアップは変更しない。

[連携契約の仕様](/Users/y.noguchi/Code/contextStill/spec/docs/saaa-personal-episode-contract.html)

## 運用設定

1. メモリー利用と失効連携の変更を反映した版を使用する。現在稼働中の古いプロセスへ、この作業が自動で差分を反映するわけではない。
2. 原記録と現在状態の経路には、既存のメモリー有効設定と抽出モデルの接続設定が必要である。
3. SAAAのContextStill検索は既存のendpoint manifestを使う。別の保存場所なら`SAAA_CONTEXT_STILL_RUN_DIR`を指定する。停止時は通常会話を継続し、検証できない旧Episodeを再利用しない。
4. 「会話から傾向を整理する」は試験運用。品質と費用の比較が済むまでオフを維持する。

保存済み設定やproviderは初期化・置換していない。今回の接続・移行テストは隔離DBと通常migrationで作成したfixtureだけを使用した。

## 補足メモ：ContextStillの自動取得（計画対象外）

ContextStillがSAAAのSQLiteを自動で読み、vibe memory化する既存処理を前提とする。取得経路の新設・設定・受入は、今回のメモリー利用計画の完了条件に含めない。

以下は前回追加した取得Adapterの実装履歴である。通常のagent-log-syncからSAAA Adapterを呼び、vibe memoryとEpisodeDistillerの既存Writer・キューを再利用する。読取専用SQLite view、変更feed、cursorと断片の冪等保存を追加した。1回のfeedと保存済み原典の巡回は128件、1原典の断片は32000 bytes、会話まとまりは8断片・64000 bytes以内。原典の版、digest、byte範囲、許可revision、権限revision、所属revisionを照合する。

この追加Adapterは、ContextStillの起動環境の`SAAA_MEMORY_SQLITE_PATH`と、SAAAの「出来事の記憶」の本人・Project別の取得許可を使う。取得許可の初期値は不許可。これは追加コードの設定記録であり、ユーザーが示した既存の自動処理を使うための新しい必須手順ではない。今回の範囲修正では、コードや設定を変更していない。

## 検証記録

最新の実行結果は[verification-implementation.json](./verification-implementation.json)に記録する。通常verifyはunit testやE2Eを含まないため、それぞれを区別する。全体の検証が別領域の診断で停止した場合、停止後の段階を合格として扱わない。

取得Adapterのテスト結果も過去の実行記録として残すが、更新後の計画の完了条件には含めない。

- SAAAのPersonal State・モデルへの発話者と時刻の受け渡し・時刻移行・実際の会話Snapshotと保存前の失効判定・Episode参照の継続・検索transport・設定画面・評価器の対象テストは合格。TypeScriptの型検査、対象ファイルの整形、IPC契約、個別の画面buildも合格。
- ContextStillのRust library testは596件合格、11件のlive test等は未実行。SAAAの通常migrationで作った隔離SQLiteを使う取得Adapterの3件と、蒸留したEpisodeの保存・検索・詳細取得・失効の1件も合格。Rust全targetのlint、TypeScript型検査、Spec HTMLの修正・検査も合格。
- SAAAのnormal/full gateは既存のRust警告とratchetで停止。module-sizeにも別領域の超過・未登録・削除済み登録があり、全体合格にはしていない。新規メモリーmoduleだけを登録し、既存のサイズ上限は変更していない。
- ContextStillのnormal gateは既存のschema・provider関連のサイズ超過で停止。別に実行したTypeScript lintは既存wiki処理の2件、全体整形は既存の実行時JSONで停止。新しいEpisode処理のサイズ超過とRust lintの指摘は分割・配置の修正後に解消した。

## 実測が必要な受入項目

ローカルモデルの実行bindingがこの環境にないため、実LLMによる日本語の抽出・Episode採用・時間解釈、方式を一つずつ外した比較、token費用、応答遅延、provider cacheの改善率は未測定である。既存の日本語suiteと評価harnessを残し、評価用の原典でも発話者・時刻・Host Scopeを保持するよう修正した。suiteのgoldは人による意味確認前であり、合成出力で品質認証しない。

意味検索の追加arm、graph、複数回のReflectは、単純な検索が失敗するケースと比較効果が確認できた場合に追加するという元の採用条件を維持する。自然言語による話題の切り替わりの認識、音声を含む実機動作、実ユーザーデータによる連続稼働の受入も、この契約テストだけでは完了としない。既に再生した音声を取り消せることは保証しない。

したがって、コードと決定的な失効・連携の検証完了と、「最良」の品質比較・実運用受入は別の状態として管理する。
