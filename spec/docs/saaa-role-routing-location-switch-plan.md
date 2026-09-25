# Role Routing: 在宅 LARM / 外出 Cloud の自動切替 実装計画

状態: **Phase 1〜2 実装済み**（2026-09-22）。Phase 3（進行中ターンの途中切替）は未接続。
関連: [Role Routing 計画](saaa-role-routing-plan.md) / [作業カード](saaa-role-routing-work-cards.md) / [実行契約](saaa-role-routing-execution-contract.md)

## 0. 目的と前提

### 目的

- 家(LAN 内に Provider Harness = LARM がある)では LARM actor で応答する。
- 外出先(LAN 不達)では Cloud API actor で応答する。
- 切替をユーザーが操作しない。Wi‑Fi が変わったら次のターンから自動で相手が変わる。

### 決定済みの既定値(変更する場合はこの節だけ直す)

| 項目 | 既定値 | 理由 |
| --- | --- | --- |
| 切替の粒度 | **ターン単位**。受付時(`prepare_runtime_run`)に到達性を見て actor を選ぶ。進行中ターンの途中切替は Phase 3 で扱い、Phase 1〜2 では行わない | まず確実に動く単位から。途中切替は `Resume` 経路の本番接続が前提 |
| Cloud 切替時の通知 | **通知するが承認は求めない**。監視パネルと chat 上のバッジで「Cloud で応答」と出す。`premium_approval` は使わない | 「シームレス」要件を優先。課金先が変わる事実は見えるようにする |
| ヒステリシス | LAN→不達は **連続 2 回失敗**(各 400ms timeout)で確定。不達→到達は **1 回成功**で確定 | 一瞬の切断で Cloud に飛ばない。帰宅時は即 LARM に戻る |
| 両方不達 | 従来どおり失敗。Cloud が無いなら LARM を選び、LARM 失敗をそのまま返す | 隠さない(providers/README の「never hide a failed provider」) |
| RoleRouting 無効時 | 本計画の対象外。legacy `fallback_provider_ids` の挙動は変えない | スコープ限定 |

### 変更しないこと

- receipt の不変性: `rr_decisions.selected_id` と policy snapshot は受付後に書き換えない。
- `executor::permit_next_step` の権威: 再選択はしない。到達性は **選択前**に入力として渡し、permit は今までどおり claim 済み step の検証だけを行う。
- `reducer.rs` の遷移表。Phase 3 でも既存の `Resume` を使い、新イベントは追加しない。

## 1. 全体像

```
[NetworkWatcher (新規)] --観測--> [ReachabilityState (新規, AppState 内メモリ)]
                                            |
                                            v (snapshot を SelectionInput に変換)
prepare_runtime_run ──> record_provider_turn_start_in_transaction
                            └─> select_dispatch_candidate(policy, input)   ← *_with_input に置換
                                    └─> candidates_for_action_with_input   ← actor_unreachable を付与
                                            └─> rr_decisions.candidates_json に理由が残る
apply_enabled_role_route ──> (receipt の selected_id を使う。変更なし)
                         └─> validate_actor_host ← 到達性の最終確認を追加(失敗は明示エラー)
```

recipe を 2 本用意する(設定側):

- `respond-home`: `roles: ["reasoner"]`、`reasoner` = LARM actor(`location: local`, `provider_id: DYNAMIC_LAN_PROVIDER_ID`)
- `respond-away`: `roles: ["advanced"]`、`advanced` = Cloud actor(`location: cloud`)

recipe id は辞書順で選ばれる(`selection.rs` の `min_by(recipe_id)`)。`home` < `away` ではないので **id に接頭辞を付けて順序を固定する**: `10-respond-home`, `20-respond-away`。`valid_id` は `-` と数字を許す。

## 2. Phase 1: 到達性の観測(Rust, providers 層)

### 2.1 新規ファイル `src-tauri/src/providers/reachability.rs`

```rust
//! LAN 上の Provider Harness への到達性を観測する。選択ロジックへは snapshot だけを渡す。
use std::sync::{Arc, RwLock};
use std::time::Instant;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Reachability { Unknown, Reachable, Unreachable }

#[derive(Debug, Clone)]
pub(crate) struct ReachabilitySnapshot {
    pub(crate) harness: Reachability,
    pub(crate) observed_at: Option<Instant>,
    pub(crate) consecutive_failures: u8,
}

#[derive(Default)]
pub(crate) struct ReachabilityState { inner: RwLock<Inner> }
struct Inner { harness: Reachability, observed_at: Option<Instant>, consecutive_failures: u8 }

impl ReachabilityState {
    pub(crate) fn snapshot(&self) -> ReachabilitySnapshot;
    /// 成功 1 回で Reachable、失敗は FAILURE_THRESHOLD(=2) 連続で Unreachable。
    pub(crate) fn record(&self, ok: bool, now: Instant);
    /// 起動直後・ネットワーク変化直後に Unknown へ戻す。
    pub(crate) fn invalidate(&self);
}

pub(crate) const FAILURE_THRESHOLD: u8 = 2;
pub(crate) const PROBE_TIMEOUT_MS: u64 = 400;
pub(crate) const PROBE_INTERVAL_SECS: u64 = 20;
```

`record` の状態遷移:

| 現状態 | ok | 次状態 |
| --- | --- | --- |
| 任意 | true | `Reachable`, failures=0 |
| `Reachable`/`Unknown` | false | failures+1。failures ≥ 2 なら `Unreachable`、それ以外は現状態維持 |
| `Unreachable` | false | `Unreachable` 維持 |

単体テスト(同ファイル `#[cfg(test)]`):

- `rr_ls_01_two_failures_flip_to_unreachable`: Reachable → false → まだ Reachable → false → Unreachable。
- `rr_ls_02_one_success_restores`: Unreachable → true → Reachable, failures=0。
- `rr_ls_03_invalidate_returns_unknown`.

### 2.2 軽量プローブ `src-tauri/src/providers/dynamic_lan/probe.rs` に追加

既存 `probe()` は `DynamicLanConnection::resolve` を呼び **allocation を claim する**ので到達性チェックには重い。claim しない関数を追加する。

```rust
/// control plane の state endpoint に GET するだけ。claim も model probe もしない。
pub(crate) async fn reachable(host: &str, timeout: std::time::Duration) -> bool
```

実装手順:

1. `src-tauri/src/providers/dynamic_lan/urls.rs` の `control_base_url(host)` と、`mod.d/02.rs` の `resolve_once` が state 取得に使っている URL 組立て関数を再利用する(関数名は `urls.rs` を開いて確認。`state` を含む名前)。
2. `reqwest::Client::builder().connect_timeout(timeout).timeout(timeout).no_proxy().redirect(none)` で GET。
3. 認証ヘッダは `resolve_at` と同じ `control_credential()` を付ける(`auth.rs`)。付けないと 401 になり到達しているのに false になる。
4. `2xx` なら true。それ以外(タイムアウト、DNS 失敗、5xx)は false。エラー種別のログは `tracing::debug!` に留める。

テスト: `mod.d/04.rs` の fixture(`resolve_world_fixture` が使う `Url` ベース)に合わせ、`reachable_at(base: Url, ...)` を `#[cfg(test)]` で公開し、モックサーバーで 200 → true、接続拒否 → false を確認する。

### 2.3 監視タスク `src-tauri/src/providers/reachability_watcher.rs`(新規)

```rust
pub(crate) fn spawn(state: Arc<AppState>) -> tokio::task::JoinHandle<()>
```

- ループ: `PROBE_INTERVAL_SECS` ごとに `load_model_providers` から `kind == dynamic-lan && enabled` の host を取り、`reachable(host, 400ms)` を呼んで `state.reachability.record(...)`。
- dynamic-lan provider が無い/無効なら `invalidate()` して sleep。
- 追加トリガ: `tokio::sync::Notify` を `AppState.reachability_kick` として持ち、以下から `notify_one()` する。
  - 設定保存(`persistence/settings.d/*.rs` の providers 保存経路)
  - ネットワークインターフェース変化(2.4)
  - ターン受付前(2.5 の「即時プローブ」)
- 起動時は最初のプローブが終わるまで `Unknown`。

登録場所: `src-tauri/src/lib.d/02.rs` の `role_routing::recovery::reconcile_startup` を呼んでいる setup 付近で `spawn` する。`larm_voice` のオーナー起動と同じ位置に置く。

### 2.4 ネットワーク変化検知(最小実装)

macOS 固有 API は使わない。`PROBE_INTERVAL_SECS` を待たずに反応させるため、インターフェース一覧のハッシュを 3 秒ごとに比較する。

- crate 追加なしで済ませる: `std::process::Command::new("ifconfig")` は使わない(サンドボックス/コスト)。`if-addrs` crate(軽量、MIT)を `Cargo.toml` に追加し、`if_addrs::get_if_addrs()` の `(name, ip)` 集合をハッシュ化。
- 差分があれば `reachability.invalidate()` + `reachability_kick.notify_one()`。

`Cargo.toml` の依存追加は 1 行。ライセンス表記が必要なら `THIRD_PARTY` 相当ファイルの既存書式に従う。

### 2.5 `AppState` への追加(`src-tauri/src/app_state.rs`)

```rust
pub(super) reachability: Arc<crate::providers::reachability::ReachabilityState>,
pub(super) reachability_kick: Arc<tokio::sync::Notify>,
```

`AppState` を構築している全箇所(本番 `lib.d/01.rs` とテスト fixture。`rg 'AppState \{'` で列挙)に初期値を追加する。

## 3. Phase 2: 選択への接続(role_routing 層)

### 3.1 `selection.rs`: `SelectionInput` 拡張と新 reason code

```rust
pub(crate) struct SelectionInput {
    pub(crate) cloud_allowed: bool,
    pub(crate) required_capabilities: HashSet<String>,
    pub(crate) estimated_cost_micros: HashMap<String, Option<u64>>,
    pub(crate) sticky_actor_id: Option<String>,
    /// 到達不能と観測された actor id。Unknown は含めない(= 楽観)。
    pub(crate) unreachable_actor_ids: HashSet<String>,   // 追加
}
```

`candidates_for_action_with_input` の reason_codes 判定に追加(`cloud_forbidden` の直後):

```rust
if actors.iter().any(|actor| input.unreachable_actor_ids.contains(&actor.id)) {
    reason_codes.push("actor_unreachable".into());
}
```

`candidates_for_action`(input なし版)は **削除せず**、`unreachable_actor_ids: HashSet::new()` のデフォルトで従来挙動を維持する。

テスト追加(`selection.rs`):

- `rr_ls_10_unreachable_actor_excludes_home_recipe`: actors `larm(local)`, `cloud(cloud)`; recipes `10-respond-home(reasoner=larm)`, `20-respond-away(advanced=cloud)`; `unreachable_actor_ids={larm}` → 先頭候補は `20-respond-away`、`10-respond-home` の reason_codes に `actor_unreachable`。
- `rr_ls_11_unknown_keeps_home`: 空集合なら `10-respond-home`。

### 3.2 到達性 → `SelectionInput` 変換(新規 `role_routing/availability.rs`)

```rust
/// AppState の観測を、policy 上の actor id 集合に写す。DB も I/O も触らない。
pub(crate) fn selection_input_for(
    policy: &RoleRoutingSettings,
    snapshot: &crate::providers::reachability::ReachabilitySnapshot,
) -> SelectionInput
```

規則: `snapshot.harness == Unreachable` のとき、`actor.transport == "provider" && actor.provider_id == Some(DYNAMIC_LAN_PROVIDER_ID)` を満たす actor の id を `unreachable_actor_ids` に入れる。それ以外(`Reachable`/`Unknown`)は空。`cloud_allowed` は `true` 固定(現状踏襲)。

`mod.rs` に `pub(crate) mod availability;` を追加。

### 3.3 受付経路の置換(`repository_turns/start.rs`)

`select_dispatch_candidate(connection, policy, now_ms)` に引数 `input: &SelectionInput` を追加し、内部の `candidates_for_action(policy, Respond)` を `candidates_for_action_with_input(policy, Respond, input)` に置換する。

呼び出し元:

- `record_provider_turn_start_in_transaction(...)`(本番): 引数に `selection_input: &SelectionInput` を追加。
- `record_provider_turn_start(...)`(テスト専用): **削除する**。`conversation_inputs_roles.d/02.rs` の 6 箇所は `_in_transaction` 版へ書き換える(`transaction` を作り、`origin="text"`, `source_id=None`, `presentation_mode="visual"` を渡す)。
- `repository.rs` / `repository_turns/mod.rs` の `pub(crate) use ... record_provider_turn_start;` を削除。

受付の `rr_decisions.reason_codes_json` は現状 `'["rules"]'` 固定。ここへ `selection_input` 由来の情報を残す: 選ばれた候補が `20-respond-away` で、かつ `10-respond-home` が `actor_unreachable` で除外されたなら `'["rules","location_fallback"]'` を書く。判定は `candidates` の除外理由を見るだけでよい。

### 3.4 `prepare_runtime_run`(`runtime/turns.d/02.rs`)

`record_provider_turn_start_in_transaction` 呼び出しの前で:

```rust
let selection_input = crate::role_routing::availability::selection_input_for(
    &policy_for_input, &state.reachability.snapshot());
```

`policy_for_input` は既に `load_role_routing_settings(&transaction)` で読んでいる値を使う(先頭の `role_routing_enabled` 判定で読んでいるので変数に保持する)。

**即時プローブ**: 受付の直前で `state.reachability_kick.notify_one()` を呼ぶ。ただし受付は同期関数なので結果を待たない。snapshot は最新観測値でよい(20 秒以内)。

### 3.5 ディスパッチ直前の最終確認(`runtime/conversation_inputs_roles.d/01.rs`)

`validate_actor_host` の `"provider"` 分岐で、`provider_id == DYNAMIC_LAN_PROVIDER_ID` かつ `state.reachability.snapshot().harness == Unreachable` なら:

```rust
return Err("Role-routing LAN provider is unreachable before dispatch".into());
```

`validate_actor_host` は `connection` しか受けていないので、`apply_enabled_role_route` と同じく `reachability: &ReachabilitySnapshot` を引数に追加する。呼び出し元(`conversation_inputs_roles.rs` 経由の 1 箇所と `d/02.rs` テスト)を追随させる。

このエラーは Phase 2 では **そのまま失敗として返す**(Phase 3 で `Resume` に置き換える)。エラー文字列は `turns.d/01.rs::public_failure_code` で `ConfigurationError` に落ちるので、`RuntimeFailureCode::ProviderUnavailable` 相当があればそれに割り当てる(無ければ追加しない。文字列だけ固有にして UI で見分ける)。

### 3.6 ipc/監視への露出

- `role_routing/ipc.d/01.rs` の snapshot に `selectedRecipeId` と `decisionReasonCodes`(`rr_decisions.reason_codes_json` をパース)を追加。既に `rr_decisions` を読んでいる箇所があるならそこへ 2 フィールド足すだけ。
- `src/lib/roleRoutingTypes.ts` と `src-tauri/src/ipc_contract/bindings.rs` に同名フィールドを追加。`tests/fixtures/ipc-receivers.json` を更新(`npm run` の契約テストが差分を検出する)。
- `src/features/audit/VoicePipelineMonitorPanel.tsx`(または RoleRouting 用パネルがあればそこ)に、`decisionReasonCodes` に `location_fallback` があれば「Cloud で応答中」バッジを表示。文言は `enCore.ts` / `jaCore.ts` に追加。

## 4. Phase 3(任意・後続): 進行中ターンの途中切替

Phase 2 の 3.5 で失敗にしている箇所を、同一 root の revision を進めて Cloud で再ディスパッチする形に置き換える。

1. `apply_enabled_role_route` で不達エラーを返す代わりに `Err(RoleRouteRetry::Unreachable)` 相当の列挙値を返す。
2. 呼び出し側(`conversation_provider_route.d/01.rs`)で受けたら、`sqlite_writer.write` 内で:
   - 現 revision の step を `settle_unfinished_steps(..., "cancelled")`。
   - `selection_input_for` を再評価し、`compile_recipe_by_id(policy, "20-respond-away")` で新 revision(現+1)の step 行を `planned` で INSERT(`start.rs` の step INSERT と同じ列)。
   - `coordinator::apply_in_transaction(tx, root_id, Event::InputBarrier)` → `Event::Resume`。Resume の `DispatchActor` 効果で新 revision の先頭 step が claim される(coordinator が旧 revision step を cancelled にし、`claim_next_planned_step` を呼ぶのは既存実装)。
   - `rr_events` に `root_resumed` が入るので `load_budget().automatic_switches` が 1 増える。`max_automatic_switches` 超過なら失敗にする。
3. ループへ戻り、再度 `permit_next_step` から進める。

この Phase は `Resume` の本番初接続になる。`revision.rs` の `#[cfg_attr(not(test), allow(dead_code))]` を外し、`resume_after_tools_settle` を使う。

## 5. 設定面(Phase 2 に含める)

- `src-tauri/src/persistence/settings_defaults.rs` の `routing.roles` 既定値には触らない(既定は `enabled: false`)。
- `src/features/settings/RoleRoutingSection.tsx` に「Cloud actor を追加して外出時フォールバックを構成」する導線は **今回は作らない**。手動で actors/roles/recipes を設定する前提。以下をドキュメントに残す:

```json
{
  "actors": [
    {"id":"larm","label":"LARM","transport":"provider","providerId":"dynamic-lan","location":"local","resourceGroup":"lan","maxInputBytes":32768,"capabilities":["reason"]},
    {"id":"cloud","label":"Cloud","transport":"provider","providerId":"<cloud provider id>","location":"cloud","resourceGroup":"cloud","maxInputBytes":32768,"capabilities":["reason"]}
  ],
  "roles": {"reasoner":"larm","advanced":"cloud"},
  "recipes": [
    {"id":"10-respond-home","action":"respond","roles":["reasoner"],"enabled":true},
    {"id":"20-respond-away","action":"respond","roles":["advanced"],"enabled":true}
  ]
}
```

`providerId` の実値は `DYNAMIC_LAN_PROVIDER_ID` 定数(`lib.d/01.rs`)を確認して合わせる。

## 6. テスト計画

| ID | 種別 | 場所 | 内容 |
| --- | --- | --- | --- |
| rr_ls_01〜03 | unit | `providers/reachability.rs` | ヒステリシス遷移 |
| rr_ls_04 | unit | `dynamic_lan/probe.rs` | `reachable_at` が 200→true、拒否→false、401→false |
| rr_ls_10〜11 | unit | `role_routing/selection.rs` | `actor_unreachable` 付与と候補順 |
| rr_ls_12 | unit | `role_routing/availability.rs` | Unreachable で dynamic-lan actor のみ集合に入る。Unknown は空 |
| rr_ls_20 | boundary | `runtime/conversation_inputs_roles.d/02.rs` | 受付時 Unreachable → `rr_decisions.selected_id == "20-respond-away"`, `reason_codes_json` に `location_fallback`, `rr_steps.actor_id == "cloud"` |
| rr_ls_21 | boundary | 同上 | 受付時 Reachable → `10-respond-home`, `actor_id == "larm"` |
| rr_ls_22 | boundary | 同上 | 受付 Reachable、dispatch 直前 Unreachable → `apply_enabled_role_route` が固有エラー文字列を返し provider I/O に入らない |
| rr_ls_23 | boundary | 同上 | 既存 6 テストが `_in_transaction` 版で同じ assert を通る |
| rr_ls_30 | contract | `tests/` (vitest) | ipc snapshot の新フィールドが `ipc-receivers.json` と一致 |

`rr_ls_20〜22` の fixture は `ReachabilityState` を直接 `record()` で操作する。監視タスクは起動しない。

## 7. 実施順序と粒度

1 コミット = 1 行。各行は単独で `cargo test -p saaa` と `npm test` が緑であること。

| # | 作業 | 主なファイル | 完了条件 |
| --- | --- | --- | --- |
| 1 | `ReachabilityState` + テスト | `providers/reachability.rs`, `providers/mod.rs` | rr_ls_01〜03 |
| 2 | `reachable()` プローブ + テスト | `dynamic_lan/probe.rs`, `urls.rs` | rr_ls_04 |
| 3 | `AppState` にフィールド追加、全構築箇所を更新 | `app_state.rs`, `lib.d/01.rs`, テスト fixture | ビルド緑 |
| 4 | 監視タスク + ネットワーク変化検知 + 起動登録 | `reachability_watcher.rs`, `lib.d/02.rs`, `Cargo.toml` | 起動してログにプローブ結果が出る |
| 5 | `SelectionInput.unreachable_actor_ids` + `actor_unreachable` | `selection.rs` | rr_ls_10〜11 |
| 6 | `availability::selection_input_for` | `role_routing/availability.rs`, `mod.rs` | rr_ls_12 |
| 7 | `record_provider_turn_start` 削除、テストを `_in_transaction` へ | `start.rs`, `repository.rs`, `repository_turns/mod.rs`, `d/02.rs` | rr_ls_23(既存 assert 維持) |
| 8 | `select_dispatch_candidate` に input を通し、`location_fallback` を記録 | `start.rs`, `turns.d/02.rs` | rr_ls_20〜21 |
| 9 | `validate_actor_host` の最終確認 | `conversation_inputs_roles.d/01.rs` | rr_ls_22 |
| 10 | ipc/TS 契約/UI バッジ/i18n | `ipc.d/01.rs`, `bindings.rs`, `roleRoutingTypes.ts`, `ipc-receivers.json`, パネル, `enCore.ts`, `jaCore.ts` | rr_ls_30、UI で表示確認 |
| 11 | 作業カード更新 | `saaa-role-routing-work-cards.md` | RR-06 の「hard filter」欄に本計画の到達性入力を追記 |
| (12) | Phase 3: Resume 接続 | `conversation_provider_route.d/01.rs`, `revision.rs` | 途中不達で revision+1 の Cloud step が claim される boundary test |

## 8. 注意点(実装者向け)

- `selection.rs` の候補順は `recipe_id` の文字列比較。recipe id に番号接頭辞を付けないと `respond-away` < `respond-home` となり Cloud が常に優先される。
- `validate_actor_host` は `location` が一致することも確認している。Cloud actor の `location: "cloud"` と provider 設定の `location` を一致させること。
- `apply_enabled_role_route` は `root_id` が `Some` のとき receipt の `selected_id` を使う。ここで再選択しないのは意図的。Phase 2 で到達性を反映するのは **受付時のみ**。
- `reachable()` は claim をしない。既存 `probe()` を流用すると allocation が消費され、LARM 側のリソースを占有する。
- `Unknown` は到達扱い(楽観)。起動直後に LARM が選ばれて失敗する可能性はあるが、Cloud に誤って課金するより安全側。
- `now_ms` は `turns.d/02.rs` 内で 2 回取っている。今回触るなら 1 回にまとめてよいが、必須ではない。
