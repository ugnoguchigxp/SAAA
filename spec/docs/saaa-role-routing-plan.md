# SAAA Role Routing 実装計画 v2

更新日: 2026-09-20。状態: **設計のみ。機能は未実装**。
初回調査基準: `f9c7f07`。Git操作後の再確認基準: `c0eda5e`（D5のwriter factory・shutdown整理を含む）。実装開始時のHEADが異なる場合はRR-00で差分を記録する。

## 1. 実装する製品動作

会話の窓口を維持したまま、通常推論、難課題の委任、回答への異議に対する再検討、別モデルによる評価を行う。LLMを替えても、目的、条件、実行済みツール、発話順序をSAAAが管理する。初期構成は会話1.2B、通常推論Qwen 27B、上位推論Sol、提案対象Astra。これらは設定上の割当であり、モデル名で分岐するコードを書かない。

旧版の「3回」は実装単位として大きすぎた。**3マイルストーン、40枚の作業カード**へ分割する。1カードを実装・検証してから次へ進む。3回の短い作業で完成するという見積もりではない。

## 2. 文書の読み順と正本

| 順 | 文書 | 用途 |
| --- | --- | --- |
| 1 | [本計画](saaa-role-routing-plan.md) | 範囲、既存コード、段階ゲート |
| 2 | [実行契約](saaa-role-routing-execution-contract.md) | 型、設定、状態遷移、永続化、アダプター、発話 |
| 3 | [学習契約](saaa-role-routing-learning-contract.md) | 特徴、ラベル、夜間処理、評価、適用 |
| 4 | [作業カード](saaa-role-routing-work-cards.md) | 変更ファイル、実装手順、依存、カード別試験 |
| 5 | [受入仕様](saaa-role-routing-acceptance.md) | 入力と期待動作、競合試験、製品ゲート |

具体的な契約は実行契約C節と学習契約L節を正本とする。カードにない機能を推測で足さない。数値は初期製品設定の設計値であり、実測値ではない。実機のモデルID、接続能力、利用可能性はRR-00の証跡に記録する。未確認のモデルを利用可能として登録しない。

## 3. 既存コードとの接続表

| 現行箇所 | 確認できた動作・制約 | 本計画の扱い |
| --- | --- | --- |
| `src-tauri/src/runtime/start_turn.rs` | 1入力につきrunとTTSを開始し、終了までawait | 既存経路維持。新Routingは専用の非同期受付IPCを追加 |
| `src/features/chat/useConversationTurn.ts` | 通常はrun中の新入力を拒否。一部音声推論はcancelして置換 | Routing有効時は専用hookへ分岐し、追加条件を専用IPCへ送る |
| `src/lib/conversationSession.ts` | 同時に1つのrun、1つのspeechだけを保持 | 既存のまま。新Routingの画面状態はDB snapshotの投影にする |
| `src-tauri/src/runtime/turns.rs` | 入力保存、scope/broker、provider選択、終了処理を所有 | context組立の共通部分だけ抽出。新rootから再帰的にexecute_turnを呼ばない |
| `src-tauri/src/runtime/conversation_controller/mod.rs` | reasoning-mcpと相槌を並行。local-only、coding不可 | 旧経路として維持。新Qwen経路はtools対応のchat-completionsを利用 |
| `src-tauri/src/providers/chat_completions/mod.rs` | ツールloop、提供済みツール検査、最大32 calls | 新しいrole制約・実行permitを接続。既存callerの挙動は維持 |
| `src-tauri/src/providers/session_store.rs` | 回答保存とruntime_runs完了を同一transactionで行う | transaction内helperを抽出し、新Routingのrevision照合と同時に確定 |
| `src-tauri/src/runtime/event_hub.rs` | Deltaは即座にTTSへ届く | 新Routingの未確定Deltaは流さない。確定回答のみ発話ownerへ |
| `src-tauri/src/runtime/reasoning_ack.rs` | 相槌deadlineと親cancelを扱う | 境界処理を参考に新発話ownerで統一。二重に起動しない |
| `src-tauri/src/runtime/codex_process.rs` | app-server、read-only、workspace必須 | SDKアダプターと混同しない。旧codingモードは変更しない |
| `scripts/pi-codex-sdk/index.ts` / `coding/settings.rs` | SDK利用あり。ただしLuna固定のpiプロファイル | Solへ固定値を置換しない。独立したRouting用SDK sidecarを追加 |
| `tool_selection/gateway.rs` / `invocation.rs` | 検索・説明・実行、caller切断後も実行管理を継続 | Routingのツール実行もこのownerへ集約 |
| `tool_selection/mcp_server/` | 外部MCPごとに専用conversation/runを発行 | Routing rootへの紐付けがないため、既定ではこの外部経路を流用しない |
| `runtime/context/` / `memory/personal_state/` | scope、生成来歴、削除・失効制御 | 毎stepで再検証。新しい独立メモリーを作らない |
| `world/runtime_frame.rs` | WorldFrameは期限付きの観測。権限や実行状態の正本ではない | ランカーの任意特徴に使用。欠落時はunknown |
| `memory/personal_state/scheduler.rs` | foregroundとbackgroundが共有slotを使う | 1.2BとQwenを同じ排他slotに入れない。resourceGroup単位で調整 |
| `persistence/sqlite/writer.rs` | DB書き込みownerが1つ | 全追加tableも同じwriter。nightly用の第2writer禁止 |
| `tool_selection/feedback.rs` / `ranking.rs` | 発言根拠、曖昧性、冪等性、ルール補正 | 型・設計を参考にする。ツールランキングのモデルをそのままRoutingへ転用しない |

表の省略pathはすべて`src-tauri/src/`からの相対path。

## 4. 固定する設計判断

1. 会話ごとに1つのcoordinator。永続状態はSQLite、actorはIOの所有者。UIは状態の正本にしない。
2. 1つの目的をroot run、推論・評価・再考をstepとして表現する。相槌・追加条件は別の独立runにしない。
3. 実行中入力はrootを更新する専用受付へ送る。最初の実装は「中断して新revisionで再実行」であり、Providerの内部思考へ直接追記する方式は使わない。
4. ツールの副作用は実行ownerが確定するまで保持。再考で二重実行しない。曖昧なremote outcomeはunknownとして停止する。
5. 1.2Bは簡易応答、反応の分類候補、相槌候補を返す。モデルが権限や完了を確定することはない。
6. 最終回答の内容は思考担当が所有し、共通TTSがそのまま読む。初期版で1.2Bに最終回答の自由な再要約をさせない。
7. Qwen→Sol、Sol→Qwen評価→Sol修正、Astra提案を定型の有限recipeとして設定する。任意コードや循環graphを設定から実行しない。
8. 通常のSol委任は設定で許可された範囲で自動実行。Astraは初期値では提案後の明示承諾が必要。クラウド禁止は承諾でも迂回しない。
9. 夜間の学習データ生成はR3に含める。訓練済みモデルの本番自動適用は含めない。R3でルール・集計ranker・学習artifactの同一境界を完成させる。
10. 設定のenabled既定値はfalse。無効時は旧start_turnのまま。新routing失敗後の旧経路への再実行は副作用重複を招くため行わない。

## 5. マイルストーンと依存

| 段階 | カード | 利用者が確認できる完成状態 | 次へ進むゲート |
| --- | --- | --- | --- |
| R1 | RR-00〜RR-15 | 1.2Bの受付＋Qwenの推論・ツール＋共通音声。設定と永続記録が有効 | A01〜A12、旧経路の回帰0、必要なlive接続確認 |
| R2 | RR-16〜RR-29 | 思考中の追加条件、Sol委任、独立評価、修正、Astra提案・承諾 | A13〜A30、全競合試験、SDK adapter gate |
| R3 | RR-30〜RR-39 | 夜間データ生成、バックグラウンド集計、shadow ranker、削除連携、実機評価 | A31〜A42、再開・リーク・削除・性能証跡 |

RR-00はモデル情報不足でもoffline設計・mock試験を進められる。必要なlive gateを未実施のまま段階完了とはしない。Needle3の実モデル評価と新しいML訓練は後続の追加作業。差し替え境界とmockによるtool-specialist recipe試験はR3内に含む。

## 6. 変更範囲とロールバック

主な新規実装は`src-tauri/src/role_routing/`、`scripts/role-routing/`、専用IPC型とUI hook。既存ファイルへの変更は登録、入力分岐、context helper、tool permit、transaction helper、削除hook、設定UIに限定する。WorldFrameやツールACLそのものの意味を変更しない。

DBは現行schema version 25から加算migration。実装開始時に番号が進んでいれば現行+1とし、既存migrationを変更しない。featureをoffにするロールバックは可能。DBのuser_versionを下げるdowngradeはしない。

無効化時は新規root受付停止→進行中stepのcancel→副作用の確定待ち→root終了→旧経路再開。外部処理の停止未確認時は同じ作業の再実行を保留する。学習artifactだけのrollbackはルールrankerへの切替で行い、routing本体を停止しない。

## 7. 実装担当の実行手順

1. RR-00で基準を記録し、その後はカード順に実装する。
2. 各カードのC/L節を読む。指定したfixtureと失敗条件を先に用意する。
3. 1カードは実装1〜3ファイル＋試験1〜2ファイルが目安。登録・生成物以外で5ファイルを超える場合、a/bの枝番に分ける。
4. 対象試験の実行件数と結果を`spec/evidence/role-routing/progress.md`に記録する。0件通過は禁止。
5. milestone gateを満たすまでenabledを製品既定値にしない。カードごとのユーザー承認やcommitは不要。
6. 全段階完了時に実装、offline合格、live合格、未検証を分けて報告する。

## 8. 外部仕様と確認範囲

[公式Codex SDK資料](https://learn.chatgpt.com/docs/codex-sdk)はTypeScript SDKでthreadの開始・継続・再開ができることを説明する。実装のAPI型は依存固定版`@openai/codex-sdk` 0.144.4の`dist/index.d.ts`を基準にする。現行型で`runStreamed`、`outputSchema`、`signal`、`skipGitRepoCheck`を確認済み。モデルのアカウント別利用可否までは確認していない。

[公式設定資料](https://developers.openai.com/ja-JP/docs/config-file/config-basic)のfeature flagも参考にするが、固定CLI版でのnative tools隔離はRR-20の実行ゲートで確認する。設定キーが受理されるだけで隔離成功としない。

## 9. 今回の補完と禁止事項

Git操作後、概要と詳細契約・学習契約・40カードは作業ツリーに残っていた。未作成だった受入仕様を補い、入力分類中の採用barrier、amendment/cancelで異なるdrain、event購読のreplay競合、設定rollbackとdigest、feedbackとdecisionの紐付けを明文化した。旧142行から概要が短くなったのは詳細を別文書へ分けたためで、機能要求の削除ではない。

禁止事項: Git reset/restoreで他の変更を戻すこと、既存の別Codexタスクへの送信、未知のモデルIDやSDK機能の推定実装、rankerからの権限昇格、旧モデルの内部思考のコピー、学習完了だけで本番方針を更新すること。新規runtime依存の追加は初期実装では行わず、既存Rust/Bun/SDKを使う。

要相談となるのは、許可済み候補では要件を達成できず外部送信範囲・費用上限・製品動作を拡大する必要がある場合。通常のカード実装、可逆な文書修正、mock検証に都度承認は求めない。固定CLIのSDK隔離gateが通らない場合はunsupportedとして報告し、黙示に実行範囲を拡大しない。
