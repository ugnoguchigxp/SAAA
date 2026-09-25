# Ornith 1.5 35B + Qwen 3.5 2B 執事応答 実装計画（LARM selector 対応版）

作成日: 2026-09-24（第 3 版）／改訂: 2026-09-25（第 4 版。LARM の profile selector に合わせて全面改訂）
状態: 計画確定、未着手
関連: `docs/plans/saaa-butler-role-routing.md`（Role Routing による受付 → 思考の実装。Stage 1 は実装済み。本計画はその上で「LARM から借りる 2 つの LLM を正しく揃える」部分を担当する）

## 0. 実装者への指示

- Phase を順番に実装する。各 Phase のチェックポイントが通るまで次に進まない。
- 「決定事項」の値と名前はそのまま使う。書かれていない設計判断が必要になったら、実装を止めて質問する。
- 「やらないこと」に書かれた操作はしない。
- 作業ツリーには本件と関係のない未コミット変更がある。触らない。コミットは Phase ごとに、この文書に列挙したファイルだけを `git add -p` で選ぶ。
- 行番号は 2026-09-25 時点。ずれていたら関数名や文字列で検索する。
- **完了の根拠は、会話ターンの入口（`execute_conversation_turn_with_candidates`）から通すテストだけにする**（`saaa-butler-role-routing.md` 0.1 と同じ）。新しく書いた関数を直接呼ぶ単体テストは補助。
- 凍結対象（`critical-path-freeze.json` の ASR / initial-response）に触れる Phase では、AGENTS.md の回帰テストを実行し、`bun run freeze:accept:<domain> --reason "..."` で該当 domain だけを更新する。**`freeze:check` は 2026-09-25 時点で、本件と無関係な 3 ファイル（`role_routing/schema.rs`、`conversation_provider_route.d/01.rs`、`conversation_turn.rs`）の差分により失敗している。** Phase 1 の前に、この差分の持ち主と承認方法をユーザーに確認する。

### 0.1 これまでの経緯（なぜ 2 つの LLM が噛み合わなかったか）

- LARM 側で、Ornith（`llm`）と Qwen 2B（`backchannel`）が 1 つの Profile に揃っていなかった。`backchannel` は `saaa-backchannel-default` など別 Profile にあり、canonical Profile は本番に存在しなかった。
- SAAA は Profile ID の固定リスト（`saaa-conversation-ornith15` → `saaa-qwen38` → `saaa-conversation-gemma4`）から自動選択していたため、どの Profile が選ばれるかで `backchannel` の有無と `llm` のモデルが変わっていた。
- 2026-09-25、LARM に consumer selector（`SAAA`、`SAAA-w-Image`、`SAAA-w-music`）が入り、1 回の問い合わせで「SAAA 用の 5 Provider が揃った具体 Profile」を 1 件だけ返すようになった。本計画はこれを唯一の入口にする。

## 1. 目的

1. LARM の profile 問い合わせ（`GET /v3/agent-profiles?profile=SAAA`）の結果を、SAAA が使う 2 つの LLM の構成として正しく反映する。
   - 受付（role `frontend`）: `backchannel` = Qwen 3.5 2B
   - 思考（role `reasoner`）: `llm` = Ornith 1.5 35B
2. 音声・テキストのどちらの会話でも、思考は Ornith 1.5 35B に届く。
3. catalog が宣言した LLM と、claim で実際に借りた LLM が食い違ったら、黙って別モデルで動かずに失敗する。

## 2. LARM の仕様（2026-09-25 実機確認済み）

接続先: `http://192.168.0.130:9810`（保存済み Harness address）。確認に使った操作は `GET /v3/agent-profiles` と `GET /openapi.json` だけで、Connection は作っていない。

### 2.1 Profile 問い合わせ

`GET /v3/agent-profiles?profile=<selector>`。selector は OpenAPI の enum で `contextStill`、`SAAA`、`SAAA-w-Image`、`SAAA-w-music`、`vulnWorkbench`。

| selector | `requestedProfile` | 件数 | 返る `id` | Provider | `services` |
|---|---|---|---|---|---|
| `SAAA` | `SAAA` | 1 | `saaa-conversation-ornith15` | asr, backchannel, embedding, llm, tts | `[]` |
| `SAAA-w-Image` | `SAAA-w-Image` | 1 | `saaa-conversation-ornith15` | 同上 | `image`（`media.image.generate`、`larm.image-generation.v1`、`/v1/images/generations`、`qwen-image-2.1`） |
| `SAAA-w-music` | `SAAA-w-music` | 1 | `saaa-conversation-ornith15` | 同上 | `music`（`media.music.generate`、`larm.music-generation.v1`、`/v1/music/generations`、`ace-step-1.5`） |

`profile=SAAA` の 2 つの LLM:

| Provider 名 | model | capability | protocol / endpoint | contextWindow（max / outputReserve / safetyMargin） |
|---|---|---|---|---|
| `llm` | `ornith-1.5-35b` | `llm.general` | `openai.chat-completions.v1` / `/v1/chat/completions` | 131072 / 4096 / 1976 |
| `backchannel` | `qwen3.5-2b-fast-response` | `llm.backchannel.classifier` | `openai.chat-completions.v1` / `/v1/chat/completions` | 65536 / 4096 / 1976 |

その他: `asr` = `qwen3-asr-1.7b`、`tts` = `voicevox-core`、`embedding` = `multilingual-e5-small`（384 次元、`larm.embedding.v1`、`/v1/embed`）。

応答のトップレベル: `contractVersion`（`agent-connection.v3`）、`catalogRevision`、`defaultAgentProfile`（`coding-default`。使わない）、`requestedProfile`、`profiles`、`audiences`。Profile の必須キーには `services` が含まれる（OpenAPI の `required`）。

### 2.2 Connection と claim（OpenAPI）

- `POST /v1/agent-connections` の body（`AgentConnectionRequest`、`additionalProperties: false`）: `agentProfile`、`explicitAgentProfile`、`audience`、`client`、`ttlSeconds`、`allowFallback`、`deploymentPolicy`。**selector や variant、services を渡す欄はない。**
- claim 応答（`AgentConnectionClaim`）のキー: `allocationId`、`audience`、`contextControl`、`expiresAt`、`id`、`providers`、`status`。**`services` は返らない。**
- したがって、3 つの selector はどれも同じ Connection（`saaa-conversation-ornith15`）になる。image / music の Service を Connection から借りる手段は、現在の LARM にはない。

## 3. SAAA の現状（2026-09-25）

### 3.1 実装済み（コミット済み。`a1446ba`、`225006a`）

- Role Routing の執事構成: actor `larm-frontdesk`（`larmProvider: backchannel`）と `larm-reasoner`（`larmProvider: llm`）、recipe `00-butler-respond`（`["frontend","reasoner"]`）。新規インストールの既定値（`role_routing/contracts.rs` 146〜178 行目、`src/features/settings/settingsRoleRoutingDefaults.ts`）。
- 受付 step（`role_routing/frontend.rs`、`conversation_provider_route.d/01.rs` 880〜970 行目付近）: `stream: true`、`max_tokens: 64`、`chat_template_kwargs.enable_thinking=false`、ack はホストの定型文。
- 思考 step: `stream/larm_voice.rs` で `acquire(larm_provider)`、`Thinking::Disabled`。
- 入口から通すテスト: `providers/larm_voice/butler_route_tests.rs`（`voice_turn_runs_frontend_then_reasoner` など 16 件）。
- `larm-session`: `BASE_PROVIDERS` + `BACKCHANNEL`、`has_provider()`、claim した全 Provider の health check、renew / release。

### 3.2 未コミット（今回の discovery 変更）

- `catalog::fetch(client, base, token, selector)`: `?profile=<selector>` を付け、`requestedProfile == selector`、`profiles.len() == 1` を検証し、`providers[].name` と `services[].name` を読む。
- `ProfilePreference::Variant(ProfileVariant)`（`Conversation` / `Image` / `Music` → `SAAA` / `SAAA-w-Image` / `SAAA-w-music`）。`Auto` は `Conversation` と同じ。
- 5 Provider がすべて宣言されていること、`services` が variant の期待（`[]` / `["image"]` / `["music"]`）と完全一致することを検証し、返った `id` で Connection を作る。
- `select_voice_profile`（ID 固定リストからの自動選択）は削除済み。

### 3.3 残っている問題

| # | 問題 | 場所 | 影響 |
|---|---|---|---|
| G1 | `Variant` が `label()` で `@auto` になり、Owner から Session を作り直すときに `Auto` へ戻る | `providers/larm_voice/profile.rs` の `label`、`providers/larm_voice/mod.rs` 121 行目 | Image / music を指定しても `SAAA` で問い合わせる |
| G2 | 保存値 `SAAA` などの selector 文字列が `Explicit` 扱いになる | `profile.rs` の `preference` | `agentProfile: "SAAA"` で Connection を作り、LARM に拒否される |
| G3 | `Explicit` の必須 Provider が `required_providers(id)` で、`backchannel` は `saaa-conversation-ornith15` のときだけ必須 | `crates/larm-session/src/contract.rs` | 明示 Profile で受付の LLM が欠けても接続が成功し、受付 step が実行時に失敗する |
| G4 | catalog の LLM 情報（model、contextWindow、protocol、endpoint、capability）を読まずに捨てている。claim の内容と照合していない | `crates/larm-session/src/catalog.rs` | catalog と claim が食い違っても気付かない。どの LLM で動いたかを診断で示せない |
| G5 | 既定値が食い違っている。Rust の `DEFAULT_PROFILE` は `saaa-conversation-ornith15`、TypeScript の `DEFAULT_LARM_PROFILE` は `saaa-conversation-gemma4` | `contract.rs`、`src/features/settings/settingsDefaults.ts` 7 行目 | 画面と実行時の既定値が違う |
| G6 | テスト fixture のモデルが実機と違う（`llm` = `gemma-4-e4b`、`backchannel` = `fixture`、両方 contextWindow 230400） | `providers/larm_voice/world_wire_fixture.rs` 94 行目 | Qwen 用の `reasoning_effort` 挿入と `Thinking::Disabled` の上書きが、実機のモデル名で試されていない |
| G7 | テキスト会話の経路 B が `AGENT_PROFILE`（`saaa-qwen38`）固定で、Provider が 1 つの Profile しか受け付けない | `providers/dynamic_lan/validate.rs` 144、202〜218、267 行目、`mod.d/03.rs` 27 行目 | テキスト会話の思考が Ornith に届かない（`saaa-qwen38` は Gemma 4 構成） |
| G8 | 表示名に旧構成が残っている可能性（「Gemma 4 E4B (LARM)」） | 保存設定の actor `label` | 画面の表示と実際のモデルが違う。保存設定は書き換えない（4.6） |

## 4. 決定事項

### 4.1 Profile の決め方（経路 A: 音声、経路 B: テキスト共通）

| 保存値 `harness.larmProfile` | 解決 |
|---|---|
| 未設定、`SAAA`、`saaa-conversation-ornith15`、`saaa-conversation-gemma4`、`saaa-qwen38` | `Variant(Conversation)`（selector `SAAA`） |
| `SAAA-w-Image` | `Variant(Image)` |
| `SAAA-w-music` | `Variant(Music)` |
| それ以外 | `Explicit(値)`。catalog を問い合わせず、その ID で Connection を作る |

- `ProfilePreference::Auto` は削除し、`Variant(Conversation)` に統一する。`Auto` を受け取る公開 API はない形にする（呼び出し側はすべて `profile::preference` を通す）。
- 旧 ID（`saaa-conversation-ornith15`、`saaa-conversation-gemma4`、`saaa-qwen38`）は、保存済み設定の互換のためだけに selector `SAAA` へ読み替える。ID の固定リストからの自動選択やフォールバックは復活させない。
- `defaultAgentProfile` は使わない。
- Owner の比較用文字列（`label`）は、`Variant` なら `"@selector:<selector>"`（例 `@selector:SAAA-w-Image`）、`Explicit` ならその値。`@` は `validate_profile` が拒否する文字なので、明示 ID と衝突しない。Session を作り直すときは、この文字列から `Variant` / `Explicit` を**復元する**関数 `profile::from_label` を使う（G1）。

### 4.2 必須 Provider

| 解決結果 | 必須 | 検証 |
|---|---|---|
| `Variant(*)` | `llm`、`backchannel`、`asr`、`tts`、`embedding` | catalog と claim の両方 |
| `Explicit` | `llm`、`backchannel`、`asr`、`tts`、`embedding` | claim のみ |

- `Explicit` でも `backchannel` を必須にする（G3）。執事構成は受付を前提にしており、欠けたまま接続を成功させない。`backchannel` を持たない Profile を使いたいという要望が出たら、止めて質問する。
- `required_providers(profile)` は引数を取らない `required_providers()` にし、常に 5 つを返す。
- `backchannel` と `llm` は `contextWindow` 必須（現状どおり）。

### 4.3 catalog の LLM 情報の反映（G4）

`CatalogProfile` を次の形にする。

```rust
pub struct CatalogProfile {
    pub revision: String,          // catalogRevision
    pub selector: String,          // requestedProfile
    pub id: String,
    pub providers: Vec<CatalogProvider>,
    pub services: Vec<CatalogService>,
}
pub struct CatalogProvider {
    pub name: String,
    pub capability: String,
    pub protocol: String,
    pub endpoint: String,
    pub model: String,
    pub context_window: Option<ContextWindow>,
}
pub struct CatalogService { pub name: String, pub capability: String, pub protocol: String, pub endpoint: String, pub model: String }
```

- `catalogRevision`、`requestedProfile`、各 Provider の `name` / `capability` / `protocol` / `endpoint` / `model` は必須。欠けたら `larm_catalog_invalid`。
- claim 後に、catalog と claim を Provider 名ごとに照合する。一致しなければ `larm_catalog_claim_mismatch` で失敗し、Connection を release する。
  - `model` が一致すること。
  - `protocol` が一致すること。
  - `llm` と `backchannel` は `contextWindow` の 3 値が一致すること。
  - endpoint は claim の `baseUrl` と組み合わせて使うため照合しない。
- renew 後の再 claim でも同じ照合を行う（Session に `CatalogProfile` を保持する）。
- モデル名やコンテキスト長は、これまでどおり claim の値を使う。catalog の値は照合と表示にだけ使う。**Profile ID やモデル名から挙動を分けるコードは書かない。**
- Session に次のアクセサを追加する。
  ```rust
  pub fn selector(&self) -> Option<&str>;                 // Explicit なら None
  pub fn catalog_revision(&self) -> Option<&str>;
  pub async fn provider_summary(&self) -> Vec<ProviderSummary>; // name, model, context_window（token は含めない）
  ```

### 4.4 media variant（image / music）

- SAAA の既定は `SAAA`。`SAAA-w-Image` / `SAAA-w-music` は、保存値で明示されたときだけ使う。設定画面に選択肢は追加しない。
- variant を指定しても Connection は同じになる（2.2）。variant は「その media Service が今 LARM にあるか」の確認にだけ使う。image / music の Service 自体の呼び出しは本計画の範囲外。
- LARM の variant 切替（sudo スクリプト）は SAAA から操作しない。

### 4.5 テキスト会話（経路 B、G7）

- `dynamic_lan` の Profile 選択を、4.1 と同じ selector 問い合わせに置き換える。catalog の問い合わせは `larm-session` の `catalog::fetch` を公開して使う（同じ検証を 2 か所に書かない）。
- 経路 B が使うのは `llm` だけ。state と claim の `providers[]` から `name == "llm"` の要素をちょうど 1 つ探し、他の Provider は無視する。
- `providers.len() != 1`、`providers[0]`、`.first()` による特定はすべて削除する。

### 4.6 保存設定

- コードから保存設定を自動で書き換えない。4.1 の読み替えは実行時に行う。
- 新規インストールの既定値だけを `SAAA` に変える（Phase 4）。

## 5. やらないこと

- production への切替、LARM service の起動・停止・再起動、media variant の切替。
- 保存済みユーザー設定の削除や書き換え。
- Profile ID の固定リストからの自動選択、`defaultAgentProfile` の利用。
- Profile ID やモデル名からモデルの性質を推測するコード。
- `providers[0]`、`.first()`、`providers.len() == N` による Provider の特定。
- `larm_voice/frontdesk*` の受付経路の復活。
- `voice/**` の ASR 処理ロジックの変更。

## 6. Phase 1: selector の解決と保持（G1、G2、G3、G5 の Rust 側）

ブランチ: `feat/larm-selector-profile`

### 変更ファイル

- `crates/larm-session/src/lib.rs`
- `crates/larm-session/src/contract.rs`
- `crates/larm-session/src/tests.rs`
- `crates/larm-session/tests/live.rs`
- `src-tauri/src/providers/larm_voice/profile.rs`
- `src-tauri/src/providers/larm_voice/mod.rs`
- `ProfilePreference::Auto` と `required_providers(` の参照箇所（`rg -n "ProfilePreference::Auto|required_providers\(" crates src-tauri/src services`）

### 手順

1. `lib.rs`: `ProfilePreference` を `Variant(ProfileVariant)` と `Explicit(String)` の 2 つにする。`ProfileVariant::selector()` を `pub` にし、逆変換 `ProfileVariant::from_selector(&str) -> Option<Self>` を足す。
2. `contract.rs`: `required_providers()` を引数なしにし、5 つを返す。`DEFAULT_PROFILE` を削除し、`DEFAULT_SELECTOR: &str = "SAAA"` を追加する。旧 ID の定数 3 つは `LEGACY_PROFILE_IDS: [&str; 3]` にまとめ、`profile.rs` の読み替えにだけ使う。
3. `lib.rs` の `connect_inner`: `Explicit` の必須 Provider を `required_providers()` にする。`Variant` の処理は未コミット分をそのまま使う。
4. `profile.rs`:
   - `preference(stored)` を 4.1 の表どおりにする。
   - `label(preference)` を 4.1 の `"@selector:<selector>"` 形式にする。
   - `from_label(label: &str) -> ProfilePreference` を追加する。`@selector:` で始まり既知の selector なら `Variant`、それ以外は `Explicit`。
5. `larm_voice/mod.rs` 121 行目: `if owner.profile == profile::AUTO_LABEL { Auto } else { Explicit }` を `profile::from_label(&owner.profile)` にする。`AUTO_LABEL` は削除する。
6. `settings_defaults.rs` と `role_routing/schema.rs` の `DEFAULT_PROFILE` 参照は `DEFAULT_SELECTOR` に置き換える。ただし `schema.rs` の既存 migration（206 行目）が書き込む値は、過去の migration の意味を変えないため `"saaa-conversation-ornith15"` の文字列で固定する。関連する migration テストの期待値を確認する。

### 追加・変更テスト

`crates/larm-session/src/tests.rs`:

| テスト名 | 内容 | 期待値 |
|---|---|---|
| `variant_queries_selector_and_creates_returned_id`（既存を拡張） | 3 つの variant | query が `profile=<selector>`、create body の `agentProfile == "saaa-conversation-ornith15"` |
| `selector_mismatch_is_rejected` | `requestedProfile` が別の値 | `larm_catalog_invalid`。Connection は作られない |
| `multiple_profiles_are_rejected` | `profiles` が 2 件 | `larm_catalog_invalid`。Connection は作られない |
| `missing_backchannel_in_catalog_is_rejected` | catalog の Provider から `backchannel` を除く | `larm_profile_unavailable`。Connection は作られない |
| `variant_service_mismatch_is_rejected` | `SAAA-w-Image` なのに `services: []`、`SAAA` なのに `image` あり | `larm_profile_unavailable` |
| `explicit_profile_requires_backchannel` | `Explicit("custom-x")`、claim に `backchannel` なし | `larm_missing_provider`。release を 1 回受信 |
| `explicit_profile_skips_catalog`（既存） | `Explicit("custom-x")` | catalog を GET しない |

`src-tauri/src/providers/larm_voice/profile.rs`:

| テスト名 | 入力 | 期待値 |
|---|---|---|
| `preference_maps_shipped_values_to_saaa_selector` | `None`、`"SAAA"`、旧 ID 3 つ | `Variant(Conversation)` |
| `preference_maps_media_selectors` | `"SAAA-w-Image"`、`"SAAA-w-music"` | `Variant(Image)`、`Variant(Music)` |
| `preference_respects_operator_choice` | `"custom-x"`、`"auto"` | `Explicit` |
| `label_round_trips_every_preference` | 上のすべて | `from_label(label(p)) == p` |

`world_tests.rs` など（会話ターンの入口から）:

| テスト名 | 内容 | 期待値 |
|---|---|---|
| `voice_session_restart_keeps_media_variant` | 保存値 `SAAA-w-Image` で音声セッションを開始 → 期限切れで作り直す | 2 回とも catalog の query が `profile=SAAA-w-Image` |

### チェックポイント

```sh
cargo test --manifest-path crates/larm-session/Cargo.toml
cargo test --manifest-path src-tauri/Cargo.toml larm
cargo test --manifest-path services/reasoning-mcp/Cargo.toml
rg -n "ProfilePreference::Auto|AUTO_LABEL|DEFAULT_PROFILE\b" crates src-tauri/src services   # 0 件
```

## 7. Phase 2: catalog の LLM 情報を照合し、診断に出す（G4、G6）

ブランチ: `feat/larm-catalog-llm-binding`

### 変更ファイル

- `crates/larm-session/src/catalog.rs`
- `crates/larm-session/src/lib.rs`
- `crates/larm-session/src/contract.rs`（照合関数）
- `crates/larm-session/src/tests.rs`
- `src-tauri/src/providers/larm_voice/world_wire_fixture.rs`
- `src-tauri/src/diagnosis/checks/harness.rs`
- `src-tauri/src/harness_llm_diagnostic.rs`

### 手順

1. `catalog.rs`: 4.3 の構造体で読む。必須キーが欠けたら `larm_catalog_invalid`。
2. `contract.rs`: `pub(crate) fn verify_against_catalog(snapshot: &Snapshot, catalog: &CatalogProfile) -> Result<(), &'static str>` を追加する。4.3 の照合を行い、不一致は `larm_catalog_claim_mismatch`。
3. `lib.rs`: `Variant` のとき `CatalogProfile` を `Session` に保持し、`claim()` と renew 後の再 claim の直後に `verify_against_catalog` を呼ぶ。失敗したら既存の失敗時と同じく release する。4.3 のアクセサ 3 つを追加する。
4. `ConnectError` のユーザー向け文言に `larm_catalog_claim_mismatch` を追加する（`src/i18n/locales/*Settings.ts` の既存の LARM エラー文言の並びに合わせる）。文言: 「LARM の Profile 情報と、実際に割り当てられたモデルが一致しません。LARM の再起動後に再接続してください。」
5. `world_wire_fixture.rs`: fixture の catalog と claim を 2.1 の実機の値にする（`llm` = `ornith-1.5-35b` / 131072、`backchannel` = `qwen3.5-2b-fast-response` / 65536、`asr` / `tts` / `embedding` も実機の model）。`?profile=` の query に応じて `requestedProfile` と `services` を返す。
6. 診断: `diagnosis/checks/harness.rs` と `harness_llm_diagnostic.rs` の LARM 項目に、`selector`、`catalog_revision`、`provider_summary()` の `llm` と `backchannel` の model と `maxTokens` を出す。token や baseUrl は出さない。

### 追加テスト

| 場所 | テスト名 | 期待値 |
|---|---|---|
| `tests.rs` | `catalog_reads_llm_and_backchannel_details` | 2 つの LLM の model、capability、protocol、endpoint、contextWindow が 2.1 の値で読める |
| `tests.rs` | `catalog_missing_model_is_invalid` | `backchannel.model` なし → `larm_catalog_invalid` |
| `tests.rs` | `claim_model_mismatch_is_rejected_and_released` | claim の `llm.model` だけを `gemma4-e4b` にする → `larm_catalog_claim_mismatch`、release 1 回、lease カウンタ 0 |
| `tests.rs` | `claim_context_window_mismatch_is_rejected` | claim の `backchannel.contextWindow.maxTokens` を 230400 にする → `larm_catalog_claim_mismatch` |
| `tests.rs` | `renew_reclaim_is_verified_against_catalog` | renew 後の claim だけ `llm.model` を変える → 以後の `acquire("llm")` が失敗し、release される |
| `tests.rs` | `provider_summary_exposes_models_without_tokens` | `provider_summary()` に 5 件、token を含まない |
| `butler_route_tests.rs` | `voice_turn_runs_frontend_then_reasoner`（既存、fixture 更新後） | `backchannel` の request body の `model == "qwen3.5-2b-fast-response"`、`chat_template_kwargs == {"enable_thinking": false}`（`reasoning_effort` が残っていない）。`llm` の request の `model == "ornith-1.5-35b"`、`enable_thinking == false` |

### チェックポイント

```sh
cargo test --manifest-path crates/larm-session/Cargo.toml
cargo test --manifest-path src-tauri/Cargo.toml larm
cargo test --manifest-path src-tauri/Cargo.toml diagnosis
bun test
```

## 8. Phase 3: テキスト会話も Ornith に届ける（G7）

ブランチ: `feat/larm-selector-text-path`

### 変更ファイル

- `crates/larm-session/src/lib.rs`（`catalog::fetch` と `CatalogProfile` を `pub` にする）
- `src-tauri/src/providers/dynamic_lan/mod.d/01.rs`、`mod.d/03.rs`
- `src-tauri/src/providers/dynamic_lan/validate.rs`
- `src-tauri/src/providers/dynamic_lan/profile_catalog.rs`
- `src-tauri/src/providers/dynamic_lan/mod.d/04.rs`、`mod.d/05.rs`（テスト）
- `AGENT_PROFILE` の参照箇所（`rg -n "AGENT_PROFILE" src-tauri/src`）

### 手順

1. `mod.d/01.rs`: `AGENT_PROFILE` を削除する。Profile は保存値から `larm_voice::profile::preference` で解決する（4.1）。
2. `validate.rs` の `select_default_llm_profile` を削除し、`Variant` なら `saaa_larm_session::catalog::fetch(..., selector)` の結果の `id` と `llm` の情報を使う。`Explicit` なら catalog を問い合わせない。
3. `validate.rs` の `validate_state_shape`（144 行目）と Profile 検証（267 行目）: `len() != 1` と `providers[0]` を削除し、`name == "llm"` の要素をちょうど 1 つ探す。0 個または 2 個以上なら contract error。他の Provider は無視する。
4. `validate_claim` の `llm` を、catalog の `llm`（model、protocol、contextWindow）と照合する。不一致は contract error（4.3 と同じ条件）。
5. `profile_catalog.rs`: v1 分岐と `legacy_profile_context_window` を削除し、`agent-connection.v3` だけを受け付ける。
6. テスト fixture の catalog と state を、2.1 の 5 Provider 構成にする。`claim["providers"][0]` を書き換えている箇所は、`name` で要素を探すヘルパー `provider_mut(claim, name)` に置き換える。

### 追加テスト（`mod.d/04.rs`）

| テスト名 | 内容 | 期待値 |
|---|---|---|
| `text_path_queries_saaa_selector` | 保存値なし | catalog の query が `profile=SAAA`、`agentProfile == "saaa-conversation-ornith15"` |
| `text_path_accepts_five_provider_state_in_any_order` | `providers[]` を `tts, llm, embedding, backchannel, asr` の順にする | 接続成功。`model() == "ornith-1.5-35b"` |
| `text_path_rejects_duplicate_llm` | `llm` が 2 件 | contract error、release 1 回 |
| `text_path_rejects_claim_model_mismatch` | claim の `llm.model` が catalog と違う | contract error、release 1 回 |

会話ターンの入口からのテスト（`butler_route_tests.rs`）:

| テスト名 | 内容 | 期待値 |
|---|---|---|
| `text_turn_skips_frontend_provider`（既存を拡張） | テキスト入力で執事 recipe | `backchannel` は呼ばれず、reasoner の request が `model == "ornith-1.5-35b"` で届く |

### チェックポイント

```sh
cargo test --manifest-path src-tauri/Cargo.toml dynamic_lan
cargo test --manifest-path src-tauri/Cargo.toml
rg -n 'providers\[0\]|providers\.first\(\)|len\(\) != 1|AGENT_PROFILE' src-tauri/src/providers/dynamic_lan   # 0 件
```

## 9. Phase 4: 既定値と表示（G5、G8）

ブランチ: `feat/larm-selector-defaults`

1. `src/features/settings/settingsDefaults.ts` の `DEFAULT_LARM_PROFILE` を `"SAAA"` にする。Rust 側の `settings_defaults.rs` は Phase 1 で `DEFAULT_SELECTOR` になっている。
2. `tests/settings-regressions.test.ts` と `tests/fixtures/ipc-receivers.json` の `larmProfile` の期待値を `"SAAA"` にする。
3. 新規インストールの actor の表示名（`role_routing/contracts.rs` と `settingsRoleRoutingDefaults.ts`）を、モデル名を含まない「LARM 受付（backchannel）」「LARM 思考（llm）」にする。実際のモデル名は Phase 2 の診断で見る。保存済みの actor の表示名は書き換えない（4.6）。
4. 設定画面の Harness の Profile 欄の説明文に、`SAAA`（既定）と `SAAA-w-Image` / `SAAA-w-music` を入力できること、旧 ID は `SAAA` として扱われることを書く（`src/i18n/locales/jaSettings.ts`、`enSettings.ts`）。
5. `spec/docs/larm-http-api-review.md` の既定 Profile の記述を selector 方式に更新する。

チェックポイント:

```sh
cargo test --manifest-path src-tauri/Cargo.toml
bun test
bun run quality:check
```

## 10. Phase 5: 実機確認（Phase 4 完了後）

保存設定は変えない。接続先は保存済み Harness address（`http://192.168.0.130:9810`）。

1. `crates/larm-session/tests/live.rs` に `live_saaa_selector_two_llm_session`（`#[ignore]`）を追加する。`Variant(Conversation)` で接続し、次を確認して close する。
   - `selector() == Some("SAAA")`、`profile_id() == "saaa-conversation-ornith15"`
   - `provider_summary()` の `llm.model == "ornith-1.5-35b"`、`backchannel.model == "qwen3.5-2b-fast-response"`
   - `backchannel` と `llm` にそれぞれ `enable_thinking: false` で短い chat request を 1 回送り、200 が返る
   ```sh
   LARM_API_TOKEN=<token> cargo test --manifest-path crates/larm-session/Cargo.toml --test live live_saaa_selector_two_llm_session -- --ignored --nocapture
   ```
2. ユーザーがアプリで音声会話を 1 回行い、DB の**コピー**で次を確認する。
   ```sql
   SELECT ordinal, purpose, actor_id, status FROM rr_steps WHERE root_id=<直近の音声 run_id> ORDER BY ordinal;
   ```
   `0 frontend larm-frontdesk succeeded`、`1 respond larm-reasoner succeeded` になること。違えば `saaa-butler-role-routing.md` 2.6 の条件を上から確認する。
3. 診断画面の LARM 項目に、selector `SAAA` と 2 つの LLM の model が表示されること。

## 11. 完了条件

- Phase 1〜4 のチェックポイントがすべて通る。
- `voice_turn_runs_frontend_then_reasoner` と `text_turn_skips_frontend_provider` が、実機と同じモデル名の fixture で通る（音声は `backchannel` → `llm`、テキストは `llm` だけ。どちらも思考は `ornith-1.5-35b`）。
- `rg -n "ProfilePreference::Auto|AUTO_LABEL|AGENT_PROFILE|select_voice_profile|select_default_llm_profile" crates src-tauri/src services` が 0 件。
- `rg -n 'providers\[0\]|providers\.first\(\)' src-tauri/src/providers/dynamic_lan crates/larm-session/src` が 0 件。
- Phase 5 の live テストが通り、ユーザーが実機の音声会話で 10 章の手順 2 を確認している。
- 各 Phase の PR 説明に、変更ファイル、テスト結果、未解決事項を書いている。

## 12. 未解決事項（LARM 担当者に確認する）

| # | 質問 | 本計画での扱い |
|---|---|---|
| Q1 | 3 つの selector が同じ `id` を返し、Connection の request にも selector を渡す欄がない。image / music の Service を SAAA が使うときの認証と接続先は何か（claim に `services` は返らない） | variant は存在確認だけに使う（4.4）。media の呼び出しは範囲外 |
| Q2 | `llm` の contextWindow が 230400 から 131072 に変わった。今後も変わりうるか | SAAA は claim の値を使い、catalog と照合するだけなので、コード変更は不要 |
| Q3 | variant 切替中（image 34GB の起動中など）に、`profile=SAAA` の問い合わせや基本セットの health はどうなるか | 既存の `larm_unhealthy_provider` と capacity 待ちに任せる |

## 13. リスク

| リスク | 対策 |
|---|---|
| LARM を再起動しないと selector が反映されず、全 Profile が返る（2026-09-25 に実際に発生） | `requestedProfile` の一致と件数 1 の検証で `larm_catalog_invalid` になり、別の Profile では動かない。エラー文言で LARM の再起動を案内する |
| catalog と claim の食い違いで接続できなくなる | 黙って別モデルで動くより安全なので失敗させる。`larm_catalog_claim_mismatch` の文言で原因を示す |
| 旧 ID を保存している環境で、意図せず selector に切り替わる | 4.1 の読み替えは、SAAA がこれまで既定値として出荷した 3 つの ID だけに限る。それ以外は `Explicit` のまま |
| `Explicit` で `backchannel` を必須にしたため、既存の独自 Profile が接続できなくなる | 4.2 のとおり、要望が出たら止めて質問する。PR 説明に挙動の変更として書く |
| `larm-session` の API 変更で `services/reasoning-mcp` がコンパイルエラーになる | `connect_with_profile*` は `Explicit` のラッパーとして残す。`cargo test --manifest-path services/reasoning-mcp/Cargo.toml` を Phase 1 で実行する |
