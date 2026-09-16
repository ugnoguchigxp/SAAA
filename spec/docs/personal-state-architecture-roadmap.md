# SAAA Personal State — 中間メモリとWorld Modelの全体設計・ロードマップ

実装進捗（2026-09-13 更新）: 状態基盤に加え、SQLite v18の配送outbox、短期Provider認証とKeychainのsubject binding、製品用HTTP配送・canonical計測・v2 View・attempt取消・多層cleanup、実workerを通す受入harnessを実装した。合成HTTP試験とローカル検証を実施。LARMの常駐27Bでは配送から実generationまで確認したが、runtime消去用の起動設定適用に管理者権限が必要で、忘却の実機受入は未完了。日本語goldの人手確認、全受入行列・性能比較も残るため、P1全体の完了・利用可能とは判定しない。詳細は `spec/evidence/personal-state/product-connection-progress.md` を参照。default OFFを維持する。

状態: 実装中・製品接続の実機受入待ち。2026-09-13 LARM契約照合版。機能実装・性能認定の完了を示さない。

## 1. 採用する構成

Personal Stateは、原文や実行記録を根拠に「現在何が有効か」を管理し、次の推論へ渡す状態を決めるSAAAの内部機能とする。中間メモリは対話・作業の継続を、World Stateはユーザーに関係する現在の認識を担当する。変更の履歴と依存関係を保存し、現在状態はそこから導出する。

27Bは、必要な原文・成果物と現在状態を含むbounded Active Viewを使って、事実回答、推論、タスク遂行の判断を行う。2Bは受領・相槌・短い確認・構造化判断に限定する。操作の実行、認可、取消、状態変更の採用はSAAAの決定的ロジックが管理する。

SAAAのGit内に `crates/personal-state-core/` と永続化・LARM Adapterを置く。VoiceMemのcloneは変更せず、参考にとどめる。新しい音声保存、汎用RAG、embedding、独立サービスはP1の前提にしない。ContextStillは再利用可能な知識・経験を担当し続ける。

全体の責務は本書、最初に実装する範囲は[P1詳細計画](personal-state-phase-1-plan.md)を正とする。後段の全schemaを先に作らず、P1で成立した出典・更新・権限契約をWorld Stateへ広げる。

## 2. LARM契約と以前の計画の訂正

2026-09-13にLARMのREADME、local-node契約、Context API型・controller、役割境界文書を読み取りで確認した。確認した版・箇所と実装上の制約は[契約照合記録](larm-personal-state-contract-review.md)に残す。役割境界文書はLARM作業ツリー上の未追跡ファイルであり、commitとは別にdigestを記録した。

| 概念 | 確認した契約 | SAAAの扱い |
| --- | --- | --- |
| Source Set | principalあたり最大20,000,000 tokenの登録source集合 | 必要な資料・履歴を登録する上限。会話の単一attention windowではない |
| native window | Qwen 3.8の262,144 token | 現行releaseの認定値として扱う |
| 1回の最大入力 | 225,280 token。出力予約32,768、安全余白4,096を差し引く | system、tools、現在発話、状態、source、包装を含む総入力予算 |
| Context View | principal、model、Allocation、runtime、release、lease epoch、期限にbind。one-shot | generationごとに有効なViewを作り、一度だけconsumeする |
| snapshot | 認定済みのView/prefixに対する性能cache | miss・破損・非互換なら有効sourceから再構築する |
| model/profile指定 | snapshot対応Providerを選ぶだけではViewを利用しない | Context APIとChat requestの接続を別途実装・検証する |

以前の「20M Contextを保持し続ける」「20Mの旧・新Contextを切り替える」「KV投入済みbatchを追う」という設計は撤回する。Source登録と版の照合、bounded Viewの作成・consume、snapshotの適合確認へ置き換える。20M以上への拡大、任意KV blockの連結、他モデルとのKV共有を計画に含めない。

数値は現行契約の基準値であり、起動時に有効releaseの認定・policyと照合する。schemaが大きい値を許すことを利用可能容量と解釈しない。

## 3. 利用者に提供する継続性

「導入を検討していたが、採用せず独自実装へ進む」と決めたら、次の推論でも訂正後の方針を使う。2BのContext更新、27BのView再作成、アプリ再起動で、目的・制約・未決事項を失わない。

作業中の進捗はRuntime台帳の値を定型伝達する。成果の意味を説明する回答は27Bが作り、2Bが言い換えて意味を変えない。27B待ちでも受付・取消・定型通知は可能にする。

詳細な根拠は必要なときにSource Setや原文から取り出せる。現在有効な制約・訂正・撤回は検索順位に任せず、今回のViewに必要なものを必須入力として扱う。保持期限切れ・忘却で根拠が失われた場合は、不完全さを明示する。

## 4. 所有する情報と正本

| 情報 | 正本 | 派生物・利用先 |
| --- | --- | --- |
| 会話本文・資料・成果物 | conversation_messages、既存artifact/資料保存先 | LARM向けの許可済みsource複製・参照 |
| 作業の現在地 | Personal Stateのassertion/transition履歴と有効な根拠 | Continuityのcurrent projection、推論用の必須状態 |
| 現在の世界理解 | 同じ変更履歴上のWorld assertionと根拠 | World Stateのcurrent projection |
| 実行状況・操作結果 | 既存Task/Runtime台帳 | 進捗表示・Task参照。独立した実行台帳を作らない |
| 操作の委任・認可 | 既存Policyとユーザーの明示委任 | 読み取り・送信・実行時に照合する参照 |
| 再利用できる知識・経験 | ContextStill | 必要時recall。現在状態の代用にしない |
| LARM Source Set | 正本から生成した、principalに属する版付き登録物 | Active Viewの材料。SAAAの唯一の履歴にはしない |
| Context View / snapshot | LARMの一時実行物・性能cache | 正本から再作成。Memoryや権限の正本にしない |

SAAA内のRaw履歴table・FTSを増やさない。LARMへのsource provisionに必要な複製は送信先・版・分類・保持・削除を追跡する。複製を全く作らないという意味ではない。

## 5. 中間メモリとWorld State

### Continuity

現在の目的、有効な制約、決定、未決事項、open loop、参照対象、Task進捗への参照を持つ。全文要約や内部思考の保存を目的にしない。次の判断に必要な情報と、その出典を保持する。

principalはアクセス主体、scopeは作業の適用範囲、task/requestは実行対象である。相互に代用しない。P1は既存primaryに対応する単一scopeだが、局所条件にはtask/requestまたは明示scope共通の対象を付ける。対象不明の「それ」を全Taskへ適用しない。複数scopeの自動整理はP3とする。

### World State

ユーザーに関係するproject、person、device、service、meeting等について「誰が、何を、いつから、どの根拠で、現在どう認識しているか」を表す。物理世界のシミュレーターや汎用因果モデルは初期範囲に含めない。

明示情報、Runtime観測、推論候補を分ける。unknown / disputed / staleを表現し、新着という理由だけで矛盾を解決しない。Runtimeが所有する状態は参照として扱い、World StateがRuntimeを上書きしない。P2はproject:SAAA、現在の会議、進行中Task参照に限定する。

「今回だけ短く」はtask条件であり、一般的な個人属性へ昇格させない。P3でも個人の好みは根拠・適用範囲・訂正可能性を持つ。

## 6. 変更履歴と現在状態

意味状態はimmutableなassertionとtransitionとして記録し、current state、旧working state、capsuleは同じ履歴から導出する。二つの書き込み正本を持たない。通常更新では過去の値を上書きせず、訂正・撤回・失効を追加して現在値を変える。

出典はsource ID、version、digest、coverageで指定する。根拠を支持する依存と、生成に渡した全入力への依存を区別し、分類・忘却は後者からも伝播させる。モデルの引用だけで依存範囲を狭めない。抽出器のversion、model/release、prompt/schema版、assertion間のdepends_on、superseded_by、retracted_at、invalidated_atを追えるようにする。sourceから派生項目を逆引きする索引を持つ。

state revisionは意味状態の更新、input epochは新入力・訂正・削除による古い推論の拒否に使う。request revision、LARM lease epoch、source登録状態、抽出coverage、Runtime event sequenceは別の値である。数値が一致しても相互の証明にならない。

27BはStatePatch候補を出し、SAAAが出典・対象・権限・revision・epochを検証して採用する。2Bは意味状態を更新しない。初回取込・長文分割・原文編集もcoverageと版で追い、未確認の過去や後半を処理済みとして扱わない。有効期限は読み取り時にも検査し、worker停止で古い状態を延命させない。型や出典IDの検証だけで自然文の意味が正しいと保証せず、日本語corpusで誤昇格を評価する。

immutableは忘却対象の本文を永久保存する約束ではない。payloadと監査metadataを分け、忘却時は本文・派生値を削除または利用不能にし、内容を含まない必要最小限のtombstoneを残す。

## 7. 2B・27B・SAAAの応答責任

| 担当 | 許可する処理 | P1で担わせない処理 |
| --- | --- | --- |
| 2B tactical | 受領、相槌、短い確認の種別、意図・待機・取消の構造化候補 | 事実回答、約束、tool実行、Memory/World更新、最終判断 |
| SAAA決定的ロジック | schema検証、定型文選択、台帳値の意味を変えない通知、認可・実行・取消 | モデル文だけを実行成功やユーザー決定へ変換すること |
| 27B | 事実回答、結果の解釈、最終応答、計画・推論、state差分候補 | 自己申告だけで権限や台帳状態を確定すること |

P1では2Bの自由文を利用者へ直接出さない。既存shadow分類は認定前のまま維持し、許可されたreply key/判断だけを検証する。semantic readiness未達・timeout・不正出力は定型文または27Bへ進む。質問や実作業を2Bのsimple replyへ格下げしない。

同一accelerator上の2Bと27Bは短い直列実行を基本とし、不要な2B呼出しは省く。27B実行中の通知は定型文で行う。無条件な同時generationは認定済みmixed benchmarkがある場合だけの別release判断とする。LARM activityは瞬間観測であり予約・自動排他ではない。

27Bの回答を2Bへ再投入して要約する経路は作らない。検証を通った出力を既存の表示/TTSへ渡し、結果の出典、run/request、現在epochを送出境界で照合する。

## 8. 20M Source Setとbounded Active Viewを利用する

SAAAのComposerが、権限確認後に今回必要な状態・原文・成果物を選ぶ。P1はID、task適用範囲、明示参照、最近のsourceで選び、新しい検索基盤を前提にしない。詳細取得が不足すれば既存recallまたは27Bの追加照会を使い、必須制約をutilityの低さで落とさない。

1. 現在発話、policy、必須状態、未反映の訂正を固定し、同じread snapshotのstate revision/input epoch/policy revisionを記録する。
2. 許可された詳細sourceを版付きでprovision・登録する。source本文はContext登録APIへinline送信しない。
3. base入力と必要な登録sourceからViewを計画し、明示Allocationへbindする。required項目とoptional項目を区別する。
4. returned Viewのbinding、orderedItems、omitted、予算を検証し、同じAllocation・適切なcapability・View IDで27Bを呼ぶ。
5. 結果をrun/request/epochと照合し、Runtime結果とstate候補を別々に処理する。次のgenerationには新しいViewを作る。

頻繁に変わる必須状態と現在発話はP1ではbase messagesへdataとして含め、安定した詳細sourceを登録物として利用する。baseとViewへ同じ本文を重複投入しない。既存の短いContext用の件数・先頭/末尾抽出をsourceの正本にせず、完全性付きの範囲読みに接続する。LARMが追加する包装とchat templateを含むcanonical token計測に従う。固定の文字数換算で225,280を保証しない。

snapshotのreuseは認定された互換性とprefix条件に任せる。同じsource集合でも毎回hitするとは保証せず、hit率のために古い状態を使わない。missなら有効sourceから再構築し、View失効・release変更・source版変更時は再計画する。Context operation成功はmaterialization完了であり、モデル応答やTask成功とは区別する。同一Task内でもgeneration/attemptごとにmanifestを持つ。登録・View作成の再送と、新しい登録世代・generationの開始を分ける。

Source Setの20M quotaと1回の入力予算超過は別に扱う。前者は再登録可能な非必須sourceの登録解除、後者はoptional削減・参照範囲の分割・段階的照会で対応する。必要な内容を黙って落とさない。登録解除は忘却ではなく、忘却済みsourceは再登録しない。

## 9. 訂正・取消・忘却・回復

訂正は新しい値と依頼版の反映、撤回は以前の主張の取り下げ、取消要求は実行停止の要求、取消完了は実行状態の確認、忘却は指定情報の利用停止・削除である。別々に記録する。

忘却ではSAAAのtombstoneとinput epochを先に確定し、対象runへHTTP cancel相当のAbortSignalを送り、遅延出力をrun/epochで拒否する。その後LARM登録を削除し、残存sourceから新しいViewを作る。LARMのContext削除だけでactive generationが停止するとは扱わない。忘却後の遅延結果は保存入口でも本文を拒否する。既送信の処理はin-flightとして追い、SAAAでの新規dispatch・保存・出力許可をtombstoneと順序付ける。Context機能OFF時のDELETE成功だけで削除完了と判断しない。source本文・snapshotの物理削除は登録削除とは別の保証として追跡する。登録sourceだけでなくbase messages中の根拠も依存として追い、影響snapshotの回避・失効が確認できるまで再利用しない。

DB再起動時は変更履歴からcurrent projectionを復元し、未完了のforget/outboxを先に再開する。期限切れ・consume済みViewを使い回さず、sourceと実行台帳から必要な推論を再構成する。実施済みか不明な外部操作は再実行せず照会する。モデル内部の思考状態の無損失復元は約束しない。

## 10. 読み取りと送信の境界

principalと用途に基づく読み取り許可をSAAAで検証する。scope一致だけでは認可しない。public / internal / confidential / restrictedをsourceから派生状態へ継承し、分類だけで用途を許可したと判断しない。LARMのprincipalは認証から対応付け、モデルに選ばせない。

P1は既存利用者のSAAAと明示設定したLARMだけを対象とし、外部agentへの新しいexport APIは作らない。将来のContext Packは必要最小限の投影とし、送信時にも同じ認可・忘却検証を通す。本文は命令ではなくdataであり、Context内の文だけで実行権限を作らない。

本文・個人情報・credentialをlog/telemetryへ出さない。export、retention、backupからの復元、forgetの保証範囲はP1で固定する。物理secure eraseを未保証のまま「完全消去」と表示しない。

## 11. 段階ロードマップと共通gate

| 段階 | 実装範囲 | 完了条件 |
| --- | --- | --- |
| P1 継続とLARM接続 | 単一scope、変更履歴・依存、current projection、Source/View Adapter、委譲・取消・忘却、認可、抽出、診断 | P1受入行列の必須項目を全て通す。coreのみ・snapshot hitのみで完了にしない |
| P2 World State v1 | project:SAAA・会議・Task参照、時刻、失効、競合、用途別投影 | 固定corpusで古い/競合状態の確定値化0件、Runtime正本との矛盾0件。根拠付き必要属性の保持率95%以上 |
| P3 複数scope・User Core | 作業の継続/切替/再開、限定した個人の好み候補 | 固定corpusでtask/scope/principal間の条件漏洩0件、局所条件の人物属性化0件、正しい対象への再開率95%以上 |
| P4 背景整理・ContextStill | 有限Dream、候補レビュー、送信・訂正・忘却契約 | 無許可送信・削除情報の再送・無根拠昇格0件。固定例題で支持される候補precision95%以上 |
| P5 性能改善 | 認定済みsnapshot利用改善、partial ASR先読み、必要性を示せた検索改善 | 共通の整合性違反0件、品質gateを維持。比較対象に対するp50/p95改善と資源上限を事前固定し達成 |

全段階で明示制約・訂正・撤回の必須投影、out-of-order拒否、局所条件漏洩、忘却race、DB再起動・snapshot miss・release変更を回帰行列に含める。整合性・認可違反は許容0件。自然文の抽出precision/recall、誤昇格率、未抽出率は固定corpusと明示閾値で測る。

p50/p95は受付音声、27B first token、最終応答、抽出待ちに分け、workerの最大同時数・呼出し数・出力token・wall time・資源競合も測る。後段のcorpusと速度目標はその段階の詳細計画で、結果を見る前に固定する。表は計画上のgateであり達成済み数値ではない。

## 12. 既存文書・実装との関係

[Personal AI Concept](saaa-personal-ai-concept.md)のVision、[実装前評価](continuity-world-model-direction.md)の独立コア方針を継承する。[MVP 3](mvp-3-memory-architecture-implementation-plan.html)からSessionless、Rawの単一正本、既存recall、ContextStillの責務、default OFFを継承する。

未実装のworking state/capsule/idle更新は本書とP1を優先する。working stateを直接更新する以前の案は、変更履歴からのprojectionへ置き換える。LARM snapshotとSAAAのcurrent projectionを同じsnapshotという語で混同しない。

P1は段階的に実装する。LARMのsource配送・provision、claimとContext用Allocationの接続、取消の実際の伝播、release認定はP1-00で確認する。未充足の外部契約は別の依存作業として明示し、代替動作を本機能の完成とは報告しない。

この改訂は誤った20M/2B前提と、以前の「レビューで整合確認済み」という記録を更新する。契約照合は行ったが、実機性能・安全な並行generation・日本語精度は本作業では未検証である。機能実装、設定有効化、外部送信は行わない。
