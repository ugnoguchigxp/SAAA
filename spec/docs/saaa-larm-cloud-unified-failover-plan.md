# 在宅LARM／外出Cloud 統一フェイルオーバー実装計画

> 状態（2026-10-07）: P1〜P5 実装済み。判断事項 D1〜D3 は推奨案で確定。実装の差分は末尾「実装結果」を参照。

- 作成日: 2026-10-06（ブランチ `feat/self-diagnosis-v2`、HEAD `824e5fb5` + 未コミット変更あり）
- 関連: [docs/plans/purpose-based-cloud-api-switching.md](../../docs/plans/purpose-based-cloud-api-switching.md)（Purpose Registry の正計画）、[.archived/saaa-role-routing-location-switch-plan.md](.archived/saaa-role-routing-location-switch-plan.md)（旧方式・本計画で置換）
- 製品コンセプトの正本は ChatGPT Page「SAAAの全体コンセプト」。本計画は実装契約であり、コンセプトを変更しない。この計画書の作成時点では正本 Page を参照していない（未確認）。

## 1. ゴール／非ゴール

### ゴール
- 「在宅＝LARM、外出（LAN不達）＝Cloud（またはLARM以外の代替先）」を、**会話・ASR・TTS・画像・音楽の5 Purpose すべて**で、**単一の resolver** によって**リクエスト（ターン／発話／生成要求）単位**でユーザー操作なしに切り替える。
- 意図は binding の「primary＝LARM、fallback＝Cloud」だけで表す。recipe の二本立ては作らない。
- Cloud へ切り替わったら通知する。承認は求めない（旧計画の既定値を引き継ぐ）。
- 失敗は隠さない。両方とも使えない場合は明示的なエラーを返す。

### 非ゴール
- Cloud 側の到達性監視（Cloud の失敗はリクエストのエラーとしてそのまま返す）。
- 同じリクエストの中で LARM から Cloud へ切り替えること（§4.4）。ただし既存の Direct→Direct の初回フォールバックは維持する。
- Purpose 化されていない LARM 利用（記憶整理 `memory/personal_state`、World worker、埋め込み、TTS辞書プレビュー、Provider Unit Test）の Cloud 代替。これらは P6 以降の課題として扱う（§8）。
- ライブ ASR（qwen-realtime）を外出時の代替先にすること（§4.5）。
- DB スキーマの変更。Registry は JSON document であり、追加するフィールドは serde の default で後方互換にする。
- 音声 I/O（capture、VoiceProcessingIO、AEC、エコー除去、player）の変更。

## 2. 検証済みの現状（根拠）と暫定所見との差分

| # | 暫定所見 | 検証結果 | 根拠 |
|---|---|---|---|
| 1 | `resolve_route` は primary を返すだけ | **正しい**。primary の解決に失敗した場合（無効など）でも fallback へは進まない。これはテスト `disabled_resource_is_rejected_without_falling_back` で固定された意図 | `providers/service_registry/resolve.rs:37-55`、`tests.rs:175` |
| 2 | `fallback_resource_ids` は direct_route だけが実行時に使う | **正しい**。さらに強い制約がある。会話の fallback は **primary も fallback も Direct（ChatCompletions/Anthropic）の場合に限って** validation を通る。Media の fallback は validation で全面的に禁止されている | `validate.rs:145-175`、`direct_route.rs:41-55`、`purpose_completion.rs:115-127` |
| 3 | service_registry は reachability を参照しない | **正しい**。加えて **本番コードでは誰も reachability を読んでいない**。`ReachabilityState::snapshot()` は `cfg(any(test, feature="offline-contracts"))` の中にあり、watcher が書き込むだけになっている | `providers/reachability.rs:55-63`、`lib.rs:400` |
| 4 | 旧 role_routing が在宅/外出の自動切替を持つ | **本番では動いていない**。`availability::selection_input_for` は test/offline-contracts 限定。`record_provider_turn_start_in_transaction` は本番から呼ばれていない（呼出元はテストだけ）。旧会話ランタイムは撤去済み。本番の会話経路は `conversation_check::queue_runtime::process_conversation_answer` だけ | `role_routing/availability.rs:2-34`、`repository_turns/start.rs:17,127`、`runtime/turns/execute_turn.rs:92`、`quality_eval.rs:120` |
| 5 | media は adapter_kind で分岐し、自動切替はない | **正しい**。route は `ledger::reserve` で DB に固定され、`reconcile_media_generation` は保存済みの route で再開する。進行中のジョブは切替の影響を受けない | `media_generation/generation.rs:30-40,100,137`、`ledger.rs:16-30`、`recovery.rs:33-35,72` |
| 6 | `cloud_allowed=false` は resolve で拒否される | **正しい**（`resolve.rs:90`）。Media の既定値は `false`、会話と音声の既定値は `true` | `migration.rs:176-230` |
| 7 | （所見なし） | **見落とされていたブロッカー**。`security.runtime.localOnlyWhenSelected`（既定値 `true`）が、primary=local かつ fallback=cloud の組み合わせを**保存時と実行時の両方で拒否する**。この設定がある限り、ゴールの構成を保存できない | `service_registry/active.rs:17-34`、`persistence/service_registry_store.rs:204-240`、`settings/voice_fallbacks.rs:42`、`settings/documents.rs:368` |
| 8 | （所見なし） | 到達性の probe 先は **DynamicLan provider の host**。一方、Registry の LARM 接続 `conn:harness` の endpoint は `providers.harness.address`（URL）。両者の一致は保証されていない | `reachability_watcher.rs:64-79`、`migration.rs:41-49` |
| 9 | （所見なし） | ヒステリシスの穴。`Unknown`（起動直後、interface 変化の直後）から 1 回失敗しても `Unknown` のまま残り、`Unreachable` になるのは次の probe（最大 20 秒後）。外出先で起動した直後のターンは LARM 接続のタイムアウトまで待たされる（ASR は最大 120 秒） | `reachability.rs:66-80`、`conversation_check.rs:455-461` |
| 10 | （所見なし） | 音声の fallback は旧 `routing.tasks` へ投影される。`res:svc-*`（Registry で新規登録したサービス）は音声の fallback に使えない。LARM も音声の fallback には使えない | `settings/registry_projection.rs:12-55,72-80`、`service_registry_store.rs:61-81` |
| 11 | （所見なし） | ライブ ASR の方式（http / qwen-realtime）は**マイク開始時**にフロントエンドが決め、発話ごとには変わらない | `src/lib/conversationAsrCapture.ts:490`、`voice/qwen_realtime_asr.rs:98-105`、`qwen_realtime_asr/selection.rs` |
| 12 | （所見なし） | 実行時の resolve 呼出しは 5 か所だけ: direct_route（会話）、voice_routes::prepare（ASR/TTS。呼出元は conversation_check.rs:660、streaming_speech.rs:79、streaming_progress.rs:35、voice_routes::utterance）、qwen_realtime_asr（2 か所と selection.rs）、media generation.rs:32 | grep 結果 |
| 13 | （所見なし） | `providers/routing.rs::effective_conversation_route_ids` は本番から参照されていない（dead code） | grep 結果 |

結論として、在宅/外出の自動切替は現在**どの経路でも本番で動いていない**。role_routing の撤去は本番挙動に影響しない（§5 P5）。

## 3. 最終アーキテクチャ

```
reachability_watcher ──probe(conn:harness endpoint, 400ms, 20s間隔, IF変化で即時)──▶ ReachabilityState
        ▲                                                                       │ snapshot()
        │ report_larm_connect_failure()（実行時の接続失敗で即時再probe）          ▼
  各consumer ──(RegistrySnapshot, Purpose, LocalAvailability)──▶ service_registry::select_route
                                                                      │
             primaryを解決（設定エラーならそのままErr。fallbackへ進まない）
             primaryがLARM かつ larm==Unreachable → fallbackを順に解決
                 cloud && !cloud_allowed → 候補から外す / LARMの候補は外す
             候補なし → Err(LocalUnreachable{cloud_blocked})
                                                                      ▼
                                    RouteCandidates{selected(selection付き), remaining}
     ┌───────────────┬───────────────┬────────────────┬─────────────────────┐
  会話(direct_route) ASR/TTS(voice_routes) live ASR(qwen)  media(generation.rs→ledgerで固定)
  adapter_kindで     既存のprovider投影         同じselected     adapter_kindで分岐（変更なし）
  Larm/Direct分岐    （変更なし）                 を使う
```

### 責務
- **ReachabilityState**: LARM ハーネス 1 系統の到達性だけを持つ（§4.2）。
- **select_route**: 到達性と binding から、そのリクエストで使う route を 1 つ決める唯一の場所。I/O は行わない純関数。
- **consumer**: 返された route の `adapter_kind` に従って実行する。consumer は自分で到達性を判定しない。
- **validate_active**: 実行中に設定が撤回されていないかを確認する（到達性は見ない）。

### 削除するもの
- `role_routing/availability.rs`、`SelectionInput.unreachable_actor_ids`、reason code `actor_unreachable`/`location_fallback`、および関連テスト（`ipc_receiver_tests.rs:34` の文字列を含む）。
- `providers/routing.rs::effective_conversation_route_ids`（dead code）。
- `localOnlyWhenSelected` による cloud fallback の拒否（4 か所）。判断事項 D1 を参照。
- `resolve_route` の旧シグネチャ（移行用の一時ラッパーとして残し、P5 で削除する）。
- `validate.rs` の「media の fallback を禁止」「会話の fallback は Direct の primary に限る」の 2 規則。代わりに「LARM は primary にだけ置ける」を全 Purpose 共通の規則にする。

## 4. 設計判断

### 4.1 選択規則（全 Purpose 共通）
1. binding が `enabled=false`、`NeedsReview`、または primary 未設定の場合は従来どおりエラーにする。会話の NeedsReview は従来どおり `legacy_harness` で処理する（`direct_route.rs:91`）。
2. primary を `resolve_resource` で解決する。Err の場合はそのまま返し、fallback へは進まない（既存の不変条件）。
3. primary の connection が `adapter_kind == Larm` で、かつ `larm == Unreachable` の場合は、fallback を順に試す。それ以外の場合は primary を選ぶ（`Unknown` は到達できるものとして扱う）。
4. fallback は `resolve_resource` が Ok で、かつ LARM でないものだけを対象にする。Cloud の候補が `cloud_allowed=false` で拒否された場合は `cloud_blocked=true` を記録して次の候補へ進む。その他のエラーも同じく次の候補へ進む（監査にスキップ理由を残す）。
5. 候補が尽きたら `Err(LocalUnreachable { purpose, cloud_blocked })` を返す。到達できないと分かっている LARM へは送らず、すぐに明示エラーを返す（旧計画の「LARM を選んで失敗させる」から変更。判断事項 D3）。
6. SystemTts（location=local、adapter≠Larm）は外出時でも使える正当な TTS の代替先になる。スキップの条件は `location` ではなく `adapter_kind == Larm` で判定する。

### 4.2 到達性の粒度：単一で足りる
LARM の LLM/ASR/TTS/画像/音楽はすべて `conn:harness`（1 つのハーネス endpoint）の背後にある（`compatibility.rs:11-34` で、LARM を許可しているのは `conn:harness` だけ）。サービス別の可否は LARM 内部の capacity であり、実行時エラーとして扱う。そのため、単一の `Reachability` を `adapter_kind==Larm` のすべての connection に適用する。

あわせて次の 2 点を変更する。
- probe 先を `providers.harness.address`（`conn:harness` の endpoint）にする。`dynamic_lan::probe::reachable_at(Url)` を使い、URL の検証には `service_harness::descriptor::validate_address` を使う。
- ヒステリシス: Reachable→Unreachable は「2 回連続の失敗」のまま。**Unknown→Unreachable は 1 回の失敗で確定**させる。到達への復帰は 1 回の成功で確定する（既存どおり）。

### 4.3 「両方不達」
Cloud の到達性は観測しない。LARM が不達で Cloud を選んだ後に Cloud も失敗した場合は、Cloud adapter のエラーをそのまま返す（既存の `ProviderFailureKind::public_message`）。LARM が不達で、許可された候補がない場合は `LocalUnreachable` を返す。

### 4.4 実行中の失敗
- 切り替えるのは**次のリクエストから**。同じリクエスト内で LARM から Cloud へは切り替えない（Tool 実行、部分公開、発話の二重配信、二重生成の問題を避け、失敗を見せる）。
- LARM への**接続段階**の失敗（`cached_larm_asr`/`connect_larm` の Err またはタイムアウト）が起きたら、`ReachabilityState::report_larm_connect_failure(now)` を呼ぶ。これは `record(false)` を記録して kick を送る。Reachable 状態から kick による probe がもう一度失敗すると 2 連続失敗になり、ほぼ即時に `Unreachable` へ移る。こうして次のターンは Cloud を使う。モデルエラーや capacity エラーでは呼ばない。
- 既存の Direct→Direct の初回フォールバック（`purpose_completion.rs:115-127`）は維持する。`remaining` は selected より後ろの候補のうち Direct のものだけにする。
- 進行中のジョブ: media は ledger の route（`route_json`）で再開するため影響しない。音楽の resume と画像の照会は、ジョブを始めた adapter でしか行わない。ASR は発話単位で固定され（`voice_routes::utterance`）、TTS は発話内で固定される。会話は job 開始時に固定される（`validate_active` は撤回だけを検出する）。

### 4.5 ASR の制約
外出時の ASR 代替先は **HTTP（発話単位）の ASR に限る**。qwen-realtime はマイク開始時にしか方式が反映されないため、fallback に指定すると保存時に拒否する（`registry_projection.rs`）。primary が qwen-realtime（Cloud）の構成は LARM と無関係なので対象外とする。

### 4.6 `cloud_allowed` と通知
- 唯一のゲートは binding ごとの `cloud_allowed` にする。`localOnlyWhenSelected` の扱いは判断事項 D1 を参照。
- 既定値は変えない。会話と音声は `true`（ただし fallback が空なので、何も送られない）、Media は `false`。外出時に Cloud を使うには、fallback の追加と（Media の場合は）`cloud_allowed` の ON が必要になる。この 2 つの明示操作を同意とみなす。既存データの書き換え、移行、needs-review の追加は不要。
- 通知: `ResolvedRoute.selection` を監査の attributes と `latestUsage` に載せる。`get_service_registry` に `larmReachability` を追加する。チャット画面には「LARMに接続できないためクラウドで応答」というバッジを出す。承認は求めない。

## 5. 契約（共有型・シグネチャ）

```rust
// providers/reachability.rs
impl ReachabilityState {
    pub(crate) fn snapshot(&self) -> ReachabilitySnapshot;          // cfg を外して本番で使う
    pub(crate) fn record(&self, ok: bool, now: Instant);            // Unknown→Unreachable は1回の失敗で確定
}
// AppState 側ヘルパ（reachability_kick を持つため app_state 近傍 or reachability_watcher.rs）
pub(crate) fn report_larm_connect_failure(state: &AppState);       // record(false)+kick.notify_one()

// providers/service_registry/resolve.rs
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct LocalAvailability { pub(crate) larm: Reachability }
impl From<&ReachabilitySnapshot> for LocalAvailability { /* larm = snapshot.harness */ }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum RouteSelection { #[default] Primary, LocalUnreachable }

pub(crate) struct ResolvedRoute {
    /* 既存フィールドは不変 */
    #[serde(default)] pub(crate) selection: RouteSelection,       // media ledger の旧行は Primary として読む
}

pub(crate) struct RouteCandidates {
    pub(crate) selected: ResolvedRoute,
    /// selected より後ろで解決できた非LARM候補（会話の Direct→Direct 初回fallback用）
    pub(crate) remaining: Vec<ResolvedRoute>,
}

pub(crate) enum ResolveError {
    /* 既存 */ NotConfigured(Purpose), Disabled(Purpose), NeedsReview(Purpose),
    ResourceDisabled(String), ConnectionDisabled(String), Invalid(String),
    CloudNotAllowed(Purpose),                                        // resolve.rs:90 の Invalid を分離
    LocalUnreachable { purpose: Purpose, cloud_blocked: bool },
}

pub(crate) fn select_route(snapshot: &RegistrySnapshot, purpose: Purpose,
                           availability: LocalAvailability) -> Result<RouteCandidates, ResolveError>;
pub(crate) fn resolve_route(snapshot: &RegistrySnapshot, purpose: Purpose,
                            availability: LocalAvailability) -> Result<ResolvedRoute, ResolveError>;
    // = select_route(..).map(|c| c.selected)
// 移行用（P2〜P4の間だけ存在し、P5で削除）:
pub(crate) fn resolve_route_primary(snapshot, purpose) -> Result<ResolvedRoute, ResolveError>;
    // = resolve_route(.., LocalAvailability::default())  ※Unknown → 現在の挙動と同一

// runtime/conversation_check/voice_routes.rs
pub(super) fn prepare(db: &rusqlite::Connection, purpose: Purpose,
                      availability: LocalAvailability) -> Result<PreparedVoice, String>;
```

ユーザー向けメッセージ（日本語）: `LocalUnreachable{cloud_blocked:false}` は「LARMに接続できません。外出時の代替先が設定されていません」。`cloud_blocked:true` は「LARMに接続できません。代替先へのクラウド送信がこの用途で許可されていません」。

監査: `operations::attributes` に `"selection"` と `"location"` を追加する（`operations.rs:6`）。

## 6. フェーズとワークストリーム

各フェーズは単独でビルドとテストができ、P2 までは本番挙動を変えない。

### P1 到達性（W1）— 依存なし。W2 と並列で進められる
- 書込み範囲: `src-tauri/src/providers/reachability.rs`、`src-tauri/src/providers/reachability_watcher.rs`
- 内容: `snapshot()` の cfg を外す。Unknown→Unreachable を 1 回の失敗で確定させる。probe 先を `providers.harness.address` に変える。`report_larm_connect_failure(&AppState)` を watcher 側に追加する。
- テスト: `cargo test -p saaa reachability`（rr_ls_01〜03 を更新し、`rr_ls_04_unknown_first_failure_is_unreachable` を追加）

### P2 resolver と validation（W2）— 依存なし。W1 と並列で進められる。**分割しない**
- 書込み範囲: `providers/service_registry/{resolve.rs, validate.rs, operations.rs, tests.rs, commands.rs}`、`providers/service_registry.rs`（re-export。W2 が所有）
- 内容: §5 の型と `select_route` を実装する。既存の 6 か所の呼出元は `resolve_route_primary` に機械的に置き換える。置き換えは W2 が行い、呼出元ファイルには 1 行の差分しか出さない（conversation_check.rs は含まない。voice_routes::prepare の内部で置き換える）。validate.rs の規則変更: LARM の resource は primary にだけ置けるという全 Purpose 共通規則を入れる。Media の fallback 禁止と、会話の「Direct の primary に限る」は削除する。operations::attributes に selection/location を追加する。commands.rs の `get_service_registry` に `larmReachability`（`state.reachability.snapshot()`）を追加する。
- テスト: `cargo test -p saaa service_registry`（§7.1）

### P3 consumer の配線（W3/W4/W5 は並列で進められる。P1・P2 の完了が前提）
- **W3 会話**: `runtime/conversation_check/direct_route.rs`、`queue_runtime/purpose_deadline.rs`（未コミット変更あり）、`terminal_decision.rs`
  - `prepare_transport(state)` で `state.reachability.snapshot()` を読み、`select_route` を呼ぶ。`fallbacks = remaining` の Direct だけにする。`legacy_harness` は変えない。
  - LARM 接続失敗の通知（`report_larm_connect_failure`）は `queue_runtime/conversation_answer.rs:43-47`（未コミット変更あり）の `cached_larm_asr` が Err のときに入れる。**このファイルは呼出元（私）が統合時に 1 か所だけ編集する**。
- **W4 音声**: `voice_routes.rs`、`streaming_speech.rs`、`streaming_progress.rs`、`voice/qwen_realtime_asr.rs`、`voice/qwen_realtime_asr/selection.rs`、`persistence/settings/registry_projection.rs`、`persistence/service_registry_store.rs`（`overlay_legacy` だけ）
  - `prepare(db, purpose, availability)`。`utterance()` の内部で snapshot を取る。
  - `selected_provider` と `start_qwen_asr_session` の pinned route も同じ `resolve_route(..., availability)` を使う。
  - 投影: primary または fallback のどれかが `res:svc-*` の binding は Registry を正とし、投影をスキップする。`overlay_legacy` の判定もそれに合わせる。qwen-realtime を fallback に指定したら拒否する。
  - `runtime/conversation_check.rs:660`（未コミット変更あり）の 1 行（prepare の引数追加）と、harness ASR の接続失敗時の `report_larm_connect_failure`（`conversation_check.rs:455-461`）は**呼出元が統合時に編集する**。
- **W5 Media**: `media_generation/generation.rs` だけ
  - `resolve_route(..., state.reachability.snapshot().into())` を使う。recovery.rs は保存済み route を使うので変更しない。
- テスト: W3 は `cargo test -p saaa --features conversation-queue-e2e conversation_queue_e2e::cloud_route`。W4 は `cargo test -p saaa --features conversation-queue-e2e voice_routes`。W5 は `cargo test -p saaa media_generation`。

### P4 ポリシーと UI（W6 Rust 側、W7 フロントエンド。P2 の完了が前提）
- **W6**: `providers/service_registry/active.rs`、`persistence/service_registry_store.rs`（`reject_cloud_while_local_only` の削除だけ。W4 の後で順番に作業する。同じファイルを同時に扱わない）、`persistence/settings/voice_fallbacks.rs`、`persistence/settings/documents.rs:357-372`
  - D1 が推奨案で確定した場合は、localOnly の判定を 4 か所から削除する。保存済みの値は残す（初期化しない）。
- **W7**: `src/features/settings/PurposeRouteDetails.tsx`（`canFallback` を全 Purpose に広げ、候補から LARM を除外し、説明文を「LARMに接続できない時（外出時）に上から順に使用」にする）、`PurposeRoutesSection.tsx`（selection の表示）、`SecuritySection.tsx`（D1 に従ってチェックボックスを撤去）、`src/lib/serviceRegistry.ts`（`selection`、`larmReachability` 型）、新規 `src/features/chat/RouteLocationBadge.tsx`
  - `ConversationCheckPage.tsx`（未コミット変更あり）へのバッジ設置（1 行）は**呼出元が統合時に行う**。
- テスト: W6 は `cargo test -p saaa persistence::settings service_registry_store`。W7 は `npx vitest run tests/settings.test.ts` と新規の `src/features/settings/PurposeRouteDetails.test.tsx`。

### P5 旧系統の撤去（W8。P3 の完了が前提）
- 書込み範囲: `role_routing/availability.rs`（削除）、`role_routing/mod.rs`（mod 行。W8 が所有）、`role_routing/selection.rs`、`role_routing/repository_turns/start.rs`、`role_routing/README.md`、`ipc_receiver_tests.rs:34`、`providers/routing.rs`（dead fn）、`providers/service_registry/resolve.rs`（`resolve_route_primary` の削除。残っている呼出元がないことを grep で確認する）
- テスト: `cargo test -p saaa role_routing`。その後、呼出元が全体ゲートを実行する。

### 共有／統合ファイル（呼出元が worker の完了後に編集する）
- `src-tauri/src/runtime/conversation_check.rs`（:660 の prepare 引数、:455-461 の接続失敗通知）
- `src-tauri/src/runtime/conversation_check/queue_runtime/conversation_answer.rs`（:43-47 の接続失敗通知）
- `src/features/chat/ConversationCheckPage.tsx`（バッジ設置）
- `src-tauri/src/lib.rs`、`ipc_contract.rs`、生成バインディングは**変更不要の見込み**（新規 tauri command はなく、`get_service_registry` は手書きの invoke を使う）。変更が必要になった場合だけ呼出元が編集する。
- `docs/plans/purpose-based-cloud-api-switching.md` の §障害と代替先、§実装状況（会話 fallback の制約緩和、Media の fallback）、`providers/README.md`、`spec/docs/INDEX.md` への追記。

注意: 上記の未コミットファイル（purpose_deadline.rs を含む）は他の作業中の変更を含む。worker を割り当てる前に `git status` を確認し、差分を巻き戻したり上書きしたりしない。

## 7. テスト計画

### 7.1 resolver の単体テスト（`providers/service_registry/tests.rs`、純関数で決定的）
各 Purpose（会話、ASR、TTS、画像、音楽）について、テーブル駆動で次を確認する。
| ケース | 入力 | 期待 |
|---|---|---|
| fo_01 LARM到達 | primary=harness、fb=cloud、larm=Reachable | selected=harness、selection=Primary |
| fo_02 Unknown | 同上、larm=Unknown | harness |
| fo_03 LARM不達 | 同上、larm=Unreachable | selected=cloud、selection=LocalUnreachable |
| fo_04 両方不可 | fb なし、Unreachable | `LocalUnreachable{cloud_blocked:false}` |
| fo_05 cloud_allowed=false | fb=cloud、`cloud_allowed=false`、Unreachable | `LocalUnreachable{cloud_blocked:true}`。cloud の route は返さない |
| fo_06 primary 無効 | primary が disabled、Unreachable | `ResourceDisabled`（fallback へ進まない。既存テストを維持） |
| fo_07 Cloud primary | primary=cloud、Unreachable | cloud（到達性の影響を受けない） |
| fo_08 SystemTts | TTS の fb=SystemTts、Unreachable | SystemTts |
| fo_09 validation | fallback に LARM、qwen-realtime を fb に指定 | 保存エラー |
| fo_10 fingerprint | primary と fallback の route | 各 resource の fingerprint が `resolve_resource` と一致する（`validate_active` と `latest` の整合） |
| fo_11 media ledger 互換 | `selection` のない旧 `route_json` | Primary として deserialize される |

**ヒステリシス**（`reachability.rs` と resolver の結合、決定的な時刻を注入する）:
- Reachable → 失敗 1 回 → harness のまま。失敗 2 回 → cloud。成功 1 回 → harness に戻る。
- Unknown → 失敗 1 回 → cloud（rr_ls_04）。
- `report_larm_connect_failure` の後に probe が失敗 → Unreachable（kick が送られることを Notify で確認する）。

### 7.2 consumer の結合テスト（`conversation-queue-e2e` feature。fixture サーバーを使い、実 LARM は不要）
- 会話: `ReachabilityState` に Unreachable を注入する → Direct の fixture に 1 回だけ POST され、LARM セッションは要求されない（`cloud_route.rs` に追加）。Reachable の場合は LARM fixture を使う。両方不可の場合は `LocalUnreachable` で job が失敗し、回答は保存されない。
- ASR: Unreachable → `voice_routes::utterance` が HTTP ASR fixture に固定される。同じ発話の途中で Reachable に戻っても、final は同じ fixture に送られる（既存テスト `custom_asr_keeps_the_same_provider...` と同じ形）。
- TTS: Unreachable → CloudTts/SystemTts の route を返す。発話内のチャンクは固定される。
- 画像と音楽: Unreachable → Replicate の fixture へ送られ、ledger の `route_json.selection=local-unreachable` になる。途中で Reachable に戻しても、`reconcile_media_generation` は Replicate だけに照会する（既存の `replicate_tests.rs` の形）。
- `cloud_allowed=false`: 各 consumer がエラーを返し、Cloud fixture への呼出し回数が 0 であることを確認する。

### 7.3 音声契約の非回帰（本計画は音声 I/O に触れないが、確認する）
- 自動テスト: `cargo test -p saaa echo_reference::tests::subtracts_matching_playback_and_preserves_overlapping_speech`、`cargo test -p saaa diagnosis::checks::voice`、`npx vitest run tests/conversation-asr-continuous.test.ts tests/ambient-native-voice-capture.test.ts tests/microphone.test.ts`。
- 差分の確認: `git diff --stat` の結果に `src-tauri/src/voice/audio_backend/`、`src/lib/conversationAsrCapture.ts`、player 関連のファイルが含まれないこと。
- 実機（P8 の受入に含める）: LARM を停止して TTS を Cloud/SystemTts にした状態で (a) TTS の再生だけでは ASR 発話が作られない、(b) 再生中に話した声が ASR（Cloud）に届く、の 2 条件を確認する。

### 7.4 全体ゲート（呼出元が最後に 1 回だけ実行する）
`cargo test -p saaa`、`cargo test -p saaa --features conversation-queue-e2e`、`cargo clippy --all-targets`、`npx vitest run`、`npm run typecheck`（スクリプト名は package.json で確認すること。未確認）。

## 8. 受け入れ基準（LARM を落とした状態で全機能が Cloud で動く手順）
1. 設定のコピーを使い（実 DB を書き換えない）、Cloud LLM、HTTP ASR、HTTP TTS（または SystemTts）、Replicate の画像と音楽を登録する。各 Purpose で primary=LARM、fallback=Cloud とし、画像と音楽は `cloud_allowed=ON` にする。
2. 在宅（LARM 稼働中）: 会話、音声入力、読み上げ、画像生成、音楽生成がすべて LARM で動く。`latestUsage.selection=primary`。
3. LARM ハーネスを停止する（または LAN を切断する）。40 秒以内（interface が変化した場合は 1 秒以内）に `larmReachability=unreachable` になる。
4. 次のリクエストから、5 機能すべてが Cloud（または SystemTts）で完了する。チャットにバッジが表示され、`latestUsage.selection=local-unreachable` になる。承認ダイアログは出ない。
5. LARM 停止の直後の 1 ターンが接続エラーになった場合、そのエラーは画面に表示される（隠されない）。次のターンは Cloud で動く。
6. Cloud の API キーを無効にした状態では、明示的なエラーになる（両方不可）。画像の `cloud_allowed` を OFF にすると「クラウド送信が許可されていません」と表示される。
7. LARM を再開すると、1 回の probe 成功の後のリクエストから LARM に戻る。生成中の音楽ジョブは、始めたサービスで完了または照会される。
8. 音声契約の 2 条件（§7.3）が実機で成立する。

## 9. リスク／未確認事項
- **R1** `report_larm_connect_failure` を接続段階の失敗だけに限定する判定は、`cached_larm_asr` が返すエラー文字列に依存する。型付きのエラー分類がないため、最初の実装は「`cached_larm_asr` が Err を返したら必ず通知する」とする（誤検知があっても次の probe で回復するため安全）。
- **R2** 停止直後に pin された ASR 発話は失敗し、その発話は失われる。フロントエンドの `earlyFailure` 表示で失敗が見えることは未確認。
- **R3** probe 先を変えると、DynamicLan provider の host と harness.address が異なる構成で観測結果が変わる。dynamic_lan の control_credential と `connect_larm` の credential が同一であることは未確認。
- **R4** Purpose 化されていない LARM 利用（personal_state worker、World、埋め込み、`tts_dictionary.rs:233` のプレビュー）は外出時に失敗する。これらが失敗をどう扱うか（リトライなのか、うるさく通知するのか）は未確認。
- **R5** 会話の NeedsReview の binding は `legacy_harness`（LARM 固定）のままなので、外出時は失敗する。UI で「適用が必要」と示されることを P4 で確認する。
- **R6** 本ブランチには他の作業中の未コミット変更がある（会話キュー、personal_state）。P3 は統合時に衝突する可能性がある。

## 10. 判断事項（推奨案つき）
- **D1 `localOnlyWhenSelected` の扱い** — 推奨: **cloud fallback の拒否規則から外し、binding ごとの `cloud_allowed` に一本化する**。保存値は残し、UI のチェックボックスは撤去する。既存の設定では local primary に cloud fallback を保存できなかったため、この変更だけで Cloud への送信が始まる構成は存在しない。代替案: グローバルの「外出時クラウド代替を許可」として残す（ゲートが 2 つになる）。
- **D2 Cloud 切替時の通知のみ・承認なし** — 推奨: 旧計画の既定値を踏襲する（承認ダイアログは出さない）。
- **D3 両方不可のときの挙動** — 推奨: 到達できないと分かっている LARM へは送らず、`LocalUnreachable` で即時にエラーを返す（旧計画は「LARM を試して失敗させる」だった。ASR は最大 120 秒待たされるため変更する）。

## 11. 実装結果（2026-10-07）

計画との主な差分:
- `RouteCandidates` と `resolve_route_primary` は作らなかった。`resolve_route(snapshot, purpose, LocalAvailability) -> ResolvedRoute` の1関数にし、会話の残り候補は `direct_route::remaining_fallbacks` が `fallback_resource_ids` から導く。
- probe は `providers.harness.address` への HTTP GET（応答があれば到達。資格情報は送らない）。Unknown からの失敗1回で Unreachable、Reachable からは連続2回（既存どおり）。
- LARM 接続失敗の通知（`reachability_watcher::report_larm_connect_failure`）は会話応答・ASR・TTS の接続箇所（`note_larm_connect`）から呼ぶ。
- 音声 Purpose は、primary か fallback に `res:svc-*` がある場合は Registry が正（`registry_projection::registry_owned`）。LARM の ASR 代替先に qwen-realtime は指定不可。
- D1: `localOnlyWhenSelected` による cloud fallback の拒否を Rust 4 か所と TS（`schemas.ts`）から削除。保存値は残し、Security 画面のチェックボックスは撤去。ゲートは binding の `cloud_allowed` のみ。
- 設定画面: LARM の接続状態の表示、LARM を primary とする全用途での代替先の選択、クラウド代替先で `cloud_allowed` が OFF のときの警告、直近の利用先に「LARMに接続できないため代替先」を表示。チャット画面に `RouteLocationBadge`。
- P5: `role_routing/availability.rs`、`SelectionInput.unreachable_actor_ids`、`actor_unreachable`/`location_fallback` の生成、`providers/routing.rs::effective_conversation_route_ids` を削除。
- 未実施: 実機（LARM 停止）での受け入れ手順 §8、音声契約の実機2条件 §7.3、Purpose 化されていない LARM 利用（§9 R4）。
