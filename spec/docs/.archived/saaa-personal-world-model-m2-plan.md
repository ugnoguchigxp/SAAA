# Personal World Model M2 実装計画 — 現在状態への接続

作成日: 2026-09-20。状態: 実装着手用の計画。今回の作業は文書作成と既存試験の確認のみ。

上位: [コンセプト](saaa-personal-world-model-concept.md)。前段: [五要素実装計画](saaa-personal-world-model-initial-plan.md)。実装担当は本書に加え、[M2固定契約](saaa-personal-world-model-m2-execution-contract.md)と[M2詳細カード](saaa-personal-world-model-m2-work-cards.md)を使う。


次段: M2Aの実装報告後は[M3A実装計画](saaa-personal-world-model-m3-plan.md)へ進む。M2Bは外部Source契約の成立を待ち、先に既存FrameのBroker shadow検証を行う。

## 1. 次に実現すること

保存済みWorld Modelの五要素に、SAAAが実際に持つ会議・コーディング作業の現在状態を添えて参照できるようにする。たとえば「このProjectの目標は何か」「関連する作業は実行中か」「会議は終了したか」を、それぞれの正本と確認時点を区別して返す。

現在状態をWorld assertionへ毎回転記しない。Personal Stateの永続WorldSliceと、専門Runtimeからその都度読む現在状態を、一回の要求に限るWorldFrameへまとめる。Runtimeの状態更新、操作許可、会議本文の保存をWorldが所有しない。

M2を二つに分ける。本書で実装するのはM2Aである。

| 段階 | 範囲 | 終了時に言えること |
| --- | --- | --- |
| M2A（本書） | 会議・coding jobの参照、明示Scope対応、現在性検査、既存Goal参照、Runtime由来Focus、外部Evidenceの適格性判定、容量・性能の限定 | 現在状態付きの内部WorldFrameを安全な参照境界で取得・再検証できる |
| M2B（次の入力契約） | Runtime / 外部Evidenceを根拠とする永続更新、必要なSource resolver・版・削除連携 | 会話以外の根拠でも、既存の訂正・忘却と一体に更新できる |
| M3 | 自然文からの候補抽出、根拠取得、必要時推論、共通Broker供給、generation依存検証 | 実際の会話がWorldを利用する |
| M4 | shadow比較・限定有効化 | 固定した品質・遅延基準を満たして利用者へ提供できる |

M2Aだけで外部Evidenceの永続化や通常会話への接続を完成扱いしない。M2Bへ進むための必要条件も§10で明文化する。

## 2. この分割を採る理由

現行 `SourceRef` はRuntime / Externalのroleを定義しているが、adapterの `sources::load / revalidate` と `store::load` はpersonal_sourcesとconversation_messagesへ依存する。roleだけ変えてRuntime IDを登録しても、正しい根拠検証にはならない。

現在のcontextStill接続はmemory-recall-v1のexperience / rule / skillを返す。厳格な返却型には安定した元資料ID・版・削除照会APIがない。返却本文のhashを外部資料の正式な版に見立てて永続化することはしない。

一方、会議の状態はMeetingRuntimeとmeeting_sessions、コーディング作業の状態はcoding_jobs / coding_runsが既に所有している。M2Aはこの正本を参照する形なら実装できる。汎用Source基盤の改造と現在状態の読み取りを一括で実装する必要はない。

## 3. 確認した到達点と残り

確認HEAD: `f5a1069`。着手時に再確認する。

| 項目 | 今回確認した事実 | M2での扱い |
| --- | --- | --- |
| core試験 | lib38＋continuity17＋world_contract2＋world_traversal9＋world_v2_validation15、計81 passed | 再実行する入口の基準 |
| World Adapter | 51 passed / 0 failed / 1 ignored | M1/v2の互換回帰を維持 |
| 完了記録 | D00〜D42はdoneと報告。check:local全体通しは未実施と記載 | 次の入口で全体結果を採り直し、個別成功で全体成功を代用しない |
| 性能記録 | warm-upなし・5 sample。1,000件は途中までしか構築されていない | M2-01で構築済み件数・warm-up5・測定30・p95を補う |
| activate_v2 | 内部API。通常Broker未接続 | M2Aも内部のみ。Provider promptへ投入しない |
| v1/v2投影 | activate_v2はprojection_version 1/2を読む | 旧計画の「2のみ」より現行互換を維持 |
| 会議のScope | Meeting StartInputにProject参照がない | 時刻・発言・会議名からProjectを推定しない。明示されたresource scopeだけ許可 |
| coding jobの版 | revisionがあるが、state変化のすべてで増えるとは限らない | revisionだけで現在性を判定せず、正本の状態tupleもdigestへ含める |

検証記録は `spec/evidence/world-model/m2-planning-baseline.md` に残す。計画作成で全体コードレビュー、live Provider評価、M2機能試験を済ませたとは扱わない。

## 4. 範囲

M2Aで追加するもの:

- MeetingRuntimeの本文・capture tokenを含まない状態snapshot。
- coding_jobs / coding_runsの最小状態を読むowner側Adapter。
- 既存ScopeSnapshotと明示resource/task参照を照合するread-only認可。
- RuntimeRef、RuntimeStateView、WorldFrame、PreparedWorldFrameと再検証API。
- 既存WorldSliceのGoal参照と、Runtime由来の一時Focusの併置。
- contextStill現行契約を「永続根拠には不適格」と明示できる境界判定。
- boundedな入口検査、容量超過時のWorld部分省略、性能の測定。

含めないもの:

- 新しいDB表・payload version・World Kind、汎用SourceRefの変更。
- Runtime状態の複製保存、Goalの新規採用、Projectと会議・Taskの自動紐付け。
- 会議本文、workspace path、coding payload/result本文、capture tokenのWorld返却。
- Tool実行、外部通信、LLM抽出、Context Broker供給、UI・IPC・HTTP・MCPの新公開API。
- 自動調査、新Scheduler、一般的なTask全種類への対応、性能改善目的の全体リファクタ。

コーディング以外のTask種別はunsupportedと返す。既存Runtimeの状態をSAAA全体のTask状態へ一般化しない。M2Aのruntime_refはFrame内の型であり、永続EntityKindV2に追加するものではない。

## 5. 固定する設計判断

| 論点 | 決定 |
| --- | --- |
| 正本 | 永続関係はPersonal State、会議はMeetingRuntime / meeting_sessions、作業はcoding台帳 |
| 接続 | WorldSliceV2を変更せず、graph:Option<WorldSliceV2>を持つWorldFrameで包む |
| 認可 | 信頼済みAccessRequest、resolved run scope、現在のepoch、Project→対象の明示リンクをすべて検証 |
| Scope登録 | 既存scope::register / linkの利用を前提にする。Worldの読み取りから登録・リンクしない |
| RuntimeとGoalの関係 | 同Projectの文脈として並べる。Taskが特定Goalに寄与するedgeを自動生成しない |
| 現在性 | 取得時のDB snapshot＋会議の前後snapshot、TTLと再読取で検査 |
| 期限 | FrameのTTL上限1,000 ms。期限内でも状態変化・Scope失効で無効 |
| Focus | 許可済みRuntimeが動作中の場合だけactive_project / current_work。一時値で保存しない |
| 外部Evidence | 現行memory-recall-v1はtransient_only。元資料の安定ID・版・再検証・削除契約が揃うまで永続採用拒否 |
| 状態の学習 | 会議終了・作業終了を因果関係の反証やconfidence更新に使わない |
| 予算 | Frame全体8,192 byte、Runtime参照8件、graph Node＋Runtime参照30件。入らない説明は単位ごと省略 |

## 6. パイロット容量と性能ゲート

M1の保存上限10,000件を変更しない。ただしM2Aのgraph読取は小規模の検証profileに限定する。Project内の現在投影Entity / Relation / Focusが計100件以下、store::loadが読む全ledger記録の合計が2,000件以下の場合だけgraphを組み立てる。超過時はWorld assertionを削除せず、graph=nullとworld_capacity_omittedを返す。認可済みRuntime状態は予算内で返せる。

全ledger記録とはpersonal_source_refs、personal_tombstones、erased=0のpersonal_assertions、personal_transitions、personal_coverage、personal_patchesの合計。入口検査も上限＋1までしか走査しない。別Projectの大量履歴も読込費用に影響するため、全体上限を隠さない。

| 測定 | 固定条件 | 通過条件 |
| --- | --- | --- |
| Runtimeのみ | 最大8参照、coding jobごとのrun履歴32件以下 | 取得p95 <= 20 ms |
| Frame | 五要素を含む投影100件、ledger記録2,000件以内、Runtime8参照 | 取得p95 <= 150 ms、最大 <= 500 ms |
| 再検証 | 上と同じFrame | p95 <= 150 ms、期限・変更を正しく拒否 |
| 予算 | 長い日本語、上限ちょうど、超過履歴 | byte・件数超過0、上限超過時にstore::loadを呼ばない |

値はM2Aの開発用通過基準であり、音声対話の最終SLOではない。基準を結果に合わせて緩めない。失敗した場合は当該カードを未完了とし、今回の追加レイヤー内の重複load・不要読取を直す。既存ledger全体の作り直しが必要なら別の性能計画へ切り出し、M3接続へ進めない。

同一端末・debug build、warm-up5回・測定30回、nearest-rank p95は昇順29番目。fixture構築と処理時間を分け、requested / created / measuredを記録する。構築5秒超で中断した規模は認定しない。正常な取得をassertしてから測定し、Errを捨てた時間を成功値にしない。

## 7. 受入シナリオ

| ID | 入力・変化 | 期待結果 |
| --- | --- | --- |
| M2-A | Project Pと明示リンクされたactive会議M、同runのresource scopeあり | meetingの現在状態を返す。本文・tokenなし |
| M2-B | A取得後にpause→stop、またはdiscard | 古いFrame再検証はChanged。再取得はpaused / completedまたはunavailable。古いactive Focusなし |
| M2-C | Project Pに紐づくrunning coding job。revisionを変えずstateをcancel_requestedへ変更 | digest変化を検出。cancel_requestedをcancelledと断定しない |
| M2-D | jobがsettled、resultにcompleteがない／false | terminalな実行状態を返すが、Goal達成・Task成功とは表現しない |
| M2-E | Project Qの同名対象、未リンク対象、scope revoked | 名前・状態を返さず、認可拒否か対象省略を固定契約どおり返す |
| M2-F | Active Goal撤回、Source削除、Project epoch変化 | 期限前でもFrame無効。古いGoalや関係を継続利用しない |
| M2-G | World投影stale / pending / 容量超過 | graphの省略理由を返す。Runtimeが正しく許可されていれば参照できる |
| M2-H | memory-recall-v1の有効な返却／同じ返却を二回 | transient_only、永続Evidenceは作成0。hash一致を独立根拠と数えない |
| M2-I | max_bytes=1、Runtime参照9件、now=expires_at | BudgetTooSmall、Limit、Expired。要求上限を増やさない |
| M2-J | 再起動、会議正本のinterrupted、coding recoveryのoutcome_unknown | 現在のowner状態を返し、前processのPreparedFrameは使用不可 |

すべて合成DB・固定時刻・fake ownerで再現し、実装済みowner Adapterについて実DB表を通る統合試験も用意する。fakeだけで実接続完了とはしない。LLM・ASR・pi subprocess・外部サービスは起動しない。

## 8. 検証コマンド

```sh
# G1 core
bun run check:personal-state
# G2 World / Personal State
cargo test --locked --manifest-path src-tauri/Cargo.toml --lib memory::personal_state
# G3 owner回帰
cargo test --locked --manifest-path src-tauri/Cargo.toml --lib meeting::
cargo test --locked --manifest-path src-tauri/Cargo.toml --lib coding::
# G4 scope・Context・復元
cargo test --locked --manifest-path src-tauri/Cargo.toml --lib runtime::context
cargo test --locked --manifest-path src-tauri/Cargo.toml --lib persistence::
# G5 size・文書
bun run size:check
bunx --bun spec-html check ./spec/docs --warnings-as-errors
# G6 最終
bun run check:local
bun run test:rust-packages
```

filter0件は成功扱いしない。カード単位は対象試験を先に実行し、該当suiteを続ける。最終G6を個別コマンドで代替して「全体通過」と書かない。既存の別文書MD001等はbaselineと比較して残存を明記する。今回起因の失敗は完了不可。

## 9. 完了記録

新規記録は `spec/evidence/world-model/m2-progress.md` と `m2-results.md`。旧v2のdoneや性能認定をコピーしない。カードごとに変更関数、試験名・件数、具体期待値、結果、未実施・失敗、次カードを残す。

M2A完了には全カード、M2-A〜J、読取の副作用0、認可・Source失効・再起動・予算の試験、性能ゲート、Scope登録が必要な運用境界の説明が必要。返却Frameは内部診断用であり、通常会話への投入はまだないと記録する。

## 10. M2B・M3への引渡し条件

M2Bは、外部側が安定resource ID、immutable revisionまたは内容digest、権限とScope、版を指定した再取得、deleted / unavailable / changedの区別を提供できることを前提にする。接続先の名前だけで実装済みとみなさない。

M2Bで新しいSource resolverを追加する場合は、store::load、commit、Reader、依存索引、tombstone/journal、バックアップ復元、generation再検証までを一つの設計として具体化する。SourceRoleだけを変える実装は禁止する。Runtimeについても、Frameの一時snapshotと監査可能な履歴イベントのどちらを根拠に保存するかを先に決める。

M3は既存BrokerのCandidateとGenerationHandleへ接続する。FrameのTTLだけでは生成中の失効を保証できないため、dispatch時と出力時の依存再検証、選択・省略、非命令表示、World無効時の継続を必要条件とする。M2Aの再検証関数はその部品であって、M3の完成を意味しない。
