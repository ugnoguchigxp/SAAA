# Ornith 1.5 35B + Qwen 3.5 2B 執事エージェント移行 実装計画

> **この計画は破棄済み。** 受付を `larm_voice/frontdesk*` に実装する前提だったが、その経路は現在の UI から到達しない（音声は通常ターンとして Role Routing を通る）。後継は `docs/plans/saaa-butler-role-routing.md`。

作成日: 2026-09-24（LARM 対応完了報告を反映した第 3 版）
状態: 計画確定、未着手

## 0. 実装者への指示

- Phase 1 から順に実装する。各 Phase の手順を上から順に行い、チェックポイントのコマンドが通るまで次の Phase に進まない。
- 「決定事項」の値と名前はそのまま使う。書かれていない設計判断が必要になったら、実装を止めて質問する。
- 「やらないこと」に書かれた操作はしない。
- 作業ツリーには本件と関係のない未コミット変更（`tool_selection/**`、`artifact_preview/**`、`chat_completions/mod.rs`、`voice_progress.rs` など）がある。それらの差分は触らない。Phase ごとにブランチを切り、この文書に列挙したファイルだけを stage する（`git add -p` で自分の hunk だけを選ぶ）。
- 行番号は 2026-09-24 時点のもの。ずれていたら、示した関数名や文字列で検索する。

### 実施順序

最初のゴールは「Qwen 2B が受付し、必要なときに Ornith へ引き継いで、2 つの LLM でユーザーの依頼に応える」状態にすること。そのため Stage 1 だけを先に実装し、完了してから Stage 2 に進む。

| Stage | Phase | 内容 | この Stage での扱い |
|---|---|---|---|
| 1 | Phase 1 | `larm-session` で canonical を選び、`backchannel` を含む 5 Provider を claim する | 全部実施 |
| 1 | Phase 3 | Qwen 2B の受付と、Ornith への昇格 | 全部実施。ただし手順 3 の `first_visible_at` は実装しない（Stage 2 の Phase 5 で追加） |
| 1 | Phase 4 | 音声会話の Ornith と受付に `enable_thinking=false` を付ける | 全部実施 |
| 2 | Phase 6 | Qwen 2B の Tool 候補を Ornith に渡す | Stage 1 完了後 |
| 2 | Phase 5 | TTFT などの計測 | Stage 1 完了後 |
| 2 | Phase 2 | テキスト会話経路（`dynamic_lan`）の Profile 選択 | Stage 1 完了後 |
| 2 | Phase 7 | 既定値の整理 | 最後 |

Stage 1 では、14 章のテストのうち #1〜#9 の音声経路（`larm-session` と `larm_voice`）の分だけを対象にする。`text_path_*` のテストと #10（TTFT）は Stage 2 で追加する。

Stage 1 の完了条件:

- Phase 1、3、4 のチェックポイントがすべて通る。
- `world_tests.rs` に次の通しテスト `butler_turn_escalates_from_backchannel_to_ornith` を追加し、通る。
  - fixture の LARM は 5 Provider を返す。
  - 1 発言目「なるほど」: `backchannel` だけが呼ばれ、定型文「なるほど。」を返す。`llm` は呼ばれない。
  - 2 発言目「明日の予定を確認して、空いている時間に会議を入れて」: `backchannel` が `delegate` を返し、`think == true` になる。その後の会話本体の request が `llm` に届き、body に `enable_thinking: false` が入っている。
  - 3 発言目: `backchannel` が不正な JSON を返しても、同じく `llm` に引き継がれる。
  - 会話 session を閉じると、fixture の lease カウンタが 0 に戻る。

## 1. 目的

LARM の Agent Profile `saaa-conversation-ornith15` へ SAAA を移行し、次の分担の執事エージェントにする。

- 一次受付: Qwen 3.5 2B（Provider 名 `backchannel`）。相槌、発話継続判定、単純な意図分類、単一の低リスク Tool 候補の提示。
- 思考・Tool use・メモリー・world model・最終回答: Ornith 1.5 35B（Provider 名 `llm`）。

## 2. LARM 側の状態（2026-09-24 報告）

| 項目 | 内容 |
|---|---|
| canonical Profile | `saaa-conversation-ornith15`。Provider は `backchannel`（Qwen 3.5 2B / 64K）、`llm`（Ornith 1.5 35B / 128K / MTP）、`asr`、`tts`、`embedding` |
| 互換 Profile | `saaa-qwen38`。Gemma 4 構成の凍結レガシーとして復元済み。Qwen 3.8 ではない |
| 既存 Profile | `saaa-conversation-gemma4`。SAAA 音声経路の現在の既定値 |
| Catalog | `GET /v3/agent-profiles` だけを使う。v1 は旧形式。v2 は `contextWindow` と `embeddingSpace` を返さない |
| Claim | `POST /v1/agent-connections/{id}/claim`、body は `{"format":"openai-provider-v1"}`。Connection ごとに 1 回。全 Provider が `providers[]` にまとめて返る。必ず `name` で検索する |
| Embedding | `apiStyle: larm-embedding`、`protocol: larm.embedding.v1`、384 次元 |
| Renew / Release | Connection 単位 |
| Ornith の通常応答 | request に `chat_template_kwargs.enable_thinking=false` を付ける |
| 実測 | Qwen 2B の相槌は TTFT 70.5ms、Qwen 2B の Tool call は 429.6ms、Ornith 通常応答は TTFT 154.9ms / 75.1 tok/s、Embedding 30ms、TTS 445ms、ASR 607ms |
| Media variant | `music`（ACE-Step）と `image`（Qwen-Image）は LARM 側の sudo スクリプトで排他的に切り替える。基本セットは維持される |
| 本番 | 新 release は隔離環境の E2E まで完了。本番 systemd は旧 release のまま。**本番の LARM には現時点で `saaa-conversation-ornith15` が存在しない** |

## 3. 現行コードの構造（前提として理解すること）

SAAA には LARM への接続経路が 2 つある。**執事の本体は経路 A**。

### 経路 A: 音声会話セッション（`crates/larm-session`）

- `crates/larm-session/src/lib.rs` の `Session`。1 つの Connection で複数 Provider を claim し、`acquire(name)` で Provider を貸し出す。renew（期限 90 秒前、`ttlSeconds: 600`）、`close()`（冪等）、Drop 時の release をすでに実装している。
- `crates/larm-session/src/contract.rs`
  - `PROVIDERS`: 必須 Provider は `tts`、`asr`、`llm`、`embedding` の 4 つ。
  - `DEFAULT_PROFILE = "saaa-conversation-gemma4"`。
  - `parse()` はすでに `name` で検索している（`providers[0]` に依存しない）。`PROVIDERS` にない name は無視する。`contextWindow` は `llm` にだけ要求する。
  - Catalog は取得していない。Profile は呼び出し側が文字列で渡す。
- `claim()` は `PROVIDERS` の全 Provider に health check を行う。
- Profile の決定: 保存設定の `harness.larmProfile`。未設定なら `DEFAULT_PROFILE`。参照箇所は次のとおり。
  - `src-tauri/src/providers/larm_voice/mod.rs`（53〜55 行目、196〜198 行目）
  - `src-tauri/src/voice/tts_catalog.rs:276`
  - `src-tauri/src/diagnosis/checks/harness.rs:71`
  - `src-tauri/src/memory/personal_state/product_binding.rs:25`
  - `src-tauri/src/harness_llm_diagnostic.rs:15`
  - `src-tauri/src/persistence/settings_defaults.rs:17`
  - `src-tauri/src/role_routing/schema.rs:206, 571`
  - `src/features/settings/settingsDefaults.ts:7`
- 利用箇所:
  - 受付の分類: `src-tauri/src/providers/larm_voice/frontdesk_decision.rs`。`respond_with_lfm` と `classify_with_qwen` が**どちらも `acquire("llm")`** を使っている。`stream:false`。
  - 会話本体: `src-tauri/src/providers/stream/larm_voice.rs` の `stream_larm_voice_provider`。`acquire("llm")` を呼び、`stream_model_provider_with_api_key` → `chat_completions/mod.rs` で streaming する。
  - メモリー: `memory/personal_state/product_binding.rs` が `acquire("llm")`、`diagnosis/checks/harness.rs` などが `embed_query`。
  - ASR: `voice/session/asr_routes.rs:67` の `acquire("asr")`。
  - TTS: `voice/http_audio/requests.rs:51` と `voice/tts_catalog.rs:285` の `acquire("tts")`。

### 経路 B: テキスト会話（`src-tauri/src/providers/dynamic_lan`）

- 音声セッションがないときのテキスト会話で使う（`stream/larm_voice.rs` の `stream_voice_aware_dynamic_lan_provider` が振り分ける）。
- `GET v3/agent-profiles` → `saaa-qwen38`（`mod.d/01.rs:2` の `AGENT_PROFILE`）→ Connection → claim → `llm` だけを使う。
- 問題点:
  - `validate.rs` の `select_default_llm_profile` が、`profile.providers.len() != 1` のときエラーにし、`.first()` を LLM とみなしている。
  - `validate.rs` の `validate_state_shape` が、`state.providers.len() != 1` のときエラーにし、`state.providers[0]` を検証している。
  - このため 5 Provider の Profile を選ぶと接続に失敗する。
  - `saaa-qwen38` は現在 Gemma 4 なので、何もしないとテキスト会話は Gemma 4 のまま動く。

### その他

- `crates/larm-session/src/http_api.rs` の `LlmOptions::apply` は `chat_template_kwargs` を毎回削除する。model 名に `qwen` を含むと `{"reasoning_effort": ...}` を入れる。`enable_thinking` を扱う仕組みはない。
- 既存テストの `crates/larm-session/tests/live.rs` は `#[ignore]` 付きで、環境変数 `SAAA_LARM_CONTROL_URL` と `LARM_API_TOKEN` で実機につなぐ。

## 4. 決定事項

### 4.1 Profile 選択

| 項目 | 値 |
|---|---|
| 定数（`crates/larm-session/src/contract.rs`） | `CANONICAL_PROFILE = "saaa-conversation-ornith15"`、`LEGACY_PROFILE = "saaa-qwen38"`、`PREVIOUS_DEFAULT_PROFILE = "saaa-conversation-gemma4"`。`DEFAULT_PROFILE` は `CANONICAL_PROFILE` と同じ値にする |
| 自動選択の対象 | 保存設定 `harness.larmProfile` が未設定、`saaa-conversation-gemma4`、`saaa-conversation-ornith15` のいずれか。これ以外の値はユーザーが明示的に選んだものとして、そのまま使う（自動選択しない） |
| 経路 A の自動選択順 | catalog の中で、必須 Provider（4.2）を**すべて catalog 上で宣言している** Profile を次の順に探す。1) `saaa-conversation-ornith15` 2) `saaa-qwen38` 3) `saaa-conversation-gemma4` |
| 経路 B の自動選択順 | 1) `saaa-conversation-ornith15` 2) `saaa-qwen38`。`llm` を宣言していれば候補になる |
| 見つからない場合 | 経路 A はエラーコード `larm_profile_unavailable`。経路 B は既存の `harness-profile-missing` 系 |
| `defaultAgentProfile` | 無視する |
| Profile ID からの推測 | しない。model、endpoint、credential、context window はすべて claim 応答から取る |

経路 A で 3 番目に `saaa-conversation-gemma4` を置く理由: 本番の LARM はまだ旧 release で、canonical がない。依頼では「canonical がない場合だけ `saaa-qwen38`」だが、旧 release の `saaa-qwen38` が音声に必要な 4 Provider を持っている保証はない。持っていなければ、現在動いている `saaa-conversation-gemma4` を使わないと本番の音声会話が止まる。「必須 Provider を catalog 上で宣言していること」を条件にしているので、新 release では 2 番目の `saaa-qwen38`（Gemma 4 凍結レガシー）が必ず先に選ばれ、依頼の意図と矛盾しない。

### 4.2 必須 Provider（経路 A）

| 選ばれた Profile | 必須 | 任意 |
|---|---|---|
| `saaa-conversation-ornith15` | `llm`、`backchannel`、`asr`、`tts`、`embedding` | なし |
| それ以外 | `llm`、`asr`、`tts`、`embedding` | `backchannel` |

- `backchannel` の protocol は `openai.chat-completions.v1`。`contextWindow` を必須にする（`llm` と同じ検証）。
- claim 時の health check は、claim 応答に実際に含まれ、かつ受け付けた Provider 全部に行う。

### 4.3 受付（Frontdesk）

| 項目 | 値 |
|---|---|
| 受付に使う Provider | `session.has_provider("backchannel")` なら `backchannel`。なければ現状どおり `llm` |
| 呼び出し回数 | 1 発話につき 1 回（`respond_with_lfm` と `classify_with_qwen` の並列実行はやめる） |
| streaming | `stream: true` |
| thinking | `chat_template_kwargs: {"enable_thinking": false}` を付ける |
| タイムアウト | 1,500 ms（定数 `FRONTDESK_TIMEOUT`）。実測では相槌 70ms、Tool call 430ms なので十分に余裕がある |
| `max_tokens` | 128 |
| `temperature` | 0.0 |
| 履歴 | 直近 4 ターン（定数 `FRONTDESK_HISTORY_TURNS`）と現在の発言 |
| 入力上限 | 16,000 bytes（`FRONTDESK_MAX_INPUT_BYTES`）。超えたら呼ばずに Ornith へ昇格させる |

### 4.4 Ornith

| 項目 | 値 |
|---|---|
| 経路 A の会話本体（`stream_larm_voice_provider`） | `chat_template_kwargs: {"enable_thinking": false}` を付ける |
| 経路 A のメモリー抽出（`memory/personal_state/**`）、reasoning-mcp の委譲推論 | 変更しない（LARM のテンプレート既定のまま） |
| 経路 B（テキスト会話） | 変更しない |
| 実装方法 | `LlmOptions` に `thinking: Thinking`（`Auto` / `Disabled` / `Enabled`、既定値は `Auto`）を追加する。`Disabled` なら `apply()` の最後で `body["chat_template_kwargs"] = {"enable_thinking": false}` とし、qwen 用の `reasoning_effort` 設定は上書きする。`Enabled` なら `true`。`Auto` なら現状の挙動 |

## 5. やらないこと

- production への切替、LARM service の起動・停止・再起動、Qwen 2B の load / unload 要求。
- `runtime-variant.sh` の実行、media variant（music / image）の切替要求。SAAA からは variant を操作しない。
- 保存済みユーザー設定の削除や書き換え（4.1 の自動選択は実行時に解決する。settings の値は書き換えない）。
- `saaa-qwen38` という ID からモデル名や context window を推測するコード。
- `providers[0]`、`.first()`、`providers.len() == N` による Provider の特定。
- Qwen 2B の出力だけで Tool を実行すること。
- `voice/**` の ASR 処理ロジックの変更（Phase 5 で時刻を記録する 1 行の呼び出し追加だけは許可する）。

## 6. Phase 1: `larm-session` の Profile 自動選択と `backchannel`

ブランチ: `feat/ornith15-session`

### 変更ファイル

- `crates/larm-session/src/contract.rs`
- `crates/larm-session/src/lib.rs`
- `crates/larm-session/src/catalog.rs`（新規）
- `crates/larm-session/src/tests.rs`
- `src-tauri/src/providers/larm_voice/mod.rs`
- `src-tauri/src/voice/tts_catalog.rs`
- `src-tauri/src/diagnosis/checks/harness.rs`
- `src-tauri/src/memory/personal_state/product_binding.rs`
- `src-tauri/src/harness_llm_diagnostic.rs`
- `src-tauri/src/providers/larm_voice/profile.rs`（新規）

### 手順

1. **定数**（`contract.rs`）
   - 4.1 の 3 つの定数を追加し、`DEFAULT_PROFILE` を `CANONICAL_PROFILE` と同じ値にする。
   - `PROVIDERS` を次の 2 つに分ける。
     ```rust
     pub const BASE_PROVIDERS: [(&str, &str); 4] = [
         ("tts", "openai.audio-speech.v1"),
         ("asr", "openai.audio-transcriptions.v1"),
         ("llm", "openai.chat-completions.v1"),
         ("embedding", "larm.embedding.v1"),
     ];
     pub const BACKCHANNEL: (&str, &str) = ("backchannel", "openai.chat-completions.v1");
     pub fn required_providers(profile: &str) -> Vec<&'static str>; // 4.2 の表どおり
     ```
   - `PROVIDERS` を参照している箇所（`rg -n "PROVIDERS" crates src-tauri services`）をすべて追従させる。
2. **`parse()` の変更**（`contract.rs`）
   - シグネチャを `parse(value: Value, id: &str, required: &[&str]) -> Result<Snapshot, &'static str>` にする。
   - 受け付ける name は `BASE_PROVIDERS` と `BACKCHANNEL`。それ以外は現状どおり無視する。
   - `contextWindow` は `name == "llm" || name == "backchannel"` のとき必須にする。
   - 最後の欠落チェックは `required` に対して行う。欠けていたら `larm_missing_provider`。
3. **Catalog**（新規 `catalog.rs`）
   ```rust
   pub struct CatalogProfile { pub id: String, pub provider_names: Vec<String> }
   pub(crate) async fn fetch(client: &reqwest::Client, control_base: &url::Url, token: &str)
       -> Result<Vec<CatalogProfile>, &'static str>;
   pub fn select_voice_profile(profiles: &[CatalogProfile]) -> Result<String, &'static str>;
   ```
   - `fetch`: `GET {control_base}/v3/agent-profiles`、Bearer 認証、200 のみ受け付ける。`contractVersion == "agent-connection.v3"` でなければ `larm_catalog_unsupported`。`profiles[].id` と `profiles[].providers[].name` だけを読む。
   - `select_voice_profile`: 4.1 の順に、`required_providers(id)` の全 name を `provider_names` に含む最初の Profile を返す。同じ id が 2 件以上あれば `larm_catalog_invalid`。見つからなければ `larm_profile_unavailable`。
4. **Session**（`lib.rs`）
   - 公開 enum を追加する。
     ```rust
     pub enum ProfilePreference { Auto, Explicit(String) }
     ```
   - `connect_with_profile_credential_and_key` の `profile: &str` 引数を `preference: ProfilePreference` に変える。`connect_inner` の最初（Connection 作成の前）で、`Auto` なら `catalog::fetch` と `select_voice_profile` を行って Profile を決める。`Explicit` なら現状どおりその文字列を使う。
   - `connect`、`connect_with_profile`、`connect_with_profile_and_credential` は `Explicit(profile)` を渡すラッパーとして残す。
   - `Session` に `profile: String` と `required: Vec<&'static str>` を持たせ、`claim()` と renew 後の再 claim で `parse(value, &self.id, &self.required)` を使う。
   - `claim()` の health check を、`snapshot.providers` の全要素に対して行うよう変える。
   - アクセサを追加する: `pub fn profile_id(&self) -> &str`、`pub async fn has_provider(&self, name: &str) -> bool`（現在の snapshot に含まれるか）。
5. **SAAA 側の Profile 解決**（新規 `src-tauri/src/providers/larm_voice/profile.rs`）
   ```rust
   pub(crate) fn preference(stored: Option<&str>) -> saaa_larm_session::ProfilePreference
   ```
   `None`、`saaa-conversation-gemma4`、`saaa-conversation-ornith15` なら `Auto`、それ以外は `Explicit(stored)`。
6. 3 章に挙げた「Profile の決定」の参照箇所（`larm_voice/mod.rs`、`tts_catalog.rs`、`diagnosis/checks/harness.rs`、`product_binding.rs`、`harness_llm_diagnostic.rs`）で、`unwrap_or(DEFAULT_PROFILE)` を `profile::preference(stored)` に置き換え、新しい `connect_*` に渡す。`larm_voice/mod.rs` の `Owner.profile: String` には、比較用の文字列として `Auto` なら `"*"`（profile id として不正な印）、`Explicit` ならその値を入れる。保存値が `"auto"` のときは明示選択のままにする。
7. `settings_defaults.rs`、`settingsDefaults.ts`、`role_routing/schema.rs` の既定値は**この Phase では変えない**（保存値 `saaa-conversation-gemma4` は自動選択の対象なので、変えなくても canonical が選ばれる）。

### 追加テスト（`crates/larm-session/src/tests.rs`）

既存の fixture server（`tests.rs` 内の claim 応答生成）に、`GET /v3/agent-profiles` の応答と、claim で返す Provider の集合を切り替える仕組みを追加する。

| テスト名 | 内容 | 期待値 |
|---|---|---|
| `auto_selects_canonical_ornith15_profile` | catalog に canonical（5 Provider）、`saaa-qwen38`（4 Provider）、gemma4 | create body の `agentProfile` が `saaa-conversation-ornith15` |
| `auto_falls_back_to_legacy_only_when_canonical_absent` | catalog に `saaa-qwen38` と gemma4 | `saaa-qwen38` |
| `auto_skips_profile_missing_required_providers` | catalog に canonical（`embedding` なし）と `saaa-qwen38`（4 Provider） | `saaa-qwen38` |
| `auto_uses_previous_default_when_only_it_exists` | catalog に gemma4 のみ | `saaa-conversation-gemma4` |
| `auto_fails_without_any_candidate` | catalog に関係ない Profile のみ | `larm_profile_unavailable`。Connection は作られない |
| `explicit_profile_skips_catalog` | `Explicit("custom-x")` | catalog を GET しない。`agentProfile == "custom-x"` |
| `legacy_profile_model_comes_from_claim` | `saaa-qwen38` を選び、claim の `llm.model` を `gemma-4-fixture` にする | `acquire("llm")` の `provider().model == "gemma-4-fixture"` |
| `no_qwen38_literal_in_sources` | `include_str!` で `contract.rs`、`lib.rs`、`catalog.rs` を読む | `qwen3.8` と `qwen-3.8` を含まない |
| `claims_five_providers_in_any_order` | claim の `providers[]` を `embedding, tts, llm, asr, backchannel` の順にする | 5 つすべて `acquire` でき、それぞれの token と base_url が正しい |
| `canonical_requires_backchannel` | canonical で claim に `backchannel` がない | `larm_missing_provider`。release を 1 回受信 |
| `legacy_backchannel_is_optional` | `saaa-qwen38` で `backchannel` なし | 成功。`has_provider("backchannel") == false` |
| `backchannel_requires_context_window` | `backchannel` の `contextWindow` なし | `larm_missing_context_window` |
| `renew_reclaims_all_five_and_release_leaves_no_lease` | fixture が create で +1、DELETE で −1 するカウンタを持つ。connect → renew（期限を 60 秒後にして発火させる）→ close → close | renew 1 回、claim 2 回、カウンタ 0。2 回目の close もエラーにならない |

`src-tauri/src/providers/larm_voice/profile.rs` の単体テスト:

| テスト名 | 入力 | 期待値 |
|---|---|---|
| `preference_auto_for_shipped_values` | `None`、`"saaa-conversation-gemma4"`、`"saaa-conversation-ornith15"` | `Auto` |
| `preference_respects_operator_choice` | `"custom-x"` | `Explicit("custom-x")` |

### チェックポイント

```sh
cargo test --manifest-path crates/larm-session/Cargo.toml
cargo test --manifest-path src-tauri/Cargo.toml larm
cargo test --manifest-path src-tauri/Cargo.toml
cargo test --manifest-path services/reasoning-mcp/Cargo.toml   # larm-session を使う別クレート
```

## 7. Phase 2: テキスト会話経路（`dynamic_lan`）の Profile 選択

ブランチ: `feat/ornith15-dynamic-lan`

経路 B は `llm` だけを使う。Provider 束の管理は経路 A（`larm-session`）に任せ、ここでは「5 Provider Profile を拒否しない」「canonical を選ぶ」だけを直す。

### 変更ファイル

- `src-tauri/src/providers/dynamic_lan/mod.d/01.rs`
- `src-tauri/src/providers/dynamic_lan/validate.rs`
- `src-tauri/src/providers/dynamic_lan/profile_catalog.rs`
- `src-tauri/src/providers/dynamic_lan/mod.d/04.rs`、`mod.d/05.rs`（テスト）
- `src-tauri/src/providers/larm_voice/world_wire_fixture.rs`（テスト fixture）
- `AGENT_PROFILE` の参照箇所（`rg -n "AGENT_PROFILE" src-tauri/src`）

### 手順

1. `mod.d/01.rs`: `AGENT_PROFILE` を削除し、`CANONICAL_AGENT_PROFILE = "saaa-conversation-ornith15"` と `LEGACY_AGENT_PROFILE = "saaa-qwen38"` を追加する。参照箇所をすべて追従させる。
2. `validate.rs` の `select_default_llm_profile`:
   - `defaultAgentProfile` を使わない。canonical、legacy の順に `profile.id` で検索する。どちらもなければ既存のエラーを返す。
   - `profile.providers.len() != 1` と `.first()` を削除する。`profile.providers` から `name == "llm"` の要素をちょうど 1 つ探す（0 個または 2 個以上なら contract error）。他の name は無視する。
   - それ以外の検証（capability、protocol、contextWindow）はそのまま、見つけた `llm` に対して行う。
3. `validate.rs` の `validate_state_shape`:
   - `state.providers.len() != 1` と `state.providers[0]` を削除する。`name == "llm"` の要素をちょうど 1 つ探し、既存の検証をその要素に適用する。他の Provider は無視する。
4. `validate_claim` はすでに `name == "llm"` で検索しているので変更しない。
5. `profile_catalog.rs`: v1 用の `legacy_profile_context_window` と、`validate.rs` 内の v1 分岐を削除する。contract version は `agent-connection.v3` だけを受け付ける。
6. テスト fixture:
   - `mod.d/04.rs`、`05.rs` で `claim["providers"][0]` を書き換えている箇所は、`name` で要素を探すヘルパー `fn provider_mut<'a>(claim: &'a mut Value, name: &str) -> &'a mut Value` に置き換える。
   - fixture の catalog と Connection state を、canonical の 5 Provider 構成にする。
   - `requests[1].contains("\"agentProfile\":\"saaa-qwen38\"")` は canonical を期待する形に変える。
   - `profile_catalog.rs` のテスト内と `world_wire_fixture.rs` の `"qwen3.8"`、`"qwen3.8-27b"` を、`"ornith-1.5-35b"` などの canonical 前提の値に置き換える。

### 追加テスト（`mod.d/04.rs`）

| テスト名 | 内容 | 期待値 |
|---|---|---|
| `text_path_selects_canonical_profile` | catalog に canonical と legacy | `agentProfile` が canonical |
| `text_path_falls_back_to_legacy_only_when_canonical_absent` | catalog に legacy のみ | legacy |
| `text_path_accepts_five_provider_state_in_any_order` | state と claim の `providers[]` を `tts, llm, embedding, backchannel, asr` の順にする | 接続成功。`model()` が claim の `llm.model` |
| `text_path_rejects_duplicate_llm` | `llm` が 2 件 | contract error、release 1 回 |

### チェックポイント

```sh
cargo test --manifest-path src-tauri/Cargo.toml dynamic_lan
cargo test --manifest-path src-tauri/Cargo.toml
rg -n 'providers\[0\]|providers\.first\(\)|len\(\) != 1' src-tauri/src/providers/dynamic_lan/validate.rs   # 0 件
```

## 8. Phase 3: Qwen 2B の一次受付

ブランチ: `feat/ornith15-frontdesk`

### 変更ファイル

- `src-tauri/src/providers/larm_voice/frontdesk_decision.rs`
- `src-tauri/src/providers/larm_voice/escalation.rs`（新規）
- `src-tauri/src/providers/larm_voice/mod.rs`（`mod escalation;` の追加）
- `src-tauri/src/providers/larm_voice/frontdesk_repository_tests.rs`、`world_tests.rs`（テスト）

### 手順

1. **出力 schema**（`frontdesk_decision.rs`）。既存の `FrontdeskClassification`、`Route`、`ReplyKey`、`ReasoningNeed` を次に置き換える。
   ```rust
   #[derive(Debug, Clone, Deserialize, PartialEq)]
   #[serde(rename_all = "camelCase", deny_unknown_fields)]
   struct FrontdeskOutput {
       route: Route,          // backchannel | wait | simple_reply | tool_candidate | delegate
       reply_key: ReplyKey,   // none | greeting | acknowledgement | nod
       tool: Option<String>,  // route == tool_candidate のときだけ Some
       confidence: Confidence // high | low
   }
   ```
   request の `response_format` の JSON schema も同じ形にする（`strict: true`、`additionalProperties: false`、`tool` は `["string","null"]`）。
2. **Provider の選択**: `decide()` の最初で `ready.session.has_provider("backchannel").await` を見て、`"backchannel"` か `"llm"` を決める（4.3）。
3. **1 回の streaming 呼び出し**: `respond_with_lfm` と `classify_with_qwen` を削除し、`classify_with_frontdesk(ready, provider_name, history, ctx)` を 1 つ作る。
   - body: `{"model": provider.model, "messages": ..., "stream": true, "max_tokens": 128, "temperature": 0.0, "chat_template_kwargs": {"enable_thinking": false}, "response_format": ...}`
   - SSE を読み、`choices[0].delta.content` を連結する。`reasoning_content` は捨てる。全体を `FRONTDESK_TIMEOUT` で囲む。
   - 400 / 422 が返ったら `response_format` を外して 1 回だけ再送する（既存の `structured_output_fallback` の挙動を維持）。
   - 最初の可視文字の時刻を Phase 5 で使うので、`first_visible_at: Option<Instant>` を戻り値に含める（判定は Phase 5 の `is_visible_delta` を使う。Phase 3 の時点では、空白以外の `content` が初めて来た時刻で仮実装してよい）。
4. **INSTRUCTION** を新 schema に合わせて書き直す。含める内容:
   - 無音 1.5 秒ごとに発言が届き、無音は依頼の完了を意味しない（既存文言を維持）。
   - 短い相槌・同意には `backchannel` + `nod`。挨拶には `simple_reply` + `greeting`、お礼には `simple_reply` + `acknowledgement`。
   - 話の途中なら `wait`。
   - 利用可能 Tool の一覧（name と `read_only` かどうか）を渡す。1 つの read_only Tool で明らかに済む依頼だけ `tool_candidate` とし、`tool` に name を入れる。
   - 少しでも推論、曖昧さの解消、複数の手順、状態変更が必要なら `delegate`。
   - 自信がなければ `confidence: low`。
   - JSON 以外の出力は禁止。
   Tool 一覧は、Frontdesk を呼ぶ側が既に持っている Tool 定義から name と read_only フラグを取って渡す。`receive_lfm_utterance` は Tool catalog を持たないので、Stage 1 は空一覧のままにする（`tool_candidate` は `UnknownTool` で昇格する）。Phase 6 の前に、catalog の `name` と `effect`（`pure` / `read` だけ read_only）を受付へ渡す。
5. **昇格判定**（新規 `escalation.rs`、HTTP に依存しない純粋関数）
   ```rust
   pub(crate) enum Verdict {
       Backchannel { reply_key: ReplyKey },
       Wait,
       SimpleReply { reply_key: ReplyKey },
       ToolCandidate { tool: String },
       Delegate { reason: EscalationReason },
   }
   pub(crate) enum EscalationReason {
       Timeout, RequestFailed, Empty, ParseError, SchemaViolation,
       InputTooLarge, LowConfidence, UnknownTool, SideEffectTool, RequestedByModel,
   }
   pub(crate) struct JudgeContext<'a> {
       pub pending_reasoning: bool,
       pub input_bytes: usize,
       pub tools: &'a [(String, bool)], // (name, read_only)
   }
   pub(crate) fn judge(raw: Result<&str, FrontdeskCallError>, ctx: &JudgeContext) -> Verdict;
   ```
   判定は次の順で、最初に当てはまったものを返す。
   1. `ctx.pending_reasoning` → `Wait`（呼び出し前に判定し、Qwen 2B を呼ばない。`think = false`、`say = None`。推論中の追加発言で Ornith を二重に起動しない）
   2. `ctx.input_bytes > FRONTDESK_MAX_INPUT_BYTES` → `Delegate{InputTooLarge}`（同上）
   3. タイムアウト → `Delegate{Timeout}`、その他の呼び出し失敗 → `Delegate{RequestFailed}`
   4. 空文字または空白のみ → `Delegate{Empty}`
   5. JSON として読めない → `Delegate{ParseError}`
   6. `FrontdeskOutput` に変換できない、`route == tool_candidate` なのに `tool == None`、`route != tool_candidate` なのに `tool == Some` → `Delegate{SchemaViolation}`
   7. `confidence == low` → `Delegate{LowConfidence}`
   8. `route == tool_candidate`: Tool が `ctx.tools` にない → `Delegate{UnknownTool}`。`read_only == false` → `Delegate{SideEffectTool}`。それ以外 → `ToolCandidate`
   9. `route == delegate` → `Delegate{RequestedByModel}`
   10. それ以外は route どおり
6. **既存の出力型への対応**: `ConversationDecision` の形は変えず、フィールドを 2 つ足す。
   ```rust
   #[serde(skip, default)] pub tool_hint: Option<String>,
   #[serde(skip, default)] pub escalation: Option<&'static str>,
   ```
   `Verdict` からの変換は次のとおり。
   - `Backchannel` / `SimpleReply`: 既存の定型文テーブル（`frontdesk_decision.rs` の reply 選択関数）から `say` を選び、`think = false`。`nod` 用に「はい。」「なるほど。」を追加する。`Backchannel` が定型表に当たらないときは `Wait` と同じ（無言、`think = false`）。`SimpleReply` が定型表に当たらないときは Ornith へ昇格する。挨拶の繰り返し（`already_greeted`）は無言のまま。
   - `Wait`: `say = None`、`think = false`。
   - `ToolCandidate`: `think = true`、`tool_hint = Some(tool)`。
   - `Delegate`: `think = true`、`escalation = Some(reason の snake_case 名)`。
   既存の `apply_reasoning_need` と `already_greeted` の扱いは、`think` の決め方を上の変換に置き換えたうえで残す。
7. `tool_hint` は、この Phase では trace とログに出すだけにする。Ornith の request に渡す処理は Phase 6 で行う。
8. ファイル内の「LFM」表記（エラーコードの `lfm-` 接頭辞を含む）を `frontdesk-` に改める。DB のテーブル名 `lfm_voice_utterances` は変えない。

### 追加テスト

`escalation.rs` の単体テスト（HTTP なし）:

| テスト名 | 入力 | 期待値 |
|---|---|---|
| `nod_for_short_acknowledgement` | `{"route":"backchannel","replyKey":"nod","tool":null,"confidence":"high"}` | `Backchannel{nod}` |
| `wait_while_mid_utterance` | `route: wait` | `Wait` |
| `read_only_tool_becomes_candidate` | `route: tool_candidate`、`tool: "current_time"`、tools に `("current_time", true)` | `ToolCandidate{"current_time"}` |
| `side_effect_tool_is_escalated` | tools に `("send_message", false)` で `tool: "send_message"` | `Delegate{SideEffectTool}` |
| `unknown_tool_is_escalated` | tools に存在しない name | `Delegate{UnknownTool}` |
| `complex_request_is_delegated` | `route: delegate` | `Delegate{RequestedByModel}` |
| `low_confidence_is_escalated` | `confidence: low` | `Delegate{LowConfidence}` |
| `invalid_json_escalates` | `はい、了解です` | `Delegate{ParseError}` |
| `unknown_field_escalates` | 余分なフィールド付き | `Delegate{SchemaViolation}` |
| `tool_without_candidate_route_escalates` | `route: backchannel` かつ `tool: "x"` | `Delegate{SchemaViolation}` |
| `empty_escalates` | `""` と `"  "` | `Delegate{Empty}` |
| `timeout_escalates` | `Err(Timeout)` | `Delegate{Timeout}` |
| `pending_reasoning_stays_silent_without_call` | `pending_reasoning = true` | `Wait`。`ConversationDecision.think == false` かつ `say == None` |

`world_tests.rs`（fixture server 使用）:

| テスト名 | 内容 | 期待値 |
|---|---|---|
| `frontdesk_uses_backchannel_provider_when_present` | 5 Provider の session | 受付の request が `backchannel` の base_url と token に届く。`llm` には届かない |
| `frontdesk_falls_back_to_llm_without_backchannel` | 4 Provider の session | 受付の request が `llm` に届く |
| `frontdesk_request_is_streaming_without_thinking` | 受付の request body | `stream == true`、`chat_template_kwargs.enable_thinking == false` |
| `schema_violation_hands_turn_to_ornith` | `backchannel` が不正な JSON を返す | `think == true`、`escalation == Some("schema_violation")` または `"parse_error"` |

### チェックポイント

```sh
cargo test --manifest-path src-tauri/Cargo.toml larm_voice
cargo test --manifest-path src-tauri/Cargo.toml
```

## 9. Phase 4: Ornith の thinking 制御

ブランチ: `feat/ornith15-thinking`

### 変更ファイル

- `crates/larm-session/src/http_api.rs`
- `src-tauri/src/providers/stream/larm_voice.rs`
- 設定の JSON schema や TypeScript 型に `LlmOptions` が出てくる箇所（`rg -n "tokenLimit|LlmOptions" src src-tauri/src` で確認）

### 手順

1. `http_api.rs` に追加する。
   ```rust
   #[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, PartialEq, Eq)]
   #[serde(rename_all = "kebab-case")]
   pub enum Thinking { #[default] Auto, Disabled, Enabled }
   ```
   `LlmOptions` に `#[serde(default)] pub thinking: Thinking` を追加する。`Default` と `standard()` は `Auto`。`deny_unknown_fields` はそのまま（既存の保存値にこのキーはないので、`default` で読める）。
2. `apply()` の最後（qwen 用の `chat_template_kwargs` 設定の後）に追加する。
   - `Disabled` → `body["chat_template_kwargs"] = json!({"enable_thinking": false})`
   - `Enabled` → `body["chat_template_kwargs"] = json!({"enable_thinking": true})`
   - `Auto` → 何もしない
3. `stream/larm_voice.rs` の `stream_larm_voice_provider` で、`request_options` を次のように決める。
   ```rust
   let mut options = request_options.unwrap_or_default();
   options.thinking = Thinking::Disabled;
   ```
   それを `OpenAiCompatibleProviderSettings.request_options` に `Some(options)` として渡す。
4. 経路 B（`stream_dynamic_lan_provider`）とメモリー抽出は変更しない。
5. TypeScript 側に `LlmOptions` の型がある場合は `thinking?: "auto" | "disabled" | "enabled"` を追加する。UI は追加しない。

### 追加テスト

| 場所 | テスト名 | 期待値 |
|---|---|---|
| `http_api.rs` | `thinking_disabled_overrides_qwen_reasoning_kwargs` | model `qwen3.5-2b-fast-response`、effort `low`、`Disabled` のとき `chat_template_kwargs == {"enable_thinking": false}` |
| `http_api.rs` | `thinking_auto_keeps_existing_behavior` | 既存テスト `model_capabilities_choose_only_supported_parameters` の期待値が変わらない |
| `http_api.rs` | `llm_options_without_thinking_key_deserializes` | `{"tokenLimit":"auto"}` を読むと `thinking == Auto` |
| `world_tests.rs` など | `voice_conversation_request_disables_thinking` | 経路 A の会話本体の request body に `chat_template_kwargs.enable_thinking == false` |

### チェックポイント

```sh
cargo test --manifest-path crates/larm-session/Cargo.toml http_api
cargo test --manifest-path src-tauri/Cargo.toml
bun test
```

## 10. Phase 5: リアルタイム計測（trace）

ブランチ: `feat/ornith15-trace`

### 変更ファイル

- `src-tauri/src/providers/turn_trace.rs`（新規）
- `src-tauri/src/providers/mod.rs`（`pub(crate) mod turn_trace;`）
- `src-tauri/src/providers/chat_completions/chunks.rs`、`sse.rs`
- `src-tauri/src/providers/larm_voice/frontdesk.rs`、`frontdesk_decision.rs`
- `src-tauri/src/providers/stream/larm_voice.rs`
- `src-tauri/src/voice/http_audio/requests.rs`（TTS 開始の記録 1 行）

### 手順

1. **trace の保存場所**（`turn_trace.rs`）: `ModelStreamContext` などの既存構造体にはフィールドを足さず、`conversation_id` をキーにしたプロセス内レジストリにする。
   ```rust
   pub(crate) enum TraceEvent {
       AsrFinal,
       LlmRequest { role: &'static str, model: String, escalated_from_backchannel: bool },
       FirstVisibleToken,
       Token,
       Done { completion_tokens: Option<u32> },
       TtsStart,
   }
   pub(crate) fn begin_turn(conversation_id: &str) -> String;      // trace_id（Uuid）を返す
   pub(crate) fn mark(conversation_id: &str, event: TraceEvent);   // 現在のターンに Instant::now() で記録
   pub(crate) fn finish(conversation_id: &str);                   // ログに 1 行出してから削除
   ```
   - 実装は `static REGISTRY: LazyLock<Mutex<HashMap<String, TurnTrace>>>`。ターンが始まっていない conversation への `mark` は無視する。
   - 同じターンで `LlmRequest` が 2 回来た場合（受付 → Ornith への昇格）は、両方を `attempts: Vec<Attempt>` に積む。`FirstVisibleToken`、`Token`、`Done` は直近の attempt に記録する。`FirstVisibleToken` は attempt ごとに最初の 1 回だけ記録する。
   - `finish` が出すログ: `tracing::info!(target: "saaa::turn_trace", trace_id, asr_to_request_ms, ttft_ms, decode_tps, total_ms, role, model, escalated_from_backchannel, first_token_to_tts_ms)`。
     - `ttft_ms` = `FirstVisibleToken` − `LlmRequest`
     - `total_ms` = `Done` − `LlmRequest`
     - `decode_tps` = `completion_tokens`（なければ `Token` の回数）÷（`Done` − `FirstVisibleToken`）
2. **可視文字の判定**（`chunks.rs`）
   ```rust
   pub(crate) fn is_visible_delta(chunk: &serde_json::Value) -> bool
   ```
   `choices[*].delta.content` が文字列で、空白以外の文字を 1 文字以上含むときだけ `true`。`reasoning_content`、`reasoning`、`role` だけの delta、空文字、空白のみは `false`。
3. **記録する場所**
   - `AsrFinal`: `larm_voice/frontdesk.rs` で、ASR 確定テキストを受け取って受付処理を始める関数の先頭。ここで `begin_turn` を呼び、続けて `mark(AsrFinal)` する。
   - 受付: `frontdesk_decision.rs` の呼び出し直前に `LlmRequest{role: "backchannel" または "llm", ...}`。SSE を読む処理で `is_visible_delta` が最初に `true` になったら `FirstVisibleToken`、各 visible delta で `Token`、終わりに `Done`。
   - Ornith: `stream/larm_voice.rs` の `stream_model_provider_with_api_key` 呼び出し直前に `LlmRequest{role: "llm", escalated_from_backchannel: <受付が Delegate または ToolCandidate だったか>}`。`chat_completions/sse.rs` のストリーム処理で、`conversation_id` が分かる箇所から `FirstVisibleToken`、`Token`、`Done` を記録する（`ModelStreamContext` が持つ conversation id を使う。見当たらなければ実装を止めて質問する）。
   - `TtsStart`: `voice/http_audio/requests.rs` で、TTS の `acquire("tts")` が成功した直後。
   - `finish`: 受付だけで終わるターンは定型文の TTS 開始後。Ornith に渡したターンは、Ornith の応答が終わって最初の TTS が始まった後。
4. 永続化はしない（ログ出力のみ）。

### 追加テスト

| テスト名 | 内容 | 期待値 |
|---|---|---|
| `is_visible_delta_cases` | `role` のみ、`reasoning_content`、`content:""`、`content:"  "`、`content:"は"` | 「は」だけが `true` |
| `ttft_ignores_reasoning_and_empty_deltas` | SSE を上の順で各 50ms 間隔で送る fixture | `ttft_ms` が 200〜300 の範囲 |
| `trace_records_backchannel_only_turn` | 受付だけで終わるターン | attempt が 1 つ、`role == "backchannel"`、`escalated_from_backchannel == false` |
| `trace_records_escalation_to_ornith` | 受付が schema 違反 → Ornith | attempt が 2 つ、2 つ目が `role == "llm"` かつ `escalated_from_backchannel == true` |
| `decode_tps_uses_completion_tokens` | `completion_tokens = 100`、FirstVisible から Done まで 1 秒 | `decode_tps` が約 100 |

### チェックポイント

```sh
cargo test --manifest-path src-tauri/Cargo.toml turn_trace
cargo test --manifest-path src-tauri/Cargo.toml chunks
cargo test --manifest-path src-tauri/Cargo.toml
```

## 11. Phase 6: Tool 候補を Ornith に渡す

ブランチ: `feat/ornith15-tool-hint`

Phase 3 の `ConversationDecision.tool_hint` を、Ornith の request まで届ける。

1. `think == true` の発言は `frontdesk_repository.rs` の `lfm_voice_utterances` に `status='delegate'` で保存され、その後 Ornith の推論 run が `claim_reasoning_request` で取り出す。`tool_hint` をこの受け渡しに載せる。
2. `lfm_voice_utterances` に `tool_hint TEXT` 列を追加する migration を書く（既存の `CREATE TABLE` 定義の近くにある migration の書き方に合わせる）。migration のテストは一時 DB で行う。
3. `claim_reasoning_request` の戻り値に `tool_hint` を含め、Ornith の request を組み立てる箇所で、system メッセージとして「受付の一次判定: Tool `{name}` が候補。使うかどうか、引数は会話から判断すること」を追加する。
4. `request.content` の一致確認（`m.content=?3`）には手を入れない。

テスト:

| テスト名 | 期待値 |
|---|---|
| `tool_hint_survives_repository_round_trip` | 保存した `tool_hint` が `claim_reasoning_request` で返る |
| `tool_hint_reaches_ornith_request` | Ornith の request body に候補 Tool 名を含む system メッセージがある |
| `backchannel_never_executes_tools` | 受付だけで終わるターンで、Tool 実行関数が 1 回も呼ばれない |

## 12. Phase 7: 既定値の整理

ブランチ: `feat/ornith15-defaults`

1. `src-tauri/src/persistence/settings_defaults.rs` と `src/features/settings/settingsDefaults.ts` の既定の `larmProfile` を `saaa-conversation-ornith15` にする。
2. `tests/fixtures/ipc-receivers.json`、`tests/settings-regressions.test.ts` の期待値を追従させる。
3. `role_routing/schema.rs` の 206 行目と 571 行目は `DEFAULT_PROFILE` を参照しているので、値は自動的に変わる。既存 migration の結果が変わるため、関連する migration テストの期待値を確認し、過去の migration が書き込む値は `PREVIOUS_DEFAULT_PROFILE` に固定する（過去の migration の意味を変えない）。
4. `spec/docs/larm-http-api-review.md` の既定 Profile の記述を更新する。

チェックポイント:

```sh
cargo test --manifest-path src-tauri/Cargo.toml
bun test
```

## 13. ルーティング判断表

| 入力の性質 | 担当 | Verdict |
|---|---|---|
| 「はい」「なるほど」 | Qwen 2B | `Backchannel{nod}`（定型文） |
| 挨拶、お礼 | Qwen 2B | `SimpleReply`（定型文） |
| 発話の途中 | Qwen 2B | `Wait` |
| 単一の read_only Tool で済む依頼 | Qwen 2B が候補を出し、Ornith が引数構築と実行 | `ToolCandidate` |
| 推論や曖昧さの解消が必要 | Ornith | `Delegate{RequestedByModel}` |
| 複数段階の Tool use、文脈からの引数構築 | Ornith | `Delegate` |
| 状態変更、権限、安全性、不可逆操作 | Ornith | `Delegate{SideEffectTool}` |
| 推論中の追加発言 | 受付を呼ばず無言 | `Wait`（`think = false`） |
| 長い入力 | Ornith | `Delegate{InputTooLarge}` |
| Qwen 2B の出力が不正、空、自信なし、タイムアウト | Ornith | `Delegate{ParseError / SchemaViolation / Empty / LowConfidence / Timeout}` |
| ユーザーへの自由文の回答 | Ornith | — |

## 14. 依頼のテスト 10 項目との対応

| # | 依頼項目 | テスト |
|---|---|---|
| 1 | canonical を発見・選択 | `auto_selects_canonical_ornith15_profile`、`text_path_selects_canonical_profile` |
| 2 | Profile がない場合だけ `saaa-qwen38` | `auto_falls_back_to_legacy_only_when_canonical_absent`、`text_path_falls_back_to_legacy_only_when_canonical_absent` |
| 3 | Qwen 3.8 をハードコードしない | `legacy_profile_model_comes_from_claim`、`no_qwen38_literal_in_sources` |
| 4 | 5 Provider を claim し同じ session で利用 | `claims_five_providers_in_any_order`、`canonical_requires_backchannel`、`frontdesk_uses_backchannel_provider_when_present` |
| 5 | Qwen 2B の短い相槌 | `nod_for_short_acknowledgement` |
| 6 | Qwen 2B の単純な Tool 選択 | `read_only_tool_becomes_candidate` |
| 7 | 複雑な依頼を Ornith へ昇格 | `complex_request_is_delegated` |
| 8 | JSON / schema 違反で Ornith へ fallback | `invalid_json_escalates`、`unknown_field_escalates`、`schema_violation_hands_turn_to_ornith` |
| 9 | renew / release で lease が漏れない | `renew_reclaims_all_five_and_release_leaves_no_lease` |
| 10 | TTFT が最初の可視文字基準 | `ttft_ignores_reasoning_and_empty_deltas` |

## 15. 実機確認（任意、Phase 5 完了後）

本番の LARM はまだ旧 release なので、実機確認は LARM の隔離環境に対して行う。保存設定は変えず、環境変数で接続先を渡す。

```sh
SAAA_LARM_CONTROL_URL=<隔離環境の control URL> LARM_API_TOKEN=<token> \
  cargo test --manifest-path crates/larm-session/Cargo.toml --test live -- --ignored
```

`tests/live.rs` に、自動選択で canonical が選ばれることと、5 Provider が `acquire` できることを確認する `live_canonical_five_provider_session`（`#[ignore]` 付き）を追加する。隔離環境の control URL と token は LARM 担当者から受け取る。

## 16. 完了条件

- 14 章のテストがすべて存在し、通過している。
- `cargo test --manifest-path crates/larm-session/Cargo.toml`、`cargo test --manifest-path src-tauri/Cargo.toml`、`bun test` が通過している。
- `rg -n 'providers\[0\]|providers\.first\(\)' src-tauri/src/providers/dynamic_lan/validate.rs crates/larm-session/src` が 0 件。
- `rg -n 'qwen3\.8|qwen-3\.8' src-tauri/src crates/larm-session/src` が、`http_api.rs` の既存テスト（model 名の判定テスト）以外で 0 件。
- 本番の旧 release の LARM でも音声会話が動く（4.1 の自動選択で `saaa-conversation-gemma4` にフォールバックする）ことを、`auto_uses_previous_default_when_only_it_exists` で確認している。
- 各 Phase の PR 説明に、変更ファイル、テスト結果、未解決事項を書いている。

## 17. リスク

| リスク | 対策 |
|---|---|
| 本番 LARM が旧 release のため canonical がない | 4.1 の自動選択で、現在動いている Profile に落ちる。LARM の本番反映後は何もしなくても canonical に切り替わる |
| Qwen 2B の JSON 遵守率が低く、昇格が多発する | Phase 5 の trace で昇格理由の内訳を見て、INSTRUCTION を調整する。昇格しても応答は成立する |
| media variant（image 34GB）の起動中に、基本セットの health が一時的に落ちる | 既存の `larm_unhealthy_provider` と capacity 待ちの処理に任せる。SAAA からは variant を操作しない |
| `larm-session` の API 変更で `services/reasoning-mcp` がコンパイルエラーになる | 既存の `connect_with_profile*` を `Explicit` のラッパーとして残す。Cargo workspace はないので、`cargo build --manifest-path services/reasoning-mcp/Cargo.toml` を個別に実行して確認する |
| `LlmOptions` へのフィールド追加で、保存済み設定の読み込みが壊れる | `#[serde(default)]` を付け、`llm_options_without_thinking_key_deserializes` で確認する |
