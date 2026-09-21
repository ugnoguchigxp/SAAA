# WorldModel残件の実装状況

更新日: 2026-09-21。実装・回帰試験を継続中。実Providerの受入完了とは区別する。

| カード | 実装内容・主な変更先 | 検証状況 |
| --- | --- | --- |
| T00 | `remaining-baseline.md` に既存正本と送信経路を記録 | 確認済み |
| T01–T02 | core Frame v2、user-only認可、実際の許可Scope集合 | `wr_t01_`, `wr_t02_` 合格 |
| T03 | `situation/world_snapshot.rs`、意味的sequence、停止・観測失効 | `wr_t03_` 合格 |
| T04–T06 | `world/runtime_sources.rs`、委任/Goal/Task join、期限の全Scope順序、既存Coding根拠検査、共通予算 | `wr_t04_`〜`wr_t06_` 合格 |
| T07 | 送信TTLと結果受理を分離。DB正本の再検査はgeneration受理transaction内 | `wr_t07_` 合格 |
| T08–T10 | strict候補schema、専用purpose、既存workerのlease/cancel、host根拠付与、訂正・冪等commit | 単独要素・訂正は合格。五要素を併用する追加回帰を実行中 |
| T11–T12 | 共通refresh、OpenAI各HTTP送信で再構成、実bodyに対応するreceipt | `wr_t11_`, `wr_t12_` 合格 |
| T13 | DynamicLan割当後/共有LARM lease後の実HTTP更新 | 割当を3秒待たせる両経路のfixture合格。実LANは別受入 |
| T14 | AgentSessionのgenerationごとのfresh session、host Tool履歴、release失敗保持 | `wr_t14_` 合格 |
| T15 | Codex app-serverとrole SDKの共通Frame、最終wire receipt、現在依頼の二重投入防止 | app-server dispatch fixture合格。role SDKと全遷移matrixの追加確認が残る |
| T16 | reasoning-answer-v3 / world-evidence-v2、初期化後refresh、実RPCとschema一致 | schema・遅延初期化fixture合格 |
| T17–T18 | source/digest/value/as-ofの一致、質問対象検査、次の期限の最小値検査、未検証Delta遮断、保存直前DB再検査 | 正常/偽claim/生成中変更の実HTTP fixture合格。保存直前検査を追加後の全体回帰中 |
| T19 | 合成前・合成後・音声queueでSituation hold確認 | 合成中hold fixture合格 |
| T20 | 登録済みScope選択、保存済みmessage Scopeから回答の対象表示 | A→B切替/対象削除のcomponent試験合格 |
| T21 | backend能力契約と実送信receiptの分離表示、IPC生成 | 能力・Scope試験、IPC 5件合格 |
| T22 | 既存33ケース維持、カード試験runner、前回report無効化、欠落/重複/0件検知 | 6経路×7遷移の網羅証跡は未完了。カードprefixの合格だけでは本カードを閉じない |
| T23 | module分割、上限100投影/2000ledger/8 Coding/8期限の比較測定 | 同一profileの交互測定へ変更して再測定中。担当サイズ/Clippyを確認中 |
| T24 | 本証跡、親計画、配備手順の更新 | 実行中。未達カードを完了と扱わない |

## 実装時に検出・修正した点

- Coding sourceを既存の所有者・根拠チェックに通し、sourceを忘却した仕事の状態を迂回して読めないようにした。
- 期限を全許可Scopeで統合してからdue/id順に選ぶ。後続の期限を「次」とするモデルclaimも拒否する。
- 抽出候補上限を既存台帳の8件に合わせた。関係の両端やFocusのObjective条件も抽出指示へ明記した。
- 複数根拠を含む抽出patchは、全assertion/transitionがgeneration全体の依存関係を引き継ぐ。モデルの列挙順にかかわらずendpointを先に有効化する。
- Worldの検証済みモデル回答は、保存transaction内でもDB正本を照合する。SituationのmutexはDB writer lockの中で取得しない。
- 全体のsize/Clippyには、並行作業中の担当外変更による未合格項目がある。既存閾値を引き上げて合格にしない。

## 実装ファイルの分割

T09はworkerの実行本体、候補変換、抽出依存source選択に分割。T14はsession所有、transport、round receipt、履歴予算へ分割。T15はapp-server/role SDKへ分割。T18は会話本体、claim選択、claim validatorへ分割。T20–T21はbackend契約、生成IPC、Scope hook、選択UI、回答表示、Provider能力表示に分割した。既存の別機能の編集内容は保持している。
