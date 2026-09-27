# Role Routing 受入仕様・検証マトリクス

状態: **受入未完了。実行証跡のないケースを合格扱いしない**。最新音声要件は[VC改訂のVT01〜VT11](saaa-role-routing-voice-integration.md)を必須追加とする。以下のA試験と競合する音声動作はVC改訂に従う。
[全体計画](saaa-role-routing-plan.md) / [実行契約](saaa-role-routing-execution-contract.md) / [学習契約](saaa-role-routing-learning-contract.md) / [作業カード](saaa-role-routing-work-cards.md)

## 1. 検証環境

- Rustは実際のin-memory SQLite migration、SqliteWriter、routing coordinatorを使用する。選択関数だけを呼ぶ試験で縦通しを代用しない。
- FakeClockはwall/monotonicを個別に進められる。FakeActor、FakeSpeech、FakeToolは開始・完了・失敗・停止確認をoneshot/barrierで制御する。
- policy fixture: frontend=F、reasoner=Q、advanced=S、reviewer=Q、premium=A。役割名は実装、文字はfixture actor ID。fixtureでは接続確認済みとする。モデルの実能力を示すものではない。
- 外部副作用はCounterToolとUnknownOutcomeToolで再現する。許可された実tool実行回数、root/step/message数、発話開始数を検査する。
- ネットワーク試験はloopback mock HTTP/SSEまたはmock SDK。通常suiteでは外部モデル・本番SQLite・個人Codexタスクへ接続しない。
- 各試験はroot ID/初期revision/actor呼出列/入力順序を明示する。時刻を待つsleepで競合順序を作らない。
- 列の「合格条件」はSQL件数、adapter call、event、speech等のassertに変換する。目視だけで合格にしない。

## 2. R1: 基礎と通常会話（A01〜A12）

| ID | 初期状態・入力・故障注入 | 合格条件 | カード |
| --- | --- | --- | --- |
| A01 | enabled=falseで通常のテキスト/音声入力 | 旧経路だけを1回呼ぶ。rr_root、rr_step作成0。既存fixture結果不変 | RR-03/14 |
| A02 | policy保存。未知role、未知action、cycle、location矛盾、変更前versionを指定 | 保存拒否、policyVersion不変、既存設定不変。正しい別actorへの割当は保存可能 | RR-03/15 |
| A03 | inputIdを同payloadで再送、別payloadで再送、同sourceIdのASR再送 | 同payloadは元receipt、message/root各1。同ID別payloadはconflict、副作用0 | RR-04 |
| A04 | 完成した依頼をFへ送信してthink=true、Qを保留、追加の同意を送る | Q task1、F会話session維持、追加発言にF応答、Q cancel0。受付をQ最終回答にしない | RR-05/09/13・VT02/03 |
| A05 | 「こんにちは」と「ありがとう、でも条件が違います」を入力 | 前者は許可された簡易応答で完了、後者を挨拶として完了させない | RR-08 |
| A06 | Fがtimeout/不正JSON、またはJSON Schema非対応。Qは成功 | 平文LFM＋Qwen補助判断へ降格、不正JSON発話0、同inputのtask/tool重複0。F失敗を既存Qの失敗にしない | RR-08/09・VT04/05 |
| A07 | 同resourceGroupと別resourceGroup、広告容量1/複数でF/Qを開始 | 実容量超過0。対応配置ではQ思考中もF応答。非対応配置はdegradedで音声gate不合格、定型文で成功を偽装しない | RR-09/13・VT03/05/10 |
| A08 | Fがtool要求、Qが未提示tool、reviewがmutationを要求 | 各要求を拒否し外部実行0。許可されたQのtoolは既存gateを通る | RR-10/11 |
| A09 | Qのtool成功後に通信切断。同じstepのretryを要求 | invocationは1件、mutation実行1回。通信失敗を理由に自動再実行しない | RR-11 |
| A10 | Qの結果を二重配送。さらにDB確定時に失敗を注入 | 通常はanswer/root完了各1。DB失敗時はanswer採用0、TTS開始0 | RR-12 |
| A11 | ack予約前/再生中/停止失敗の各時点でQ結果を配送 | 不要ack取消、同時speech<=1。停止未確認なら最終音声失敗、画面回答は保持 | RR-13 |
| A12 | mock音声入力→F→Q→tool→回答→TTS→UI再接続 | receipt時にASR queue解放、DBから確定回答復元、TTS failureでも回答完了を維持 | RR-14/15 |

### 音声稼働の追加ゲート

VT01〜VT11を通常の受付IPC・実writer・coordinator・speech ownerで検証する。特に「1.5秒無音≠依頼完了」「LFMは会話担当のまま」「Qwen回答で現在/待機/遅着LFM音声を破棄」を必須assertとする。liveは固定20ケース各3回と実マイク10往復以上、通常ビルド・policy・actor fingerprint・レイテンシを記録する。数値目標と失敗時の扱いはVC改訂§6を参照する。JSON構文だけ成功、単体Providerだけ成功、0 tests、未実行は合格ではない。

## 3. R2: 追加条件・再検討・委任（A13〜A30）

| ID | 初期状態・入力・故障注入 | 合格条件 | カード |
| --- | --- | --- | --- |
| A13 | Q思考中に「費用も比較して」。classifier保留中に旧Q結果到着 | 入力保存直後から採用barrier。旧結果は発話0、条件確定後revision+1 | RR-16 |
| A14 | Q思考中に「今どうなっていますか」 | root/revision維持、Q cancel0、実stateに合う受付。旧結果が保留中なら分類後採用可 | RR-16 |
| A15 | 複数の解釈が可能な追加発言、確認の返答も保留 | clarifyを返しpending barrier維持。未解決条件のまま回答を確定しない | RR-16/27 |
| A16 | update→resultとresult採用→updateをそれぞれ実行 | 前者は旧結果不採用。後者は旧回答を保持し追加条件を新rootへ。完成rootの再開0 | RR-17 |
| A17 | mutation実行permit発行前/後にrevision更新 | 前なら実行0。後なら実結果を保持してsettle待ち。unknownなら次mutation実行0 | RR-10/17 |
| A18 | Qとtoolが動作中に明示中止、cancel acknowledgment喪失 | 新tool開始0、子cancel伝播、unknown副作用は記録。停止要求だけで取消成功にしない | RR-17 |
| A19 | busy中に別依頼を4件、5件目、queued rootのcancel | FIFO、上限超過の入力保存0、queued cancelはProvider開始0。現rootは壊れない | RR-18 |
| A20 | Q結果前にUI切断、app再起動、再購読 | UI切断のみなら保存継続。再起動ならinterrupted、tool再実行/自動再生0 | RR-18 |
| A21 | Q回答への「そんな結論でよいんですか？」を明確にtarget指定 | 新rootでSへreconsider、旧回答を参照、Fは再確認案内、確定回答はSが作成 | RR-22/23 |
| A22 | S回答へのchallenge、Qによる根拠付きreview、Sによる修正 | S→Q→Sの順、reviewerとauthorが異なる、issueと修正根拠を記録、最終回答1件 | RR-24/25 |
| A23 | 「本当に？」、引用中の不満、別conversationのtarget | ambiguous/clarifyまたはreject。自動低評価label0、不正targetへの再考0 | RR-23 |
| A24 | Qレビューが存在しない根拠、未検証issue、根拠のある反論を返す | 虚偽ref拒否、未検証は仮説として扱い、根拠なしの結論反転0 | RR-24/25 |
| A25 | Q/Sが互いに委任を繰り返す、review rounds上限、root期限 | 設定上限で停止してunresolvedを説明。最大step/call/deadline超過の開始0 | RR-22/25 |
| A26 | S再考後に問題未解消、A利用を提案 | proposalをDBへ保存、A呼出0。費用不明はunknown、未解決点を表示 | RR-26 |
| A27 | 有効proposalに承諾、曖昧な「はい」、期限切れ、revision変更後の承諾 | 有効な明示承諾だけAを1回起動。その他は起動0、二重承諾も起動増加0 | RR-26/27 |
| A28 | local-only中、またはdecision後dispatch前にcloudを禁止 | S/A起動0。許可されたQへの再考か制約説明。ranker/承諾で上書きしない | RR-06/22/26 |
| A29 | SDK started→agent message→EOF、wrong step ID、oversize、cancel | turn.completed+schema妥当の場合だけ成功。違反はfailed、root回答なし、子ツリーを回収 | RR-19/20 |
| A30 | Sがhost tool要求→結果→回答。revision/modelを変更して再実行 | toolは既存gatewayを通り、別revision/modelは新SDK thread。既存個人thread操作0 | RR-21/29 |

## 4. R3: 夜間処理と学習（A31〜A42）

| ID | 初期状態・入力・故障注入 | 合格条件 | カード |
| --- | --- | --- | --- |
| A31 | decision後にfeedback/world状態を追加 | 保存済みfeatures不変。後の反応はlabelsだけに入る。decision時点の統計を再現 | RR-30 |
| A32 | 100件pageの書込前/後に停止し再起動、同page再実行 | checkpointと例が同時commit。欠落/重複0、保存失敗でcursorを進めない | RR-31 |
| A33 | 前日の回答への明示評価が翌日到着 | dirty root再処理、labelRevision増加、旧dataset不変、新datasetに最新評価 | RR-31/32 |
| A34 | 無反応、中止、条件更新、HTTP成功、推定レビュー、明示評価を混在 | 未知はnull、推定は別source、矛盾は除外。全件をsuccess/failure二値にしない | RR-32 |
| A35 | 同会話・同root lineage・同fixture派生が日をまたぐ | train/validation/testのgroup重複0、未来情報0、件数不足は昇格不可 | RR-33 |
| A36 | export途中に終了、manifestだけ存在、hash不一致 | ready扱い0、.tmpを読まない、再開で冪等に確定。個人本文export0 | RR-33 |
| A37 | 夜間sleep、翌日idle、時計巻戻り/DST、foreground開始 | 同job二重作成0、missed job再開、foregroundでpause、会話優先 | RR-34 |
| A38 | 19/20件のeligible例、cost欠損、新actor版 | 少数はrules、20件以上は所定統計、欠損を0円にしない、新版cold start | RR-35/36 |
| A39 | shadow有効化、ranker不正値/未知ID/無効hash | real Provider起動数不変。不正rankerはrulesへ。shadow一致率を品質改善と表示しない | RR-35/36 |
| A40 | 学習後に元会話削除、scope撤回、forget、filesystem削除失敗 | DB内artifact即時失効、次dispatchで使用0、file削除は再試行、元本文を復活させない | RR-37 |
| A41 | mock specialistへtool引数生成を委任。無効化/失敗を注入 | 同じpermit/ACL/ledger、specialistが最終回答を上書きしない、無効時は直接Q | RR-38 |
| A42 | 記録済みtool実行をoffline比較し、未選択候補のlabelを要求 | 副作用再実行0、未選択結果を捏造しない。unsupported/insufficient_dataを報告 | RR-36/39 |

## 5. 競合試験の順序

少なくとも次の順序をそれぞれ独立したtestとして作る。

1. 入力受付commit → classifier未完 → Provider結果 → classifierがamendmentを確定。
2. 最終回答採用commit → 次入力受付commit → 前回答のspeech開始要求。
3. tool permit reserve → input update → invoke前再検査。
4. tool dispatch commit → input update → remote成功 → 新revision。
5. cancel commit → Provider完了、Provider完了commit → cancel受付。
6. final speech開始 → ユーザー割り込み → SpeechEnded遅延 → 別root speech開始。
7. subscription登録 → replay上限取得 → live event → replay終了。
8. dataset file rename → DB ready前に終了、DB invalidated → artifact cache採点要求。

rootとspeechのrevisionだけでなく、eventSeq、tool operationKey、artifact generationをassertする。乱数によるストレス試験は上記の決定的試験の代用にしない。

## 6. 日本語反応fixture

`tests/fixtures/role-routing/reactions-v1.json`をRR-08で追加する。1件はtext、前の回答、active state、期待kindの集合、禁止Action、targetAnswerId。単一の正解に定められない発言はallowed kindsを複数にし、危険なActionを禁止する。

| 入力例 | 期待・許容 | 禁止 |
| --- | --- | --- |
| 「その結論、本当に条件を全部満たしていますか？」 | answer_challenge | socialとして完了 |
| 「本当に？」 | unclear/explanation_request | low-quality確定、無条件premium実行 |
| 「説明が短すぎるので根拠を教えて」 | explanation_request | 誤答確定 |
| 「Solでもう一度検討してください」 | explicit_actor_request | モデル名を無視してfrontで完了 |
| 「『本当にそれでいい？』と言われたときの返答を書いて」 | new_task | 過去回答へのnegative feedback |
| 「結論は問題ありません。説明だけ短くして」 | new_task/explanation_request | answer_challenge確定 |
| 「ありがとう、ただ予算は10万円です」 | add_constraint（activeあり） | 挨拶のみで完了 |
| 「中止して」 | cancel（対象一意） | 別rootも一括cancel |
| 「中止してと言われた場合の動作は？」 | new_task | cancel |
| 「はい」 | unclearまたは通常会話 | premium承諾 |
| 「はい、Astraでお願いします」 | 有効な一意proposalへのapprove | 失効proposalの実行 |

live分類評価は最低100件、上記の言い換え・引用・否定・対象曖昧を含む。原案fixtureと調整用/固定評価用を分ける。明示cancel/承諾の誤実行0件、実質依頼のfrontend誤完了0件を必須とし、分類一致率とclarify率を併記する。低いclarify率を目的に閾値を緩めない。

## 7. 性能・品質gate

以下は初期の受入目標値。実機未測定。固定Qwen baselineと同じ入力・同じモデルpool・同じ温度・同じ音声設定で比較し、CPU/GPU機種、同居モデル、warm/coldを記録する。

| ID | 指標 | 方法 | 目標と失敗時の処置 |
| --- | --- | --- | --- |
| P1 | host routing/DB受付の追加処理 | warm5+100件、LLM/TTS待ちを除外、mock adapter | p95<=50ms。超過ならquery/index/pageを修正し先に進まない |
| P2 | LFM応答準備・受付発話開始TTFA | 音声依頼30件以上、ASR finalから応答準備と実音声再生開始を別計測 | VC初期目標: 温間応答準備p95<=2000ms、可聴開始p95<=3000ms。cold別報告。旧定型受付の1500ms目標を置換した設計値であり実測結果ではない。定型文で並走不能を隠さず未達として報告 |
| P3 | 最終回答開始までの遅延 | 通常依頼30件以上、baselineと交互に実行 | 同actor経路の追加p95<=max(1000ms,baseline p95の10%)。bufferingの影響も含める。未達なら体験gate未合格 |
| P4 | 回答品質と継続性 | 固定日本語タスク100件、blind rubric、条件一致/根拠/課題達成 | 明示条件の欠落・二重tool実行・禁止送信0。baselineとの成功率差と区間を報告し、差の下限<-2ptなら導入保留 |
| P5 | 夜間負荷 | page100件、foregroundを途中開始、実機30件 | DB write p95<=50ms、cancel要求<=100ms（仮想時計）、TTFA p95悪化<=10%。超過時はnightly pause |

R1はP1〜P3、R2はP1〜P4、R3は全てを報告する。P4のサンプルが足りず信頼区間が広い場合、品質維持を断定しない。最終回答全文をbufferする初期設計は遅延が増える可能性があり、P3未達を相槌の速さで隠さない。ストリーミング採用への変更は部分発話の訂正契約を別途追加してから行う。

## 8. 要求から証跡への対応

| ユーザー要求 | 契約 | カード | 試験 |
| --- | --- | --- | --- |
| 会話1.2BとQwen推論の分担 | C0/C8/C9 | RR-05〜15 | A04〜A12 |
| 思考中も会話を維持 | C5/C7 | RR-16〜18/27 | A13〜A20 |
| Solへ委任 | C3/C8 | RR-19〜22 | A21/A28〜A30 |
| 不満に応じた再思考・他モデル評価 | C3/C10 | RR-23〜25 | A21〜A25 |
| Astraの利用提案 | C10 | RR-26/27 | A26〜A28 |
| 担当/手法を柔軟に変更 | C2/C3.3 | RR-03/06/35/36 | A02/A38/A39 |
| ツール専門モデルの検証 | C8.4 | RR-38 | A41 |
| SQLiteから夜間学習 | L1〜L8 | RR-30〜37 | A31〜A40/A42 |
| 会話・メモリー・worldを基盤にする | C4/C5 | RR-07/17/37 | A13/A16/A17/A31/A40 |

## 9. 証跡の形式と完了判定

`spec/evidence/role-routing/progress.md`はカードID、HEAD、変更ファイル、test名、command、実行件数、合否、未実施理由を持つ。R1/R2/R3報告はA/Pの各IDを列挙し、offline/live/未検証を明示する。

完了報告には設定例、利用可能actor、unsupported actor、既知の遅延、rollback手順を添える。文書だけ整っても実装完了ではない。nightly datasetが作れてもML精度が確認されたとは表現しない。

本文書を含む計画一式の整合性確認は、local linkの存在、RR-00〜39の重複/欠落、A01〜42とP1〜P5の重複/欠落、JSON例のparse、参照コードの存在、git diff --checkで行う。ランタイム試験は実装時に作業カードのV1〜V6で実行する。
