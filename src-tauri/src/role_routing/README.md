# Role Routing：実装を読む入口

会話経路・provider選択を変更する前に、この文書の「実行契約」を読む。
その後は「変更目的別の参照先」から必要な行だけをたどる。全ファイルの一括読込は不要。
関数名は検索の起点。仕様変更時はこの案内も更新し、設定値やコード全文の複製は増やさない。

## 責務と主要概念

Role Routingは、**誰に・どの順序で・どの予算内で実行させ、どの結果を回答として採用するか**を管理する。
設定画面やモデル名の対応表だけではない。決定と実行状態をSQLiteへ保存し、会話runtimeがその決定に従ってI/Oを行う。

| 用語 | 意味・混同しないもの |
| --- | --- |
| Role | `reasoner`、`reviewer`などの役割。actor IDへの割当。モデルそのものではない。 |
| Actor | 役割を担う実行主体。transport、providerIdまたはmodel、能力、実行場所などを持つ。 |
| Provider | 実通信の接続設定。actorから参照される。endpoint/auth等はprovider層の責務。 |
| Recipe | actionとrolesから作る有限の実行手順。任意のグラフや無制限の再帰ではない。 |
| Root | 一つのrouting処理の親。policy、入力、revision、期限、step、最終回答を束ねる。通常会話ではrun IDと対応する。 |
| Revision | 条件変更などで進むrootの世代。古い世代の結果を現在の回答として採用しないために使う。 |
| Step / Permit | stepはrecipe中の一実行。`DispatchPermit`はDB上の実行中stepについてactor・世代・予算を確認した実行許可。 |
| Receipt（記録） | 入力、候補、選択、policy等の永続記録。UI表示や一時的なメモリ状態とは別。 |

型・設定検証は [contracts.rs](contracts.rs)、DB制約は [schema.rs](schema.rs) が入口。
provider名・モデル・IPを固定の仕様だと思わない。設定の読込は [settings.rs](../persistence/settings.rs) の `load_role_routing_settings`。

## 通常会話での実行経路

1. **入力の受理**：[runtime/turns.rs](../runtime/turns.rs) から `record_provider_turn_start_in_transaction` を呼ぶ。
   [repository_turns/start.rs](repository_turns/start.rs) がpolicy・候補・選択recipe・stepを入力/runと同じtransactionに記録する。
2. **候補と手順の決定**：[selection.rs](selection.rs) が候補の適格性を判定し、[recipe.rs](recipe.rs) がroleをactorへ解決して有限のstep列にする。
3. **送信前の許可確認**：[conversation_inputs_roles.rs](../runtime/conversation_inputs_roles.rs) の `apply_enabled_role_route` が保存済みの選択を読み、
   [executor.rs](executor.rs) の `permit_next_step` とactorの現在の利用可否を確認する。
4. **実行**：[conversation_turn.rs](../runtime/conversation_turn.rs) がproviderまたはCodex SDKへdispatchする。
   provider側は [conversation_stream.rs](../runtime/conversation_stream.rs)、SDK側は [conversation_role_codex.rs](../runtime/conversation_role_codex.rs) へ進む。
5. **次stepまたは回答採用**：`advance_provider_step` / `advance_review_step` が続きを決める。
   最終回答は `accept_provider_turn` 等で採用条件を確認し、[session_store.rs](../providers/session_store.rs) のtransactionで会話メッセージと整合させる。
6. **発話**：[event_hub.rs](../runtime/event_hub.rs) と [speech_repository.rs](speech_repository.rs) が回答の発話intentを扱う。step完了と発話完了は別。

`driver.rs` は汎用async driverの足場であり、通常会話の入口ではない。まず上記runtimeの呼出し元を追う。
`classifier.rs`、学習、review等も、モジュールが存在するだけで毎ターン実行されるとは限らない。設定と呼出し元の条件を確認する。

## 実行契約：変更時に守ること

- **確定した選択を後段でやり直さない。** rootに保存されたpolicyとdecisionを使い、permitのactorと実dispatchを一致させる。
  設定変更後も既存rootの選択をすり替えない。ただし現在のprovider無効化・cloud許可等は送信直前にも確認する。
- **許可が取れないまま下位providerを呼ばない。** `queued`は待機、`draining`は結果採用を止める状態。
  terminal、古いrevision、期限超過、予算超過を「LLMを動かすため」に回避しない。
- **状態と実行を分離する。** [reducer.rs](reducer.rs) が遷移を計算し、[coordinator.rs](coordinator.rs) が状態・イベントを保存する。
  I/Oはcommit後に行い、DB transactionをawait中に保持しない。
- **重複実行・古い回答を防ぐ。** 会話ごとのactive root、rootごとのactive reasoning stepはDB制約で制御する。
  [steps.rs](steps.rs) のclaim/complete/finalizeを通し、取消後・世代違い・採用済みの結果を再採用しない。
- **予算と権限を共有する。** step追加・tool実行・review・premiumへの移行もrootの制約を引き継ぐ。
  tool specialistは最終回答を公開しない。premiumの承認は候補とrevisionに結びつけ、単なる肯定文で代用しない。
- **fallbackを勝手に追加しない。** provider transportではrole選択適用時にlegacy routeのfallback一覧を消し、root/stepの期限を適用する。
  別actorへの変更は対応するrecipe・revision・承認経路で扱う。
- **音声入力で推論を迂回しない。** ASR確定文は通常の `submitPrompt` / `start_turn` へ入る。
  LFM分類・委譲を通常会話の前提に戻さず、入力を連結・要約・複製しない。ASR/TTS搬送と会話推論は区別する。
- **無効化・再起動を再実行の口実にしない。** 無効化は既存処理のdrain/cancelも含む。
  [recovery.rs](recovery.rs) は中断を記録し、actorやtoolを暗黙に再送しない。

## 変更目的別の参照先

各行の左のファイルから読み、関数の呼出し元・関連テストへ範囲を広げる。

| 調べたいこと | 最初に読む場所 → 境界の確認先 |
| --- | --- |
| role/actor/設定・既定値・移行 | [contracts.rs](contracts.rs) → [settings.rs](../persistence/settings.rs) のbinding検証、[settings_migration.rs](../persistence/settings_migration.rs) |
| 候補から除外された理由・選択 | [selection.rs](selection.rs) → [repository_turns/start.rs](repository_turns/start.rs) の `select_dispatch_candidate`、必要なら [ranker.rs](ranker.rs) |
| recipeの順序・reviewer独立性 | [recipe.rs](recipe.rs) → [executor.rs](executor.rs)、[conversation_turn.rs](../runtime/conversation_turn.rs) |
| 選択されたのに送信されない | [conversation_inputs_roles.rs](../runtime/conversation_inputs_roles.rs) → [executor.rs](executor.rs) → [providers/stream/mod.rs](../providers/stream/mod.rs) |
| HTTP/SSE・providerエラー | [chat_completions/mod.rs](../providers/chat_completions/mod.rs) → [providers/http.rs](../providers/http.rs)、同ディレクトリの `sse.rs` / `chunks.rs` |
| 追加入力・取消・世代変更 | [signals.rs](signals.rs)、[reducer.rs](reducer.rs) → [revision.rs](revision.rs)、[runtime/turns.rs](../runtime/turns.rs) |
| toolの許可・重複・予算 | [tools.rs](tools.rs)、[tool_ledger.rs](tool_ledger.rs) → [tool_specialist.rs](tool_specialist.rs)、[gateway](../tool_selection/gateway.rs) |
| review結果・回答訂正・premium | [review.rs](review.rs)、[proposals.rs](proposals.rs) → [repository_turns/advance.rs](repository_turns/advance.rs) の `advance_review_step` |
| stepに渡す条件・scope | [context.rs](context.rs) → [executor.rs](executor.rs) の `project_context`、[runtime/context](../runtime/context) の実payload構成 |
| 回答の重複保存・古い結果の混入 | [repository_turns/lifecycle.rs](repository_turns/lifecycle.rs) の `accept_provider_turn` → [steps.rs](steps.rs)、[session_store.rs](../providers/session_store.rs) |
| 音声入力の待機・回答の二重発話 | [useConversationTurn.ts](../../../src/features/chat/useConversationTurn.ts) → [speech_repository.rs](speech_repository.rs)、[event_hub.rs](../runtime/event_hub.rs) |
| 監査・UI表示・イベント再生 | [ipc.rs](ipc.rs) → [useRoleRouting.ts](../../../src/features/chat/useRoleRouting.ts)、[routingEventReplay.ts](../../../src/features/chat/routingEventReplay.ts) |
| 起動回復・設定無効化 | [recovery.rs](recovery.rs) → [repository_turns/lifecycle.rs](repository_turns/lifecycle.rs) の `cancel_all_for_disable`、[lib.rs](../lib.rs) の起動処理 |
| 学習・適応選択 | [learning](learning)、[ranker.rs](ranker.rs) → [adaptive_improvement](../adaptive_improvement.rs)。候補の適格性を学習結果で広げない。 |

## 問題の切り分けと検証

`runtime_run_id`から `rr_roots` → `rr_decisions` → `rr_steps` を調べ、policy・revision・actor・phaseを照合する。
必要な表だけ [schema.rs](schema.rs) で列名を確認する。provider実行は `provider_sessions`、最終回答はrootの `result_message_id`、発話は `rr_speech` へ追う。
**候補に選ばれた／permitが出た／HTTPを送った／サーバーが受信した／回答を保存した／発話した、は別の事実。** UIやdecisionだけで到達・完了と判断しない。

変更したモジュール内の `#[cfg(test)]` をまず読む。dispatch境界は [conversation_inputs_roles.rs](../runtime/conversation_inputs_roles.rs) 内の
`rr_03_queued_root_uses_its_immutable_policy_receipt`、`rr_04_receipt_and_rr_12_completion_are_persisted_once`、`rr_22_*` が入口。

```sh
# リポジトリルートで実行。変更した範囲を選ぶ。
cargo test --manifest-path src-tauri/Cargo.toml role_routing::
cargo test --manifest-path src-tauri/Cargo.toml conversation_inputs_roles::
bun test tests/role-routing-replay.test.ts tests/role-routing-proposal.test.tsx tests/role-routing-codex.test.ts
```

provider/音声経路を変えた場合は、対象実サーバーの受信、応答終端、回答保存、最終発話まで同じrunで照合する。
単体テスト合格は通信成功の証明にならない。READMEのみの変更ではリンクと記述の整合性を確認する。
