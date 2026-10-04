# 用途別クラウドAPI切替 実装レビュー

対象はcommit `86826f02fe0e14ef2ada1526797e156f68b57727`の当該機能。2026年10月4日、technical / deep / agentプロファイルで評価した。単一commitには無関係な変更も含まれるため、登録、保存、移行、会話transport、音声Binding射影、設定UIとその消費先に範囲を限定した。

**結論: 設計の骨格と直接クラウド会話は成立するが、初期提供の完了条件は未達。全計画の実装完了とも判定できない。確認済みの問題は6件（High 2件、Medium 4件）。出荷前にP1の5件を修正する必要がある。**

技術品質の正式な100点満点スコアおよび暫定総合点は保留する。採点基準の証拠カバー率は60.3%、信頼度はMedium。security/reliabilityの全基準、配布・依存管理、代表性能測定を網羅していないためであり、未検証を満点にも欠点にも置き換えていない。除外重み0%、未検証はUnknownのまま残した。F01は送信停止制御の失敗で、ルーブリック上の上限59点・production_readiness=blockedが適用される。ただしこれは機能範囲の判定であり、製品全体の品質評価やリリース認証ではない。事業性/Project Potentialは今回評価しない。

先に直す順序は、①無効化後の送信停止、②選択候補と実行先の一致、③登録途中の復旧、④保存ID規則とクラウド受入fixture、⑤既存互換optionsの反映。全体期限も用途詳細を公開する前に保証する。

## コードレビューで確認した問題

### F01 / P1 / High: 無効化後も同じクラウドに送信する

[ornith.rs](/Users/y.noguchi/Code/SAAA/src-tauri/src/runtime/conversation_check/queue_runtime/ornith.rs:162)はjob開始時のRouteを全stepで使うが、各送信前に対象接続/resourceの現在の有効状態を再確認しない。[Contextの検査](/Users/y.noguchi/Code/SAAA/src-tauri/src/runtime/conversation_check/queue_context.rs:35)は会話・Scope・Worldの整合性であり、接続の取消を検査していない。

一時コピーで最初の推論が検索actionを返した直後に、通常の保存関数で接続を無効化した。**その後も3回のLLM要求が続き、合計4回、接続enabled=falseのまま回答が保存された。** 根拠は[cloud-revoked.log](/Users/y.noguchi/Code/SAAA/spec/evidence/purpose-cloud-api-review/20261004/cloud-revoked.log)。このprobeのexit 0は欠陥挙動を観察できたことを表し、受入合格を表さない。

通常の用途選択変更から実行中jobを守るRoute固定は維持すべき。固定したIDの最新拒否状態を送信/Tool前に検査し、拒否なら代替先へ切り替えず停止する。secretは各stepで読み直されるため、「secret削除も無視する」という指摘はしていない。受入条件は無効化後の追加LLM送信0件、後続Tool停止、未確定回答の採用なし。

### F02 / P1 / High: 選択可能な候補と実行できるサービスが違う

[candidatesFor](/Users/y.noguchi/Code/SAAA/src/lib/serviceRegistry.ts:181)は能力だけで候補を作る。一方、[transport_for](/Users/y.noguchi/Code/SAAA/src-tauri/src/runtime/conversation_check/direct_route.rs:32)はChatCompletionsとLarmだけを処理する。

- AgentSessionは会話用resourceとして移行・表示され、保存も成功するが、次の会話が「まだ対応していない」で失敗する。
- DynamicLanはLarmに分類されるが、Larm分岐がResolvedRouteを捨てる。実際の接続準備は[providers.harness.address](/Users/y.noguchi/Code/SAAA/src-tauri/src/runtime/conversation_check.rs:830)を使うため、選択したDynamicLanのhostが実行先にならない。
- 音声の「使わない」もUIでは選べるが、[保存側](/Users/y.noguchi/Code/SAAA/src-tauri/src/persistence/settings/registry_projection.rs:30)は未選択を拒否する。

AgentSessionの保存→拒否、非harness LarmのRoute情報破棄は[backend-probes.log](/Users/y.noguchi/Code/SAAA/spec/evidence/purpose-cloud-api-review/20261004/backend-probes.log)でも確認した。実機DynamicLanへの通信を実測したとの主張ではなく、実行分岐とcalleeのsource追跡に基づく。

最小の修正は用途とadapterの対応表を候補・保存で共有し、未対応の理由を表示すること。harness以外のLarm候補は提供を止めるか、解決済みendpointを接続準備へ渡す。受入では各adapterを画面で選び、保存と再起動を経て実送信先を確認する。

### F03 / P1 / Medium: 登録途中の失敗から再開できない

[registerService](/Users/y.noguchi/Code/SAAA/src/features/settings/PurposeRoutesSection.tsx:69)はdisabled draftを保存するが、`setView`はsecret保存と有効化まで成功した後にしか呼ばない。途中で失敗すると画面のrevisionが古いままになる。

React画面にsecret保存故障を注入したところ、DB相当のrevisionは1、再試行の送信revisionは0だった。再読込後もdraftはdisabled候補となり、キー再保存・有効化・同じIDで再開する操作がない。[registration-probe.log](/Users/y.noguchi/Code/SAAA/spec/evidence/purpose-cloud-api-review/20261004/registration-probe.log)に記録した。

各保存成功後にview/revisionを即時反映し、保存済みdraftに対する再開UIを設ける。2段階目/3段階目の失敗と画面再表示後、同じIDで完了でき、重複draftが増えないことを受入条件にする。draftを先に保存する順序と、失敗時にsecretを勝手に削除しない方針は維持する。

### F04 / P1 / Medium: 保存で受理したIDを読出しが捨て、受入fixtureが失敗する

[overlay_legacy](/Users/y.noguchi/Code/SAAA/src-tauri/src/persistence/service_registry_store.rs:49)は`conn:svc-`/`res:svc-`だけを新規として残す。しかしvalidatorはこのprefixを要求せず、保存を成功させる。受理した接続が読出しで消え、参照Bindingが無効化される。

[クラウドfixture](/Users/y.noguchi/Code/SAAA/src-tauri/src/conversation_queue_e2e/cloud_route.rs:20)の`conn:cloud-llm`/`res:cloud-llm`がこの条件に該当する。元のfixtureのクラウド分岐を単独実行すると、通信0件、30秒で回答待ちが失敗した。[cloud-original.log](/Users/y.noguchi/Code/SAAA/spec/evidence/purpose-cloud-api-review/20261004/cloud-original.log)。一時コピーでfixture IDだけを適切なprefixに変えると、**LLM 4回・LARM要求0件で回答が採用された**。[比較結果](/Users/y.noguchi/Code/SAAA/spec/evidence/purpose-cloud-api-review/20261004/cloud-fixed-fixture.log)。

通常UIは適切なprefixを生成しているため、正常なUI登録がすべて消える問題ではない。保存前にID所有規則を統一し、受理したIDは保存後も保持する。fixtureを修正し、save戻り値に選択した接続/resource/Bindingが残ることと、元のクラウド受入分岐の成功を検査する。

### F05 / P1 / Medium: 保存済みLLMの互換optionsを会話で使わない

[移行](/Users/y.noguchi/Code/SAAA/src-tauri/src/providers/service_registry/migration.rs:69)はendpoint/model/authenticationを写すが、既存の`request_options`をResourceやResolvedRouteに含めない。[新しい送信](/Users/y.noguchi/Code/SAAA/src-tauri/src/runtime/conversation_check/direct_route.rs:88)は毎回DefaultのLlmOptionsを作る。

例えば単体試験で`tokenLimit=completion`が必要な非標準モデル名でも、会話は自動判別の`max_tokens`を送る。thinking/temperature等の明示設定も使わない。legacy documentの値は残るためデータ消失ではないが、同じProviderの単体試験と会話の送信契約が異なる。外部APIの拒否を実行確認したわけではなく、[共通wire変換](/Users/y.noguchi/Code/SAAA/crates/larm-session/src/http_api.rs:56)までsourceを追跡した判断で、信頼度はMedium。

ResolvedRouteに必要なoptionsを固定し、会話hostの`tools=false`だけを明示上書きする。model名だけでは推測できないCompletion指定、thinking、temperatureを保存→再読込→実requestまで確認する。

### F06 / P2 / Medium: 全体期限がstepごとにリセットされる

[timeoutの選択](/Users/y.noguchi/Code/SAAA/src-tauri/src/runtime/conversation_check/direct_route.rs:87)は各stepで同じ値を使い、残りのjob期限を消費しない。Tool側も旧routingのtimeoutを使う。SSEからJSONへの再要求も同じtimeoutを再利用する。

全体/attempt各1秒、各fake LLM応答に400ms遅延を与えると、4stepを**約1.9秒**で成功採用した。[cloud-deadline.log](/Users/y.noguchi/Code/SAAA/spec/evidence/purpose-cloud-api-review/20261004/cloud-deadline.log)。単一attemptのtimeoutとstep上限はあるが、用途の全体期限を保証していない。

job開始時のdeadlineを固定し、各request/Tool/再要求で残量とattempt上限の小さい方を使う。期限後の追加送信と採用がないことを確認する。

## 計画の達成度

| 段階 | 評価 | 根拠・残る条件 |
|---|---|---|
| P1 登録・保存・移行 | 部分実装 | 3層、競合、secret参照はある。保存IDとlossless optionsに問題 |
| P2 会話LLM | 部分実装 | fixture修正後に直接LLMとTool loop成立。取消、候補適合、全体期限、fallback/attempt永続監査が未達 |
| P3 ASR/TTS | 共有Bindingまで | 旧routingへ同transactionで反映。新Registry資源のvoice利用は未対応。実機受入なし |
| P4 設定UI | 最小UI | 登録と3用途選択あり。途中復旧、無効化/削除、接続確認、詳細設定、実利用先表示、プリセットが不足 |
| P5 画像・音楽 | 未実装 | 共通Bindingから消費先への接続なしと計画に記載 |
| P6 記憶・World・埋め込み | 未実装 | 索引移行と共通adapter接続なしと計画に記載 |
| P7 Agent・検索 | 未実装 | 用途別接続なしと計画に記載。現在の会話内Tool検索が存在することとは別 |
| native Model API adapter | 未実装 | 全計画の受入要件 |
| P8 実サービス・実機受入 | 未実施 | API適合、再起動、選択と送信先、AEC/TTS-only/重なった人声、性能の受入記録なし |

根拠は[計画の完了条件と追記](/Users/y.noguchi/Code/SAAA/docs/plans/purpose-based-cloud-api-switching.md:307)。部分実装との記載は実装者自身の記録であり、独立した完了証拠としては扱わない。上表の初期経路はsourceと隔離試験でも照合した。

## 維持する設計

3層の責務分離、純粋なRoute解決、single writerのtransaction、expectedRevision、旧設定とsecret参照の保全、needs-reviewで勝手にクラウド送信しない移行は妥当。音声Bindingを同transactionで旧routingへ射影することで正本を増やさずに導入している。直接会話も既存Context/tool loopを使うため、別executorを増やす必要はない。

修正は現在の構造内で可能。共通adapter対応表、固定Routeと最新拒否状態の分離、optionsの明示保持、登録draftの状態投影で足りる。登録/用途/秘密情報を全面刷新したり、Provider数ごとに新executorを増やす案は今回の不整合を解決するためには不要。

## 実施した検証と限界

| 検証 | 結果 |
|---|---|
| 既存Registry Rust unit | 14 pass |
| Registry helper/関連設定 frontend | 21 pass |
| Provider unit既定script | 3 pass |
| Continuous ASR/packetizer/IPC | 21 pass |
| TypeScript型検査 / frontend build | 成功 |
| 元のクラウドE2E分岐を単独実行 | 失敗: 30秒timeout、通信0件 |
| fixture ID修正コピーで同じqueue | 成功: 4LLM、0LARM、回答採用 |
| 接続無効化 / 全体期限の故障probe | 欠陥を再現。受入合格ではない |
| 登録途中失敗のReact probe | 欠陥を再現 |
| 保存/adapterの追加Rust probe | 3件で欠陥を再現。広いフィルタの既存38件もpass |

実DB・実APIキー・実サービスは使っていない。git archiveで固定snapshotを作り、in-memory DB、localhost fixture、mock IPCを使った。node_modulesとcargoのbuild cacheは既存を参照し、frontend/生成resourceとprobeソースはコピー内へ出力。初回のRust compileはコピーにdistがなかったため失敗し、frontend build後に解消。混載した音声テストはmock干渉で1件失敗したが、既定scriptと分割実行で全対象通過。この2件を機能の回帰として数えていない。Rust警告多数とfrontend chunkサイズ警告は記録するが、本変更への帰属を確定していない。

全テスト/`check:local`/実機P8は実行していない。エンドポイント/secret参照に関する全脅威経路、CI継続運用、依存・配布、費用/メモリ/実音声性能も未検証。source正本は書き換えておらず、対象ファイルのSHA-256はsnapshot採取後と最終確認で一致した。レビュー中に現れた範囲外の作業ファイルは評価せず変更していない。

## 採点した観点と改善手順

下表の点は**採点した観点だけの値/10**で、未調査を含む製品全体の評価ではない。重みは事前選択したagentプロファイル。詳細な根拠は[review.json](/Users/y.noguchi/Code/SAAA/spec/evidence/purpose-cloud-api-review/20261004/review.json)。

| 観点 | 観測値/10 | 基本/実効重み | カバー率 | 信頼度 | 主な根拠 |
|---|---:|---:|---:|---|---|
| 構造 | 3.3 | 12% / 12.0% | 100.0% | Medium | 3層分離と保存/実行不一致 |
| 正しさ | 2.5 | 15% / 15.0% | 100.0% | High | F02/F04/F05 |
| 送信・権限境界 | 2.5 | 18% / 18.0% | 33.3% | High | F01。secret/abuseの全体はUnknown |
| 障害と復旧 | 2.5 | 12% / 12.0% | 66.7% | High | F03/F06。dependency飽和はUnknown |
| 保守性 | 5.0 | 8% / 8.0% | 33.3% | Medium | 主要pathの追跡性。代表変更と長期コストはUnknown |
| 共通部品 | 4.2 | 5% / 5.0% | 100.0% | Medium | 既存queue/secret/writer再利用とoptions不一致 |
| 検証 | 3.8 | 10% / 10.0% | 66.7% | Medium | 単体とfrontend成功、主経路fixture失敗 |
| 依存・配布 | 未評価 | 8% / 8.0% | 0.0% | Unknown | 今回未調査 |
| 性能 | 未評価 | 7% / 7.0% | 0.0% | Unknown | 代表条件/費用/実機計測なし |
| 設定・操作 | 2.5 | 5% / 5.0% | 100.0% | High | 復旧と適合表示に不整合 |

採点済み部分だけの計算平均は30.5/100、Unknown基準の算術上下限は18.4–58.1。これは暫定総合点として公表できる条件を満たさず、全体品質の点数・統計的な信頼区間を意味しない。[score-summary.json](/Users/y.noguchi/Code/SAAA/spec/evidence/purpose-cloud-api-review/20261004/score-summary.json)に計算とgateを保持した。

| 順序 | 対応 | 依存・受入 | 工数の扱い |
|---|---|---|---|
| 1 | F01 最新拒否の再検査 | 固定Routeを維持しながら無効化以後の送信/採用を停止 | 変更境界は明確。許可Scopeの接続範囲を確認して見積 |
| 2 | F02 候補とadapter契約 | 非対応を理由付き拒否、選択と実endpoint一致 | 共通対応表で開始。DynamicLanまで対応する場合は別途実機検証 |
| 3 | F03 保存済draftの再開 | 同じIDでsecret/enableを再開、競合・再表示・重複なし | UIと保存結果の状態投影。バックエンド全面変更は不要 |
| 4 | F04 保存IDとfixture | accepted snapshotのroundtrip、元のqueue受入成功 | 小さい保存契約とfixture修正。元のE2E再実行が必要 |
| 5 | F05 options保持 | 旧互換設定をrequestまでlosslessに適用 | 型・移行・ResolvedRoute・送信を横断 |
| 6 | F06 deadline | 全request/Toolが残量を消費、期限後の送信/採用なし | ToolやSSE再要求まで跨ぐため単一attempt修正では不十分 |
| 7 | 初期提供の再判定 | 上記修正、残るP1-P4要件と対応adapterのP8受入 | 成功を示すfixtureと実機記録が揃ったsnapshotで再評価 |

予想スコアの上乗せは行わない。修正後は別snapshotで故障probeを受入assertに変更し、実際の検証結果で再評価する。

証跡: [snapshot.json](/Users/y.noguchi/Code/SAAA/spec/evidence/purpose-cloud-api-review/20261004/snapshot.json)、[review.json](/Users/y.noguchi/Code/SAAA/spec/evidence/purpose-cloud-api-review/20261004/review.json)、[score-summary.json](/Users/y.noguchi/Code/SAAA/spec/evidence/purpose-cloud-api-review/20261004/score-summary.json)。context_compile / compile_evalは今回各1回、会話累計各3回。initial_instructionsは会話開始時の1回のみ。
