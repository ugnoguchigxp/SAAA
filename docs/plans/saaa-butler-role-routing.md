# SAAA 執事エージェント実装計画（Role Routing 版）

作成日: 2026-09-24
状態: 計画確定、未着手
置き換え対象: `docs/plans/ornith15-butler-migration.md`（音声セッション内の受付経路 `larm_voice/frontdesk*` を前提にしていたため破棄。Profile 選択などの `larm-session` 部分だけ本計画の Phase 1 で引き継ぐ）
追記（2026-09-25）: LARM の profile selector（`SAAA` / `SAAA-w-Image` / `SAAA-w-music`）への対応と、2 つの LLM を catalog どおりに揃える作業は、第 4 版に改訂した `docs/plans/ornith15-butler-migration.md` で行う。

## 0. 実装者への指示

- Phase を順番に実装する。各 Phase のチェックポイントが通るまで次に進まない。
- 「決定事項」の値と名前はそのまま使う。書かれていない設計判断が必要になったら、実装を止めて質問する。
- 「止めて質問する」と書かれた箇所では、推測で進めない。
- 作業ツリーには本件と関係のない未コミット変更（`tool_selection/**`、`artifact_preview/**`、`ci.yml` など）がある。触らない。コミットは Phase ごとに、この文書に列挙したファイルだけを `git add -p` で選ぶ。
- 行番号は 2026-09-24 時点。ずれていたら関数名で検索する。

### 0.1 到達性のルール（前回の失敗を繰り返さないため）

前回は、UI から呼ばれない関数に受付を実装し、その関数を直接呼ぶテストで「動いた」と判断した。これを防ぐため、次を必須にする。

- **入口から通すテストだけを完了の根拠にする。** 新しい挙動を証明するテストは、`execute_conversation_turn_with_candidates`（`start_turn` が呼ぶ会話ターンの入口）から、`input_origin: "voice"`、`routing.roles.enabled = true` の設定、LARM fixture を使って実行する。新しく書いた関数を直接呼ぶ単体テストは補助であり、完了の根拠にしない。
- **先に骨組みを通す。** 機能を足す前に、Phase 2 の手順 0 で「現行コードのまま 2 step の recipe が両方 LARM fixture に届く」テストを書き、通ることを確認する。通らなければ、その時点で止めて質問する。
- **経路の途中にある条件はすべて 2.6 に書いてある。** 2.6 に載っていない条件（設定のフラグ、環境変数、既定値）で分岐する箇所を見つけたら、止めて質問する。
- **実機での確認は、ログではなく記録で行う。** 12 章の SQL で `rr_steps` の担当者と Provider を確認する。

## 1. 目的と責任分担

執事エージェントの本体は **SAAA** が実装する。LARM は LLM・ASR・TTS・Embedding を提供する Provider にすぎない。

| 担当 | 中身 |
|---|---|
| SAAA | 会話ターン、役割ルーティング（誰が何をするか）、受付と思考の連携、Tool 実行、メモリー、world model、最終回答の採用、発話 |
| LARM | 1 回の claim で `llm`（Ornith 1.5 35B）、`backchannel`（Qwen 3.5 2B）、`asr`、`tts`、`embedding` を貸し出す |

目指す構図:

- 受付（role `frontend`）: Qwen 3.5 2B。相槌・挨拶・お礼への即答、「考えます」などの短い ack。
- 思考（role `reasoner`）: Ornith 1.5 35B。推論、Tool use、メモリー、world model、最終回答。
- Tool 選択の補助（role `tool_specialist`、Stage 2）: Qwen 3.5 2B。Tool 操作を 1 つ JSON で選ぶだけ。実行は SAAA、最終回答は Ornith。

## 2. 現行コードの事実（この計画の前提）

### 2.1 音声は通常ターンとして処理される

- UI は音声の確定文を `submitPrompt(queued.text, { sourceId: queued.utteranceId })` で通常の会話ターンとして送る。`receiveLfmUtterance` は呼ばない。`tests/microphone-desktop-contracts.test.ts` の 207〜220 行目がこれを保証している。
- `src-tauri/src/runtime/README.md`: 「Voice and text use the ordinary turn path. Do not restore LFM classification/delegation as a prerequisite for voice reasoning.」
- `src-tauri/src/role_routing/README.md`: 「Normal voice uses the ordinary turn path, without LFM classification or input rewriting.」
- したがって `src-tauri/src/providers/larm_voice/frontdesk.rs` の `receive_lfm_utterance` と `frontdesk_decision.rs` は、現在の UI から到達しない。**受付をここに実装しても動かない。**

### 2.2 会話ターンは Role Routing が担当者を決める

- 設定 `routing.roles` の `actors`（担当者）、`roles`（役割 → actor）、`recipes`（手順）で決まる。型は `src-tauri/src/role_routing/contracts.rs`。
- `recipe.rs` の `step_purposes` は、`Respond` に対して `roles: ["frontend", "reasoner"]` なら `["frontend", "respond"]` の 2 step を組み立てる。受付 step を 1 つだけ先頭に置く形は、すでに設計上許されている（テスト `rr_05_frontend_ack_then_reasoner_compiles_as_two_bounded_steps`）。
- 各 step は `runtime/conversation_inputs_roles/role_dispatch.rs` で actor に解決される。`transport == "provider"` なら `route.primary_provider_id = actor.provider_id` にして、通常の Provider 実行（`runtime/conversation_provider_route.d/01.rs` の `execute`）に渡す。
- step の出力は `role_step_sink` で候補（`RoleCandidate`）として記録され、`execute_conversation_turn_with_candidates` が次の step を再帰的に実行する（`conversation_provider_route.d/01.rs` 398、413、476 行目付近）。
- `frontend` step に渡す入力は、元の依頼文そのまま（`conversation_role_steps.rs` の `role_step_request`、274 行目）。`frontend` step の出力をどう扱うか（発話するか、次の step に渡すか）の実装は**ない**。

### 2.3 LARM への振り分け

- `runtime/conversation_role_steps.rs` の `should_share_larm_voice_session` は、`primary_provider_id == DYNAMIC_LAN_PROVIDER_ID` かつ音声入力のとき `true` を返す。このとき `providers/stream/larm_voice.rs` の `stream_larm_voice_provider` が LARM セッションから Provider を借りる。
- 借りる Provider 名は `acquire("llm")` に**固定**されている。actor から `backchannel` を指定する手段はない。
- `RoutingActor` は `transport == "provider"` のとき `model` を持てない（`contracts.rs` の `validate_settings`、255 行目）。

### 2.4 いまの保存設定

- reasoner の actor は、表示名が「Gemma 4 E4B (LARM)」のまま、`providerId` が `commandcode-deepseek-v4-1-flash` に書き換わっている（2026-09-21 23:14）。このため会話は DeepSeek に届いている。**これは別エージェントの報告で、この計画の作成者は確認していない。Phase 0 で確かめる。**
- `frontend` 役は未設定（v36 の migration で削除済み）。

### 2.6 音声ターンが受付と Ornith に届くまでの条件（コードで確認済み）

経路: UI の `submitPrompt`（音声は `inputOrigin: "voice"`）→ `start_turn` → Role Routing の受付記録（`rr_roots` / `rr_steps` の作成）→ `execute_conversation_turn_with_candidates` → `apply_enabled_role_route` → `provider_route::execute` → step ごとに再帰。

| # | 条件 | 場所 | 満たさないとき |
|---|---|---|---|
| 1 | `routing.roles.enabled == true` | `role_dispatch.rs` の `apply_enabled_role_route` 冒頭。**既定値は `false`**（`contracts.rs` の `Default`） | Role Routing を通らず、会話 route の Provider（DeepSeek など）に直接届く。recipe も actor も無視される |
| 2 | 受付入り recipe が、`respond` の候補の中で最初に選ばれる | 同関数。候補は `recipe_id` の**辞書順で先頭**が選ばれる（adaptive improvement が無効のとき）。選択結果は受付時に `rr_decisions` に固定される | 既存の reasoner だけの recipe が選ばれ、受付 step が作られない |
| 3 | 各 actor が到達可能（`exclusion_reason` なし） | `selection::candidates_for_action` | recipe ごと候補から外れる |
| 4 | actor の `providerId == DYNAMIC_LAN_PROVIDER_ID` かつ音声入力 | `should_share_larm_voice_session` | LARM 音声セッションを使わず、テキスト用の `dynamic_lan`（`llm` 1 本）に行く |
| 5 | 音声の聞き取りが ON で、LARM 音声セッションが開始済み | UI の `useLarmVoiceLifetime`（`useAmbientVoiceSession.ts` 52 行目）が聞き取り ON で開始する。Rust 側は `larm_voice::enabled()`（環境変数で無効化されていない） | `LARM voice session is not started` で失敗する |
| 6 | step の完了後に次の step が `planned` で残っている | `advance.rs` の `advance_provider_step`。purpose を問わず次の step を起動する | 最終回答として採用される |

`route.source` は Role Routing で `"provider"` になるため、reasoning-mcp 経路（`dispatch_reasoning_client`、`route.source == "harness"` のときだけ）には入らない。

### 2.7 ack 発話の既存経路（コードで確認済み）

- `runtime/reasoning_ack.rs` の `speak` が ack を発話する。`hub.speech.begin(state, "{run_id}_ack", ...)` で準備し、`hub.speech.finish(&ack_id, text)` で読み上げる。準備と読み上げ完了を 2 秒の期限で待つ。文言は「確認します。」/「Let me check.」に固定されている。
- 呼び出し口は `RuntimeEventSender::acknowledge`。実装は `TurnEventHub`（`voice_response.rs` 95 行目）だけで、**trait の既定実装は何もしない**（`event_hub.rs` 66 行目）。
- role step の出力を受ける `BufferedRoleStepSink` は `acknowledge` を実装していない。**step 用の sink に対して呼んでも発話されない。** 外側の `on_event` に対して呼ぶ必要がある。
- 現在の呼び出し元は `conversation_controller`（reasoning-mcp 経路）だけ。Role Routing の Provider 経路では ack は出ていないので、受付の ack と二重にはならない。

### 2.5 前回実装（未コミット）の扱い

前回の実装はコミットされておらず、作業ツリー（ブランチ `feat/ornith15-session`）にある。Phase 1 の手順 1 で、次の表のとおり処理する。

**戻す（`main` の状態にする）**

受付を到達しない経路（`larm_voice/frontdesk*`）に実装した部分。すべて戻す。

| ファイル | 操作 | 理由 |
|---|---|---|
| `src-tauri/src/providers/larm_voice/frontdesk_decision.rs` | `git checkout main -- <file>` | UI から呼ばれない受付経路の書き換え |
| `src-tauri/src/providers/larm_voice/frontdesk.rs` | `git checkout main -- <file>` | 同上（タイムアウトの変更） |
| `src-tauri/src/providers/larm_voice/frontdesk_repository_tests.rs` | `git checkout main -- <file>` | `ConversationDecision` へのフィールド追加に追従しただけ |
| `src-tauri/src/providers/larm_voice/escalation.rs` | 削除（新規ファイル） | 到達しない受付経路の判定ロジック |
| `src-tauri/src/providers/larm_voice/butler_turn_test.rs` | 削除（新規ファイル） | 到達しない経路を直接呼ぶテスト。実際の会話ターンを通っていない |

`escalation.rs` の JSON 検証の考え方は Phase 3 の `role_routing/frontend.rs` の参考にしてよい。ただしコピーせず、Phase 3 の schema で書き直す。

**一部だけ戻す（hunk 単位）**

| ファイル | 戻す hunk | 残す hunk |
|---|---|---|
| `src-tauri/src/providers/larm_voice/mod.rs` | `pub(crate) mod escalation;` の 1 行 | `pub(crate) mod profile;`、`profile::preference` / `profile::label` を使う変更、`ProfilePreference` を渡す変更 |
| `src-tauri/src/providers/larm_voice/world_tests.rs` | 先頭の `#[path = "butler_turn_test.rs"] mod butler_turn_test;` の 2 行 | `profile` と `larm_profile` を `"fixture-voice"` にした 2 か所（`DEFAULT_PROFILE` が canonical になり `backchannel` が必須になったため、4 Provider の fixture を Explicit で動かす必要がある） |

**残す（Phase 1 で見直して確定する）**

LARM から Provider を借りる部分。新しい構成でもそのまま必要になる。

| ファイル | 内容 |
|---|---|
| `crates/larm-session/src/contract.rs` | `BASE_PROVIDERS` と `BACKCHANNEL`、Profile 定数、`required_providers`、`backchannel` の `contextWindow` 検証 |
| `crates/larm-session/src/catalog.rs`（新規） | `GET /v3/agent-profiles` と Profile の自動選択 |
| `crates/larm-session/src/lib.rs` | `ProfilePreference`、`profile_id()`、`has_provider()`、claim 時の health check を claim した全 Provider に行う変更 |
| `crates/larm-session/src/http_api.rs` | `Thinking`（`enable_thinking` の制御） |
| `crates/larm-session/src/tests.rs` | Profile 選択、5 Provider の claim、renew / release のテスト |
| `crates/larm-session/tests/live.rs` | 新しい API への追従 |
| `src-tauri/src/providers/larm_voice/profile.rs`（新規） | 保存設定から `ProfilePreference` を決める |
| `src-tauri/src/providers/stream/larm_voice.rs` | Ornith の request に `Thinking::Disabled` を付ける |
| `src-tauri/src/diagnosis/checks/harness.rs` | `ProfilePreference` への追従 |
| `src-tauri/src/harness_llm_diagnostic.rs` | 同上 |
| `src-tauri/src/memory/personal_state/product_binding.rs` | 同上 |
| `src-tauri/src/voice/tts_catalog.rs` | 同上 |
| `src/lib/providerSchemas.ts`、`src/lib/settingsTypes.ts` | `LlmRequestOptions.thinking` の型 |

**触らない（本件と無関係の未コミット変更）**

`.github/workflows/ci.yml`、`critical-path-freeze.json`、`package.json`、`src-tauri/src/artifact_preview/**`、`src-tauri/src/bin/toolchain_simulate.rs`、`src-tauri/src/generated_capabilities/tools.rs`、`src-tauri/src/providers/chat_completions/mod.rs`、`src-tauri/src/providers/chat_completions/voice_progress.rs`、`src-tauri/src/providers/stream/agent_dispatch.rs`、`src-tauri/src/tests/typed_memory_tools_are_routed_only_from_a_valid_.rs`、`src-tauri/src/tool_selection/**`、`src/features/chat/artifacts/**`、`tests/artifact-tab.test.ts`。戻さない、コミットにも含めない。

## 3. 決定事項

### 3.1 actor から LARM の Provider を選ぶ

- `RoutingActor` に `#[serde(default)] pub(crate) larm_provider: Option<String>` を追加する（JSON は `larmProvider`）。
- 許される値は `"llm"` と `"backchannel"` のみ。`provider_id == DYNAMIC_LAN_PROVIDER_ID` の actor だけが持てる。それ以外の actor で `Some` なら設定エラー。
- `None` は `"llm"` と同じ意味（既存の保存設定との互換）。
- `RoleDispatch::Provider` に `larm_provider: &'static str` を追加し、`stream_larm_voice_provider` の `acquire("llm")` をこの値に置き換える。

### 3.2 受付 step（`frontend`）の役割

受付は「思考の前提」ではなく「思考の前に出す短い応答」とする。README の「LFM 分類を思考の前提にしない」を守るため、次の 2 段階で実装する。

| Stage | 受付の出力 | Ornith の実行 |
|---|---|---|
| Stage 1 | ack だけを出す。相槌や挨拶なら定型文、依頼なら「確認します。」など | **常に実行する**（受付の結果で止めない） |
| Stage 2 | 定型文で完結できる場合に限り、ターンを受付で完了してよい | 受付が完了させた場合だけ実行しない |

Stage 2 で受付がターンを完了できる条件（すべて満たすとき）:

- `resolves_turn == true` かつ `confidence == "high"`
- ack が 3.3 の定型文テーブルに完全一致で当たる
- Tool、状態変更、推論が不要（`intent` が `social` か `acknowledgement`）

それ以外は必ず Ornith に進む。受付の出力が不正・空・タイムアウトのときも Ornith に進み、ack は出さない。

### 3.3 受付の出力と発話

- 受付の出力は strict JSON。**ユーザーに見せる文字列はモデルに作らせない。** SAAA の定型文テーブルから選ぶ。
  ```json
  {"ack": "none|nod|greeting|thanks|working", "intent": "social|acknowledgement|request|unclear",
   "resolvesTurn": true|false, "confidence": "high|low"}
  ```
- 定型文テーブル（`role_routing/frontend.rs` に置く）

  | ack | 発話 |
  |---|---|
  | `nod` | 「はい。」 |
  | `greeting` | 「こんにちは。」（その会話ですでに挨拶済みなら発話しない） |
  | `thanks` | 「どういたしまして。」 |
  | `working` | 「確認します。」 |
  | `none` | 発話しない |

- request body: `stream: true`、`max_tokens: 64`、`temperature: 0.0`、`chat_template_kwargs: {"enable_thinking": false}`、`response_format` に上の JSON schema（400 / 422 のときだけ外して 1 回再送）。
- タイムアウトは `limits.frontend_timeout_ms`（既存設定。既定 1,200 ms）。実測 TTFT は 70ms なので十分に余裕がある。

### 3.3.1 回答の確定と待ち時間のつなぎ（決定: 案 A）

- Ornith は結果を置き場に書くだけで、チャット欄にも発話にも直接出さない。置き場は既存の role step の候補（`BufferedRoleStepSink` と `RoleCandidate`、`rr_steps`）を使う。新しい保存先は作らない。
- 最終回答は **SAAA のホストが Ornith の本文をそのまま採用して出す**。2B に言い直させない（README「Specialist cannot publish the final answer」を守る）。
- ホストは結果の完成を**待ち受けで**知る（step の完了）。定期的に確認しに行く方式は使わない。
- Ornith の結果は最大 40 秒（2 秒 × 20 tick）待つ。tick は 2 秒ごとだが、**tick ごとには発話しない**。
- 待っている間の相槌は 5 tick に 1 回（10 秒・20 秒・30 秒）だけ、「まだ確認しています。」を出す。文言はホストの定型文で、2B には作らせない。

- 予定の時点で直前の発話がまだ読み上げ中なら、次の tick に 1 回だけ持ち越す。持ち越し先でも読み上げ中なら、その相槌は出さない。
- 40 秒で結果がなければ Ornith の step をキャンセルし、「すみません、時間内にお答えできませんでした。」を最終回答にする。Ornith が完了、失敗、キャンセルした時点で止める。
- 結果が遅れたときの発話は、つなぎだけにする。部分的な回答や推測は出さない。

### 3.4 テキスト入力の扱い

- 音声以外（テキスト）のターンでは LARM 音声セッションがないため、受付 step は Provider を呼ばずに `succeeded`（出力は空、ack なし）で完了させ、すぐ `respond` に進む。

### 3.5 Ornith の設定

- reasoner の actor: `transport: "provider"`、`providerId: DYNAMIC_LAN_PROVIDER_ID`、`larmProvider: "llm"`。
- 会話本体には `chat_template_kwargs.enable_thinking=false` を付ける（前計画の `Thinking::Disabled` を引き継ぐ）。

### 3.6 保存設定の扱い

- コードから保存設定を自動で書き換えない。
- 新しい構成は、設定画面またはこの計画の Phase 0 の手順で、ユーザーが明示的に適用する。
- 既定値（新規インストール時の `routing.roles` の初期値）は Phase 4 で新しい構成に変える。

## 4. やらないこと

- `larm_voice/frontdesk*` の受付経路を復活させる、または UI から `receiveLfmUtterance` を呼ぶこと。
- 受付の結果で Ornith の実行を止めること（Stage 2 の条件を満たす場合を除く）。
- 受付モデルに、ユーザーへ見せる文字列を作らせること。
- 受付モデルに Tool を実行させること、最終回答を採用させること。
- 保存設定の自動書き換え。
- production への切替、LARM service の操作、media variant の操作。

## 5. Phase 0: 現在の保存設定を確認する（コード変更なし）

実装に入る前に、実装者が次を確認し、結果を PR 説明に書く。書き換えはしない。

1. アプリのデータディレクトリの SQLite を**コピーして**読む（元ファイルは開かない）。
2. `settings_documents` の `namespace='routing.roles'` から次を確認する。
   - `enabled`（2.6 の条件 1）
   - `roles.reasoner` と `roles.frontend` が指す actor の `providerId`
   - `recipes` のうち `action == "respond"` のものすべての `id` と `roles`（2.6 の条件 2。辞書順で先頭になる recipe を特定する）
   - `adaptiveImprovement.enabled` と `providerRecipe`
3. `namespace='providers.model'` の `harness.larmProfile` を確認する。
4. 直近の音声ターン 1 件について `SELECT ordinal, purpose, actor_id, status FROM rr_steps WHERE root_id=<run_id> ORDER BY ordinal` を読み、実際にどの actor が動いたかを確認する。行がなければ Role Routing を通っていない。
5. 結果を「enabled = …、respond recipes = […]、reasoner = `<providerId>`、larmProfile = `<値>`、直近ターンの steps = […]」の形で報告する。**2.4 の記述と違っていたら、止めて質問する。**

新構成の適用（reasoner を LARM に戻す、frontend 役を追加する）は、Phase 3 完了後にユーザーが設定画面で行う。

## 6. Phase 1: `larm-session` の確定（引き継ぎ分）

ブランチ: `feat/butler-larm-session`（`feat/ornith15-session` から作る）

### 手順

1. 2.5 の表のとおり前回実装を整理する。
   ```sh
   git checkout main -- \
     src-tauri/src/providers/larm_voice/frontdesk_decision.rs \
     src-tauri/src/providers/larm_voice/frontdesk.rs \
     src-tauri/src/providers/larm_voice/frontdesk_repository_tests.rs
   rm src-tauri/src/providers/larm_voice/escalation.rs \
      src-tauri/src/providers/larm_voice/butler_turn_test.rs
   ```
   続けて、エディタで次の行を削除する。
   - `src-tauri/src/providers/larm_voice/mod.rs` の `pub(crate) mod escalation;`
   - `src-tauri/src/providers/larm_voice/world_tests.rs` の `#[path = "butler_turn_test.rs"]` と `mod butler_turn_test;`

   最後に `cargo check --tests --manifest-path src-tauri/Cargo.toml` が通ることを確認する。
2. 残す変更について、前回のレビュー指摘を直す。
   - `crates/larm-session/src/lib.rs` 冒頭のコメント「all four tokens」を「all claimed tokens」に直す。
   - `catalog::fetch` を `connect_inner` のキャンセル監視の内側に移す。`tokio::select!` で `cancelled(&mut cancellation)` と競わせる。
   - `catalog::fetch` の応答は既存の `http::json` ヘルパー（サイズ上限あり）で読む。
   - `profile.rs` の `label()` は、`Explicit("auto")` と `Auto` を区別できるよう、`Auto` を `"@auto"` にする（`@` は `validate_profile` で拒否される文字なので衝突しない）。
3. `larm-session` の `Session` に `pub async fn provider_names(&self) -> Vec<String>` を追加する（Phase 2 の検証用）。

### チェックポイント

```sh
cargo test --manifest-path crates/larm-session/Cargo.toml
cargo test --manifest-path src-tauri/Cargo.toml larm
cargo test --manifest-path services/reasoning-mcp/Cargo.toml
git diff main --stat -- src-tauri/src/providers/larm_voice/frontdesk_decision.rs src-tauri/src/providers/larm_voice/frontdesk.rs src-tauri/src/providers/larm_voice/frontdesk_repository_tests.rs   # 差分なし
test ! -e src-tauri/src/providers/larm_voice/escalation.rs && test ! -e src-tauri/src/providers/larm_voice/butler_turn_test.rs
```

コミットは 2.5 の「残す」ファイルだけを対象にする（`git add -p` で `larm_voice/mod.rs` と `world_tests.rs` は残す hunk だけを選ぶ）。

## 7. Phase 2: actor から LARM の Provider を選べるようにする

ブランチ: `feat/butler-actor-larm-provider`

### 変更ファイル

- `src-tauri/src/role_routing/contracts.rs`
- `src-tauri/src/runtime/conversation_inputs_roles/role_dispatch.rs`
- `src-tauri/src/runtime/conversation_turn.rs`
- `src-tauri/src/runtime/conversation_provider_route.d/01.rs`
- `src-tauri/src/providers/stream/larm_voice.rs`
- `src/lib/settingsTypes.ts`、`src/lib/providerSchemas.ts`（`RoutingActor` の型に `larmProvider?: "llm" | "backchannel"` を追加）
- テスト: `src-tauri/src/runtime/conversation_inputs_roles/tests.rs`、`src-tauri/src/role_routing/contracts.rs` の tests

### 手順

0. **骨組みテスト（機能追加の前に書く）**: `world_tests.rs` の LARM fixture を使う既存テストの隣に `role_routed_voice_turn_reaches_larm_for_every_step` を書く。
   - 設定: `routing.roles.enabled = true`、actor `a` と `b`（どちらも `providerId: DYNAMIC_LAN_PROVIDER_ID`）、recipe `respond` = `["frontend","reasoner"]`、ほかの respond recipe はなし。
   - 入口: `execute_conversation_turn_with_candidates` を `input_origin: "voice"` で呼ぶ。LARM 音声セッションは fixture で開始済みにする。
   - 期待値: `rr_steps` に `frontend` と `respond` の 2 行があり両方 `succeeded`。fixture の `llm` に 2 回 request が届く（この時点では両 step とも `llm`）。最終回答は 2 回目の出力。
   - **このテストが現行コードで通らなければ、以降の手順に進まず止めて質問する。** 通ったら、以降の Phase ではこのテストを書き換えて期待値を更新していく（Phase 2 で 1 回目が `backchannel`、Phase 3 で受付の専用処理）。
1. `contracts.rs`: `RoutingActor` に `larm_provider` を追加する（3.1）。`validate_settings` のループに次の検証を足す。
   - `Some(v)` のとき、`v` が `"llm"` か `"backchannel"` で、かつ `provider_id == Some(DYNAMIC_LAN_PROVIDER_ID)`。違反は `"Invalid role routing actor transport"`。
   - 既存テストで `RoutingActor { ... }` を直接書いている箇所には `larm_provider: None` を足す（`rg -n "RoutingActor \{" src-tauri/src` で列挙）。
2. `role_dispatch.rs`: `RoleDispatch::Provider { max_input_bytes }` を `RoleDispatch::Provider { max_input_bytes, larm_provider: &'static str }` にする。値は `actor.larm_provider.as_deref()` が `Some("backchannel")` なら `"backchannel"`、それ以外は `"llm"`。
3. `conversation_turn.rs`: `role_dispatch` から `larm_provider` を取り出し、`provider_route::execute` に新しい引数 `larm_provider: &'static str` として渡す。role dispatch がない場合は `"llm"`。
4. `conversation_provider_route.d/01.rs`: 受け取った `larm_provider` を、LARM 音声経路の呼び出し（`stream_voice_aware_dynamic_lan_provider`）まで渡す。途中の関数に引数を 1 つずつ足す。
5. `stream/larm_voice.rs`: `stream_voice_aware_dynamic_lan_provider` と `stream_larm_voice_provider` に `larm_provider: &'static str` を足し、`ready.session.acquire("llm")` を `ready.session.acquire(larm_provider)` にする。
   - `larm_provider == "backchannel"` で、LARM 音声セッションが共有されていない（`shared_voice_session == false`）場合は、`ProviderFailureKind::Contract` で失敗させる。テキスト経路の `dynamic_lan` は `llm` しか扱えないため。
   - `Thinking::Disabled` は `llm` と `backchannel` の両方で付ける。
6. TypeScript の型を追加する。設定画面の UI はこの Phase では変えない。

### 追加テスト

| 場所 | テスト名 | 期待値 |
|---|---|---|
| `contracts.rs` | `larm_provider_is_limited_to_dynamic_lan_actors` | `providerId` が他の Provider で `larmProvider: "llm"` なら設定エラー |
| `contracts.rs` | `larm_provider_accepts_only_llm_or_backchannel` | `"asr"` はエラー、`"backchannel"` は OK |
| `contracts.rs` | `stored_actor_without_larm_provider_still_loads` | `larmProvider` キーのない JSON が読める |
| `conversation_inputs_roles/tests.rs` | `backchannel_actor_dispatches_with_backchannel_provider` | `RoleDispatch::Provider { larm_provider: "backchannel", .. }` |
| `world_tests.rs`（LARM fixture を使う既存テストの隣） | `voice_step_acquires_actor_larm_provider` | fixture の LARM に 5 Provider を返させ、`larm_provider = "backchannel"` で実行すると request が `backchannel` の base_url と token に届く。`llm` には届かない |

### チェックポイント

```sh
cargo test --manifest-path src-tauri/Cargo.toml role_routing
cargo test --manifest-path src-tauri/Cargo.toml conversation_inputs_roles
cargo test --manifest-path src-tauri/Cargo.toml larm
bun test
```

## 8. Phase 3: 受付 step（Stage 1: ack のみ）

ブランチ: `feat/butler-frontend-ack`

### 変更ファイル

- `src-tauri/src/role_routing/frontend.rs`（新規）
- `src-tauri/src/role_routing/mod.rs`（`pub(crate) mod frontend;`）
- `src-tauri/src/runtime/conversation_role_steps.rs`
- `src-tauri/src/runtime/conversation_provider_route.d/01.rs`
- `src-tauri/src/runtime/reasoning_ack.rs`、`src-tauri/src/runtime/event_hub.rs`（trait）、`src-tauri/src/runtime/voice_response.rs`（`TurnEventHub`）
- テスト: `src-tauri/src/runtime/conversation_inputs_roles/tests.rs`、`role_routing/frontend.rs` の tests

### 手順

1. **`role_routing/frontend.rs`** に、HTTP に依存しない純粋関数を置く。
   ```rust
   pub(crate) enum Ack { None, Nod, Greeting, Thanks, Working }
   pub(crate) enum Intent { Social, Acknowledgement, Request, Unclear }
   pub(crate) struct FrontendResult { pub ack: Ack, pub intent: Intent, pub resolves_turn: bool, pub high_confidence: bool }
   pub(crate) fn parse(raw: &str) -> Result<FrontendResult, &'static str>;          // deny_unknown_fields。失敗は Err
   pub(crate) fn ack_text(ack: Ack, already_greeted: bool) -> Option<&'static str>; // 3.3 の定型文テーブル
   pub(crate) const INSTRUCTION: &str = "...";                                      // 手順 2
   pub(crate) fn response_format() -> serde_json::Value;                           // 3.3 の schema
   ```
2. **INSTRUCTION** に含める内容:
   - あなたは音声受付。返すのは JSON だけ。文章は書かない。
   - 相槌・同意 → `ack: nod, intent: acknowledgement`
   - 挨拶だけ → `ack: greeting, intent: social`
   - お礼だけ → `ack: thanks, intent: social`
   - 依頼・質問 → `ack: working, intent: request`
   - 判断できない → `ack: none, intent: unclear`
   - `resolvesTurn` は、挨拶・お礼・相槌だけで返事が完結するときだけ `true`
   - 自信がなければ `confidence: low`
3. **受付 step の request を組み立てる**（`conversation_role_steps.rs` の `role_step_request`）。
   - `"frontend"` の分岐を、`"respond" | "reconsider" | "frontend"` から分ける。`frontend` のときは元の依頼文をそのまま返す（現状維持）。
   - system prompt と body の差し替えは、Provider 実行側で `purpose == "frontend"` を見て行う（手順 4）。
4. **ack の発話口を文言指定できるようにする**（2.7 の既存経路を使う。新しい `RuntimeEvent` は作らない）。
   - `reasoning_ack.rs`: `speak` の本体を `speak_text(hub, state, run_id, conversation_id, text: &str, wait_end: bool, cancellation)` に切り出す。既存の `speak` は、言語で選んだ固定文言と `wait_end = true` で `speak_text` を呼ぶだけにする（既存の挙動は変えない）。
   - `wait_end == false` のときは、`begin` の準備（既存の 2 秒期限）と `finish(&ack_id, text)` までで戻り、`speechEnded` を待たない。
   - `RuntimeEventSender` に `acknowledge_text(state, run_id, conversation_id, text: String, cancellation)` を追加する。既定実装は何もしない。`TurnEventHub` だけが `speak_text(..., wait_end = false, ...)` を呼ぶ。
   - 呼び出しは**外側の `on_event`**（`execute` の引数）に対して行う。`role_step_sink` 経由の `provider_events` に対して呼ばない（2.7）。
5. **受付 step の実行**（`conversation_provider_route.d/01.rs`）。`active_provider_step` が決まった直後、`compose_after_connect`（履歴・world・context の組み立て）と `role_step_sink` の作成より**前**に、`purpose == "frontend"` なら次の専用処理に分岐して戻る。受付に不要な context 組み立てで遅れないようにするため。
   - 3.4 のとおり、共有 LARM 音声セッションがない場合は、Provider を呼ばずに step を空の出力で完了し、次の step に進む。
   - 共有セッションがある場合: actor の `larm_provider`（Phase 2）で Provider を借り、system に `frontend::INSTRUCTION`、user に直近の発言だけを入れて、3.3 の body で 1 回呼ぶ。`limits.frontend_timeout_ms` で囲む。会話履歴、world、Tool は渡さない。
   - SSE の `delta.content` を連結し、`frontend::parse` にかける。
   - 成功し、`ack_text` が `Some(text)` なら、`on_event.acknowledge_text(...)` を呼ぶ。`speech.maxAckChars` を超える文言は発話しない。
   - step の出力（`RoleCandidate.content`）には、parse 済みの JSON を文字列で入れる。parse に失敗したら空文字列を入れる。
   - 成功・失敗にかかわらず、`advance_provider_step` で step を完了し、既存の再帰と同じく `execute_conversation_turn_with_candidates` に戻って `respond` に進む（Stage 1 では受付の結果で Ornith を止めない）。
   - 手順 0 の骨組みテストで使った経路（`advance_provider_step` → 再帰）から外れる書き方をしない。
6. **`respond` step への影響**: `role_step_request` の `"respond"` は、これまでどおり元の依頼文だけを渡す。受付の出力は Ornith に渡さない。
7. **挨拶済みの判定**: Stage 1 では `already_greeted = false` 固定で実装し、PR 説明に未対応と書く。判定の置き場所は Stage 2 で決める。
8. **待ち時間のつなぎ**（3.3.1）: `respond` step の `streaming::attempt` を `tokio::select!` で 2 秒間隔の `tokio::time::interval`（最初の tick は捨てる）と並べる。tick の番号が 5 の倍数（5、10、15）のときだけ、外側の `on_event.acknowledge_text("まだ確認しています。")` を呼ぶ。直前の発話の `speechEnded` をまだ受けていなければ次の tick に 1 回だけ持ち越す。tick 20（40 秒）で `attempt` をキャンセルし、3.3.1 のタイムアウト文言で完了する。条件は、音声ターンであることと、`role_candidates` に `frontend` の候補があること。`attempt` の完了で select を抜け、つなぎは止まる。
   - Ornith の本文が確定前に UI や読み上げに漏れないことを、既存の `role_step_sink` の仕組みのまま確認する。最終 step の出力がバッファされずに流れていると分かったら、止めて質問する。
   - テスト: `filler_is_spoken_every_fifth_tick`（tokio の仮想時間で 25 秒かかる Ornith → 10 秒と 20 秒の 2 回）、`no_filler_when_reasoner_finishes_within_ten_seconds`、`filler_is_deferred_once_while_previous_speech_plays`、`reasoner_times_out_after_forty_seconds`（最終回答がタイムアウト文言、Ornith の step は `cancelled`）、`reasoner_draft_is_not_visible_before_adoption`。いずれも会話ターンの入口から実行する。
9. **ack と回答の発話順**: ack の発話 id は `{run_id}_ack`、回答は `run_id`。回答の発話が ack より先に始まらないことを、テスト `ack_speech_is_queued_before_answer_speech` で確認する。speech の仕組みで順序を保証できないと分かったら、止めて質問する。

### 追加テスト

| テスト名 | 内容 | 期待値 |
|---|---|---|
| `frontend::parse_accepts_exact_schema` | 正しい JSON | 各フィールドが読める |
| `frontend::parse_rejects_extra_fields_and_free_text` | 余分なキー、`はい、承知しました` | `Err` |
| `frontend::ack_text_is_host_owned` | 各 ack | 3.3 のテーブルどおり。挨拶済みの `greeting` は `None` |
| `voice_turn_runs_frontend_then_reasoner` | recipe `["frontend","reasoner"]`、frontend actor は `backchannel`、reasoner actor は `llm`。音声ターン「明日の予定を確認して」 | `backchannel` に 1 回、`llm` に 1 回届く。`backchannel` の request に `enable_thinking: false` と `stream: true`。ack「確認します。」が発話キューに積まれる。最終回答は `llm` の出力 |
| `frontend_failure_still_runs_reasoner` | `backchannel` が不正な JSON を返す | ack なし。`llm` は実行され、最終回答が採用される |
| `frontend_timeout_still_runs_reasoner` | `backchannel` が応答しない | `frontend_timeout_ms` 後に `llm` が実行される |
| `text_turn_skips_frontend_provider` | テキスト入力で同じ recipe | `backchannel` は呼ばれず、`llm` だけが実行される |
| `frontend_output_is_not_forwarded_to_reasoner` | 受付が `{"ack":"working",...}` | `llm` の request の messages に受付の出力が含まれない |
| `ack_is_spoken_through_outer_event_hub` | 受付が `working` を返す | 外側の sink に `acknowledge_text("確認します。")` が 1 回届く。step 用 sink には届かない |
| `ack_speech_is_queued_before_answer_speech` | 同上 | `{run_id}_ack` の発話開始が `run_id` の発話開始より先 |
| `existing_reasoning_ack_is_unchanged` | reasoning-mcp 経路の既存テスト | 変更前と同じ文言・同じ待ち方 |

上の表のうち `voice_turn_*`、`frontend_*`、`text_turn_*`、`ack_*` は、0.1 のルールどおり `execute_conversation_turn_with_candidates` から実行する。

### チェックポイント

```sh
cargo test --manifest-path src-tauri/Cargo.toml role_routing
cargo test --manifest-path src-tauri/Cargo.toml conversation_inputs_roles
cargo test --manifest-path src-tauri/Cargo.toml
bun test
```

### Phase 3 完了後のユーザー作業（コード変更なし）

設定画面の Role Routing で次を設定する。これで今日の会話が Qwen 2B の受付と Ornith の思考に切り替わる。

- actor `larm-frontdesk`: transport `provider`、provider `dynamic-lan`、larmProvider `backchannel`、location `local`、resourceGroup `larm-backchannel`、maxInputBytes 16000、capabilities `social_reply`
- actor `larm-reasoner`: transport `provider`、provider `dynamic-lan`、larmProvider `llm`、location `local`、resourceGroup `larm-llm`、maxInputBytes 65536、capabilities `reason`, `tools`
- roles: frontend = `larm-frontdesk`、reasoner = `larm-reasoner`
- recipe: id `00-butler-respond`、action `respond`、roles `["frontend","reasoner"]`。id は辞書順で先頭になるように付ける（2.6 の条件 2）。Phase 0 で見つけた既存の respond recipe は削除せず、無効化するか id の順で後ろに置く
- Role Routing の `enabled` を ON にする（2.6 の条件 1）
- 適用後、12 章の SQL で確認する

設定画面で `larmProvider` を選べない場合は、Phase 4 の UI 追加が必要になる。その場合は Phase 4 の手順 1 を先に行う。

## 9. Phase 4: 設定 UI と既定値

ブランチ: `feat/butler-defaults`

1. 設定画面の Role Routing の actor 編集欄に「LARM Provider」（`llm` / `backchannel`）の選択を追加する。provider が `dynamic-lan` のときだけ表示する。対象ファイルは `rg -n "resourceGroup" src/features/settings` で特定する。
2. 新規インストール時の `routing.roles` の既定値（`rg -n "RoleRoutingSettings::default|fn default" src-tauri/src/role_routing/contracts.rs` で特定）を、8 章の「ユーザー作業」の構成にする。**既存の保存設定は migration で書き換えない。**
3. 設定画面に「執事構成（LARM: Qwen 2B 受付 + Ornith 思考）を適用」ボタンを追加する。押すと 8 章の構成（`enabled` ON と recipe id `00-butler-respond` を含む）を保存する。既存の actor は削除せず、同名の actor があれば上書き確認を出す。
4. テスト: 既定値が `validate_settings` を通ること、ボタンの保存内容が 8 章の構成と一致すること（`bun test`）。

## 10. Stage 2（Phase 3 と Phase 4 の完了後）

### Phase 5: 受付でターンを完了する

- `frontend` step の後、3.2 の Stage 2 の条件をすべて満たすとき、`respond` step を `cancelled` にし、ack の定型文を最終回答としてターンを完了する。
- 実装場所は `role_routing/repository_turns/advance.rs`（step の完了と次 step の扱い）。受付の出力で完了させる経路は README の「Specialist cannot publish the final answer」に触れるため、**着手前に、定型文だけを最終回答にしてよいかをユーザーに確認する。**
- テスト: 「なるほど」「ありがとう」で `llm` が呼ばれないこと、「ありがとう、あと明日の予定も」で `llm` が呼ばれること。

### Phase 6: Qwen 2B を tool_specialist にする

- 既存の recipe `[reasoner, tool_specialist, reasoner]` を使う（`recipe.rs`）。tool_specialist の actor を `larmProvider: "backchannel"` にする。
- Qwen 2B は Tool 操作を 1 つ JSON で選ぶだけ（既存の `role_step_request` の `"tool_specialist"` プロンプトと `execute_specialist_request`）。実行は SAAA、最終回答は Ornith。
- 状態変更を伴う Tool は、既存の Tool 実行ゲート（`tool_selection` の承認）に任せる。Qwen 2B の選択だけで不可逆な操作は確定しない。
- テスト: tool_specialist の request が `backchannel` に届くこと、Qwen 2B が不正な JSON を返したとき Tool が実行されず Ornith が最終回答を出すこと。

### Phase 7: 計測

- 受付と Ornith の TTFT（最初の可視文字まで）、decode 速度、総時間、担当 Provider を、role step 単位で記録する。記録先は既存の `rr_steps` とプロバイダセッションの記録（`providers/session_store.rs`）を優先する。
- reasoning 専用の delta と空の delta は、最初の文字として数えない。

## 11. ルーティング判断表

| 入力 | 受付（Qwen 2B） | 思考（Ornith） |
|---|---|---|
| 「なるほど」「はい」 | 「はい。」 | Stage 1 は実行、Stage 2 は実行しない |
| 「こんにちは」 | 「こんにちは。」 | Stage 1 は実行、Stage 2 は実行しない |
| 「ありがとう」 | 「どういたしまして。」 | Stage 1 は実行、Stage 2 は実行しない |
| 依頼・質問 | 「確認します。」。待ち時間が長いときは 10・20・30 秒に「まだ確認しています。」（最大 40 秒待つ） | 実行。結果は置き場に書き、ホストがそのまま最終回答として出す |
| 判断できない | 発話なし | 実行 |
| 受付の出力が不正、空、タイムアウト | 発話なし | 実行 |
| Tool が必要 | 「確認します。」 | 実行（Stage 2 では Qwen 2B が tool_specialist として Tool を 1 つ選び、Ornith が最終回答） |

## 12. 完了条件（Stage 1）

- Phase 1〜4 のチェックポイントがすべて通る。
- `role_routed_voice_turn_reaches_larm_for_every_step`、`voice_turn_runs_frontend_then_reasoner`、`frontend_failure_still_runs_reasoner`、`text_turn_skips_frontend_provider`、`ack_is_spoken_through_outer_event_hub` が通る。いずれも会話ターンの入口から実行している。
- `src-tauri/src/providers/larm_voice/frontdesk_decision.rs` と `frontdesk.rs` が `main` と同一である。
- 8 章のユーザー作業を行ったあと、実機の音声会話で次を確認できる（ユーザーが確認する）。
  - 依頼を話すと、すぐに「確認します。」が流れ、その後 Ornith の回答が流れる。
  - DB のコピーで `SELECT ordinal, purpose, actor_id, status FROM rr_steps WHERE root_id=<直近の音声 run_id> ORDER BY ordinal` が `0 frontend larm-frontdesk succeeded`、`1 respond larm-reasoner succeeded` になる。行がない、または actor が違う場合は 2.6 の条件を上から順に確認する。

## 13. リスク

| リスク | 対策 |
|---|---|
| Role Routing が無効、または既存 recipe が先に選ばれ、受付が動かない（前回と同じ種類の失敗） | Phase 0 で `enabled` と respond recipe の並びを確認する。8 章で id と `enabled` を明示的に設定する。12 章の SQL で実際の step を確認する |
| ack を step 用 sink に送って無音になる | 2.7 のとおり外側の `on_event` に送る。`ack_is_spoken_through_outer_event_hub` で確認する |
| ack の発話完了を待って Ornith の開始が遅れる | `wait_end = false` で読み上げを渡すだけにする |
| 受付 step が直列なので、Ornith の開始が受付の分だけ遅れる | 実測で受付の TTFT は 70ms、全体でも数百 ms。`frontend_timeout_ms`（1,200ms）で上限を切る。遅延が問題になれば、受付と Ornith の並列実行を別計画で検討する（`steps.rs` には frontend 用の別 slot の考え方がある） |
| Stage 1 では相槌にも Ornith が動く | Stage 2（Phase 5）で解消する。Stage 1 ではまず 2 つの LLM が連携して応答する状態を優先する |
| 保存設定が DeepSeek のまま | コードでは書き換えない。8 章のユーザー作業、または Phase 4 のボタンで適用する |
