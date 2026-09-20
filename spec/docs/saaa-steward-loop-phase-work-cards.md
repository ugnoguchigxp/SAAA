# SAAA 執事循環フェーズ 作業カード

作成日: 2026-09-20。状態: 実装順のカード。正本は[フェーズ計画](saaa-steward-loop-phase-plan.md)。

Step 1 のカードは既存の[M3Aカード](saaa-personal-world-model-m3-work-cards.md)を使う。ここでは重複して書かない。Step 2 のカードは M3-21 後に別文書へ最大10枚。Step 4 のカードは[最小循環計画](saaa-minimal-loop-plan.md)。本書は横断ゲートと Step 3 のみ。

## 実行規則

一枚を完了してから次へ。並行着手しない。5実装ファイルを超える場合は枝番へ分割する。カード単位の commit は不要。試験名は `sl_` / `st_` 接頭辞。0件実行を合格にしない。

## 横断カード

| ID | 対象 | いつ | 実装すること | 合格条件 |
| --- | --- | --- | --- | --- |
| SL-00 | spec/evidence/steward-loop/progress.md | フェーズ開始 | HEAD、dirty差分、Role Routing 未コミットを凍結対象として記録。M2結果と WorldFrame 未配線を転記 | 巻き戻し0。並行差分を本フェーズの欠陥としない |
| SL-01 | M3-00〜21 | Step 1 | 既存M3カードを順に完走 | m3-results.md に全カード証拠。本表へ再掲しない |
| SL-02 | M3B計画（未作成） | Step 1 完了後 | m3-results.md §8 に従い計画1文書・カード10枚以内を書く。対象は会議・進行中Task・明示Projectのみ | グラフ深化なし。TTL延長なし。default OFF |
| SL-03 | M3Bカード | Step 2 | SL-02 のカードを順に完走 | 誤断定増0、Scope漏洩0、命令位置1。未達なら shadow 残留と報告 |
| SL-04 | ST-00〜07 | Step 3 | 下表 | 会議中TTS 0、hysteresis後復帰 |
| SL-05 | ML-00〜13 | Step 4 | 最小循環計画のカード | 受入シナリオ実DB。漏洩0、重複0、撤回後新規実行0 |
| SL-06 | bun run check:local / spec:check | 各Step完了時 | 全体ゲート。size baseline登録。IPC変更時は ipc:generate | 機能カード通過だけでStep完了としない |

## Step 3 カード（Situation TTS 一点強制）

契約はフェーズ計画 §7。分類器・シグナル・hysteresis・Meeting開始は変更禁止。

| ID | 対象 | 実装すること | 合格条件 |
| --- | --- | --- | --- |
| ST-00 | spec/evidence/situation/tts-gate-progress.md | Situation 試験の現行件数、ShadowDecision の runtime 非参照、voice_response の既存ゲートを記録 | baseline と今回変更を分離 |
| ST-01 | situation の読取口（新規小関数可） | 最新の scene と proposed_attention を runtime が読める内部API。serde 公開・IPC 追加なし | 分類を再実行しない。ledger 本文を返さない |
| ST-02 | runtime 側 policy | MEETING かつ IGNORE/OBSERVE なら canSpeak=false。RESPOND（明示入力）は抑止しない | 条件外（FOCUS のみ、UNKNOWN）で誤抑止0 |
| ST-03 | voice_response.rs | start / complete / 途中 speak の入口で ST-02 を強制。voice_behavior の speak 判定より先に Situation を見る | TTS spawn 0。テキスト turn は継続 |
| ST-04 | 監査 | 抑止時に本文なしの監査イベント（reason、scene、attention、run_id）。7日保持の既存監査へ | 会議名、発話本文、transcript 0 |
| ST-05 | situation または runtime 実DB試験 | 合成 MEETING + OBSERVE で TTS ハンドルなし。hysteresis 経過後の非MEETING で start が再び Some | 実DB。clock は試験注入。分類器定数変更0 |
| ST-06 | 境界試験 | 自動通知、Meeting start、アプリ操作、Situation mode の shadow→enforce 昇格が無いこと | 新IPC 0、actual_execution は NONE のまま |
| ST-07 | size / check:local / spec:check | 追加ファイルを size baseline へ。閾値を緩めない | 対象 suite 件数>0。results.md に offline合格と live未検証を分ける |

ST-03 の実装漏れとして残してよいもの: streaming TTS の途中チャンクが既に出ている run の打ち切り。本Stepは開始抑止が完了条件。進行中発話の即時 mute は deferred.md へ一行。
