# SAAA 執事循環フェーズ — 実装計画

作成日: 2026-09-20。状態: 次フェーズの優先順位と実行契約。コードは本計画の作成では変更しない。

上位は[Personal AI Concept](saaa-personal-ai-concept.md) §4・§5.1・§16、段階gateは[Personal Stateロードマップ](personal-state-architecture-roadmap.md) §11。Worldの接続は既存[M3A計画](saaa-personal-world-model-m3-plan.md)を正本とする。最小循環の詳細は[最小執事循環計画](saaa-minimal-loop-plan.md)。作業カードは[本フェーズ作業カード](saaa-steward-loop-phase-work-cards.md)。

## 1. いま成立していないこと

基盤（Memory、Personal State、World Model M2 Frame、Tool Selection D0–D5、L-Lang実行ゲート）は厚い。利用者が体験できる執事的な瞬間はまだ一つもない。原因は基盤の横幅であり、次の縦一本が会話へ届いていない。

```text
現在の世界が判断に効く
        ↓
状況に応じて黙る／話す
        ↓
任された一件の仕事が続き、止めれば止まる
```

確認した未配線（計画作成時点）:

| 部品 | 現状 | 本フェーズでの扱い |
| --- | --- | --- |
| `WorldFrameService` | `runtime_frame.rs` に存在し `#![allow(dead_code)]`。Broker / turns / IPC から未呼び出し（M2結果も同旨） | Step 1 で shadow Candidate として compose へ供給 |
| Context Broker | Personal State 候補を compose。World 専用 source なし | Step 1 で `world-model-shadow`、Step 2 で通常 generation へ限定有効化 |
| `ShadowDecision` | Situation tick 内で更新。runtime は会話状態の書込みだけ参照 | Step 3 で TTS 抑止の一点だけ強制。分類器は変更しない |
| TTS | `voice_response.rs` は voice origin と `voice_behavior` のみ。Situation の `canSpeak` なし | MEETING かつ attention が IGNORE/OBSERVE のとき開始を抑止 |
| Goal / Delegation | World の Goal ノードはグラフ要素。ユーザー登録の Intent 正本はない | Step 4 で一件の明示 Goal と委任を新表へ。五要素推定は増やさない |
| 実行 | `runtime_runs` と `coding_jobs` / `coding_runs` が既にある。再起動時は `coding/recovery.rs` が未解決を再実行しない | 新しい実行台帳は作らない。Task 識別と委任状態だけを載せる |

成功の定義は行数や試験数ではない。開発者本人が1日使って次を体験できること。

1. 会議中は黙り、会議終了後（hysteresis経過後）に話す。
2. 「今何を進めている？」に WorldFrame 由来の根拠付きで答え、期限切れなら分からないと言う。
3. 一件のテスト監視を任せ、別の話をして、戻ると結果があり、止めれば止まる。

## 2. 優先順位

並行着手しない。一つを完了してから次へ進む。数値が足りなければ有効化せず、shadow のまま報告する。

| 順位 | Step | ロードマップ対応 | 到達点 | 着手条件 |
| --- | --- | --- | --- | --- |
| P0 | 1 M3A | P1.5の後段、P2の供給路 | WorldFrame が Broker compose に Candidate として届き、採否・省略理由・予算が shadow 記録される | 既存 M3-00。カード外の機能を足さない |
| P1 | 2 M3B | P2 のうち「会議・進行中Task・明示Project」だけ | 上記3状態に限り通常 generation へ default OFF で投入。誤断定0、Scope漏洩0、現在入力の命令位置1 | M3-21 の `m3-results.md` §8。別計画1文書・カード10枚以内 |
| P2 | 3 Situation TTS | plan.html §16 の `canSpeak=false` の最初の実体 | 会議中に音声応答が出ず、終了後に復帰する。自動通知・自動Meeting・アプリ操作は禁止 | Step 2 完了、または Step 2 が shadow 残留でも TTS 点は独立に完了可（World 有効化を待たない） |
| P3 | 4 最小循環 | Concept §16 と P1.5 統合受入シナリオ | Goal登録→話題切替→完了通知→委任撤回→再起動→再開または撤回報告 | Step 3 完了。計画は[最小循環](saaa-minimal-loop-plan.md) |

P2 を P1 より後にした理由: 「今何を進めている」が先に会話へ届かないと、黙る／話すの体験が空になる。ただし TTS ゲートは World 候補を必要としない。P1 が数値ゲートで止まった場合は P2 を先に完了して「会議中は黙る」だけを成立させてよい。その判断は M3A 結果の報告時に記録する。通常は表の順を守る。

P3 を最後にする理由: 委任仕事の通知も会議中は保留するため、Situation の一点強制がないと受入シナリオの「通知」が音声で割り込む。

## 3. 凍結（着手禁止）

文書は残す。実装・新計画・新カードを足さない。思いついた項目は `spec/evidence/steward-loop/deferred.md` に一行書いて止まる。

- Role Routing（`spec/docs/saaa-role-routing-*.md`）。`src-tauri/src/role_routing/` を作らない。未コミットの Role Routing 差分も本フェーズでは進めない。
- World 五要素の新機能（Goal 推定、相関、条件、confidence、ResearchGap）。M3 が使うのは既存 M2 Frame のみ。
- L-Lang の「LLMがコードを生成して配備」、permissions 非空の許可モデル。
- Tool Selection D6、MCP の新 transport / OAuth / resources。
- 新しい会話 Runtime 経路、第2 SqliteWriter、独立 daemon、TypeScript 永続化、新規 runtime 依存。
- 定期ポーリング、自動コード修正、自動Meeting開始、アプリ操作、会議の自動文字起こし開始。

Step 4 の SQLite 表追加は「新しいDB正本を増やす」禁止の例外ではない。既存 schema version（計画作成時点 25）から加算し、既存 migration は変更しない。正本は一つ、Writer は一つ、会話経路は分岐しない。

## 4. 全Stepの不変条件

Concept §5.1 と指示書をそのまま守る。

- Memory / World / Situation / Task は共通 Turn Orchestrator と Context Broker へ Context Source または Reducer として接続する。会話 Runtime を分岐しない。
- DB 書込みは既存 `SqliteWriter` 一つ。
- Tool 出力・Memory・World・Situation は untrusted data。これらから権限・完了・ユーザー指示を導出しない。
- モデル出力（confidence、完了申告）を客観値として保存しない。
- 訂正・撤回・忘却は履歴追加。上書きしない。忘却は既存 forget journal。
- Feature default は OFF。有効化は既存 Memory 設定配下（`SAAA_MEMORY_ENABLED` と同一制御面。World 投入用の別 Runtime flag は作らない）。
- 数値ゲート（p95、0件条件）を結果を見て緩めない。満たせなければ shadow のまま報告する。

開発コマンドはリポジトリ直下 README.ja.md の「開発と検証」に従う。Rust IPC 型を変えたら `bun run ipc:generate`。新規ファイルは `bun run size:check` の baseline に登録し、閾値を緩めない。カード完了時は対象 suite の実行件数を `spec/evidence/<area>/progress.md` に書く。0件通過は禁止。Step 完了時に `bun run check:local` と `bun run spec:check` を通す。

## 5. Step 1 — M3A（既存カードを完走）

正本は既存の3点。本計画は順序と完了報告だけを固定する。

- [M3A計画](saaa-personal-world-model-m3-plan.md)
- [M3A契約](saaa-personal-world-model-m3-execution-contract.md)
- [M3Aカード M3-00〜21](saaa-personal-world-model-m3-work-cards.md)

到達点: `WorldFrameService` が `runtime/context/broker.rs` の compose へ Candidate として届く。通常 turns の候補配列へは足さない。shadow の selected が generation へ流入したら記録・dispatch 前に拒否する。

完了: `spec/evidence/world-model/m3-results.md` に全カードの証拠、p95、未接続、A〜L。生成品質や製品接続を完了扱いしない。

既知の実装漏れ（許容し、カード外で埋めない）:

- M2B（会話外 Source の永続更新）
- 通常 Provider への投入は M3B で完了。agent_session / 音声 / Codex 本文と live 正答は未接続
- Frame TTL 1,000 ms と生成時間の差。M3A で TTL を延ばさない
- `turns.rs` からの直接呼び出し

## 6. Step 2 — M3B（通常 generation への限定投入）

正本は [M3B計画](saaa-personal-world-model-m3b-plan.md)。実装完了記録は `spec/evidence/world-model/m3b-results.md`。live 回答品質は完了条件にしない。

## 7. Step 3 — Situation を判断へ届ける

正本は [TTSゲート計画](saaa-situation-tts-gate-plan.md)（契約 B0–B7 とカード ST-00〜07）。先回りで分類器や Meeting 所有者を変えない。実装はカード順。

追加は一点だけ。Interaction Policy の最初の実体。条件は `MEETING` かつ `proposed_attention` が `IGNORE` または `OBSERVE`。抑止は runtime の TTS **開始**（`voice_response` と `streaming_tts.begin` の両方）。`mode=shadow` のまま。`meeting.blocks_tts()` は別政策として残す。

完了: 正本 §6 のゲートと `spec/evidence/situation/tts-gate-results.md`。

## 8. Step 4 — 最小の執事循環

詳細は[最小循環計画](saaa-minimal-loop-plan.md)。ここでは優先理由と相談点だけを固定する。

採用する例（Concept §16）: 指定 Git ワークスペースのテスト失敗を検知し、許可された範囲で原因を調査し、結果を報告する。自動修正は入れない。

固定点:

- Goal はユーザーが明示登録した一件。由来・状態（active / paused / withdrawn）・達成条件・所属 Scope。
- Delegation は対象 workspace、許可操作（読み取り・テスト実行のみ、L2相当）、回数・時間予算、通知条件。撤回は即時。
- 実行先は既存 coding ジョブと Codex read-only sandbox。新しい実行台帳は作らない。
- 観測は明示トリガ「テストを確認して」と、既存 Situation の foreground=Coding への遷移のみ。定期ポーリングなし。
- 再開時、実施済みか不明な操作は再実行せず状態照会する（既存 `coding/recovery.rs` の outcome_unknown 方針を再利用）。
- 報告は完了・失敗・停止のいずれでも1メッセージ。会議中は保留。
- 同じ失敗から複数 Task を作らない。

相談が必要な場合（それ以外は止まらず進める）:

- 既存契約（Scope、忘却、single writer）と矛盾しないと実装できないとき。
- 外部送信範囲、費用上限、自動実行範囲を広げる必要が出たとき。
- 受入シナリオを既存 coding ジョブで満たせないと判明したとき。代替案を2つ添える。想定される論点は、coding_jobs が `conversation_id` / ユーザーメッセージ `source_id` に結び付いていること、ジョブ状態語彙が指示書の queued / awaiting_user / done と一致しないこと。写像表は最小循環計画 §5。新しい実行エンジンは代替案にしない。

## 9. 検証と報告

各カード: 対象 suite を実行し、件数と結果を evidence に書く。

| Step | progress | results |
| --- | --- | --- |
| 1 | `spec/evidence/world-model/m3-progress.md` | `spec/evidence/world-model/m3-results.md` |
| 2 | `spec/evidence/world-model/m3b-progress.md` | `spec/evidence/world-model/m3b-results.md` |
| 3 | `spec/evidence/situation/tts-gate-progress.md` | `spec/evidence/situation/tts-gate-results.md` |
| 4 | `spec/evidence/steward-loop/progress.md` | `spec/evidence/steward-loop/results.md` |

Step 完了報告は「実装済み / offline合格 / live未検証 / 未着手」を分ける。カード単位のユーザー承認と commit は不要。

## 10. 実装漏れとして残してよいもの

本フェーズの成功条件に含めない。後続へ引き渡す。

- Role Routing、D6、L-Lang 動的配備、MCP OAuth
- M2B、M3C（自然文からの候補抽出）、M4 品質評価
- Situation の分類精度改善、自動通知、複数 Goal、コード修正の委任
- World のグラフ探索深化、confidence の永続化
- 音声 live・実会議・実 Provider での品質。offline 実DB試験と live を混同しない
- 並行作業中の Role Routing / Tool Selection 未コミット差分の完成

## 11. 完了判定

フェーズ完了は次の3体験が開発環境で再現できること。基盤の増加は判定に使わない。

| 体験 | 必須 Step | 欠けてもよいもの |
| --- | --- | --- |
| 会議中に黙り、会議後に話す | 3 | World の通常投入、Task |
| 今の状態を根拠付きで答え、古ければ分からない | 1 の後の 2 | Situation TTS、Task |
| 一件のテスト監視が続き、止めれば止まる | 4（3 の保留に依存） | World の通常投入。ただし報告文に World を必須としない |
