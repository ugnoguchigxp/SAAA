# SAAA × L-Lang 生成・検査 統合 検証報告

実施日: 2026-09-20。対象計画: `spec/docs/saaa-llang-generation-inspection-plan.md`（v2）。
本報告は実装済みの範囲と未達を隠さず記録する。計画 9 章の形式に従う。

## 1. 判定

| 区分 | 結果 |
| --- | --- |
| 決定的コア（C01〜C06、C08、C11/C12 の純ロジック） | 実装・試験済み |
| C04 actor 伝播 | 実装済み。所有権記録は unit 試験済み、会話/MCP からの end-to-end は未検証 |
| C07 公開同期（両台帳・grant・epoch の 1 transaction） | **未実装** |
| C09 生成（fake/prompt/kit/build は実装、provider 実行は未接続） | 一部実装 |
| C10 GenerationService オーケストレーション | **未実装**（状態機械の部品のみ） |
| C11/C12 会話ターンへの接続 | **未接続**（parser/display の純ロジックのみ） |
| C13 会話/MCP 通し試験 | **未実装** |
| C14 実モデル live 実証 | **未実施**（kit と credential の接続前段） |
| **全体（G01〜G12）** | **未完了**。上記未達のため |

計画 11 章のとおり、未達を成功として報告しない。汎用 ABI・host API・WASI 等の後続項目は
今回の対象外である。

## 2. 開始/初回/修正後 snapshot

| 項目 | 値 |
| --- | --- |
| SAAA commit | `584d9b9fc9f7679382288321ba6455ceeabd5354` |
| DB schema version | 25 → 26（新規 3 table、既存 table は drop しない） |
| L-Lang commit | `a116435f2d890fae6ed16100bbdffb4ddd0ce1e2`（着手時 dirty=false） |
| L-Lang 作業ツリー | セッション後半に並行作業で docs と `src/llang-module-value-wasm.ts` が変更され dirty になった（本実装は L-Lang を変更していない）。live 用 kit は再生成後に digest を再固定する |
| Bun | `1.3.14` |
| 生成 kit digest（clean snapshot） | `7bd4d6415ab4214236ca0ac05710878c3ab8db18323327c8ccff5f5b05033c83` |

試験 snapshot（最終、`cargo test --lib` フィルタ）:

| コマンド | 初回 | 最終 |
| --- | --- | --- |
| `cargo test --lib generated_capabilities` | 78 passed | 111 passed / 0 failed |
| `cargo test --lib tool_selection` | （既存） | 183 passed / 0 failed |
| `cargo test --lib providers` | （既存） | 79 passed / 0 failed / 2 ignored |
| `cargo test --lib runtime::capability_commands` | — | 4 passed / 0 failed |
| `cargo fmt --check` | 成功 | 成功 |
| `cargo clippy --lib -- -D warnings` | — | 成功（新規モジュールの警告 0） |
| `bun run size:check` | — | **未完了**（下記 2.1） |

### 2.1 モジュールサイズゲート

新規モジュールの baseline を追加し、既存ファイルへの追記は新モジュールへ分離して ratchet 内に収めた
（`providers/openai_compatible/generation.rs`、`generated_capabilities/{retirement.rs,generation/schema.rs,inspection/schema.rs}`）。
`bun run size:check` の残る失敗は `src-tauri/src/lib.rs: 801 exceeds hard budget 800` のみで、
これは並行作業中の未追跡 `src-tauri/src/steward/**` が `lib.rs` に `mod steward;` と command 登録行を
追加したことによる。本実装は `lib.rs` を変更していない。並行作業が baseline を更新すれば、本実装の
モジュールは既に baseline 登録済みのため `size:check` は通る。上限緩和や ignored 化で通してはいない。

初回の `cargo test --lib generated_capabilities` は 78 passed で成功した。その後、作業ツリーの
並行作業モジュール（未追跡 `src-tauri/src/steward/**`）が一時的に test コンパイル不能となり、
`cargo test --lib` 全体が中断する時間帯があった。本実装は当該モジュールを変更していない。並行
作業の収束後に再実行し、上記の最終値を得た。

## 3. C カード別の根拠

### C01（T00）生成 kit

- `scripts/llang/build-generation-kit.ts` を追加。`packageLlangCapability`/`inspectLlangCapability`
  を呼ぶ固定 dispatch を Bun bundle し、manifest と digest を保存する。
- `--selftest` で「空白入りパスへ移動」「checkout なし」「file 改変で digest 不一致」「inspect の
  format と semanticEquivalence=not-checked」を実測。結果は
  `spec/evidence/llang-generation/dependencies.md` に記録。

### C02（T01）設定・契約

- `generation/config.rs`: `SAAA_LLANG_GENERATION_CONFIG`（64 KiB、未知 field 拒否、`enabled` 必須）、
  requests（1 MiB、最大 32 entry、file hash を読込時固定）。
- `generation/contracts.rs`: `GenerationStatus`/`GenerationErrorCode`（12.3 の固定値）、
  `GenerationContext`/`GenerateInput`/`GenerationReceipt`、厳密 JSON の `ModelResponse`/
  `SourceDocument`（camelCase・`deny_unknown_fields`）。
- 試験: 未知 field 拒否、Markdown fence 非救済、id/profile/field 順不一致、重複 id、field 不正。

### C03（T02）DDL・repository

- `generated_capabilities/schema.rs` に 12.4 の 3 table を追加（`generated_capability_generation_jobs`、
  `generated_capability_call_owners`、`generated_capability_inspections`）。時刻は unix ms。
- `generation/repository.rs`: CAS（`UPDATE ... WHERE id=? AND status=?`）、`(principal,run,message)`
  idempotency、同一 capability の同時 job 拒否、FK、call owner 記録。
- 試験: idempotency、CAS 再適用拒否、one-active-job、FK 拒否、owner 記録。

### C04（T02）actor 伝播

- `BackendRequest` に `origin`/`actor`、`InvokeRequest` に `actor: Option<CallActor>` を追加。
- `CapabilityService::invoke` は call 行と owners 行を同一 transaction で作る。owner の無い call は
  inspection で not-authorized。
- `tool_selection::invoke_with_origin` を追加し、gateway の conversation/MCP で origin を固定。
  M2A 直接経路（`generated_capabilities::tools::execute_with_actor`）も actor を渡す。
- 試験: owner の FK/記録（repository test）。会話/MCP からの end-to-end 所有権確認は未実装。

### C05（T03）inspection

- `inspection/contracts.rs`: `llang-capability-inspection` v1 の厳密型、hash 検証、`semanticEquivalence`
  の `not-checked` 固定、1 MiB 上限。
- `inspection/repository.rs`: `(revision_id, inspector_digest)` 一意、完全成功のみ記録。
- `inspection/service.rs`: owner・project 検査（他人は not-authorized）、revision 固定、managed package
  再検査、report の hash 照合と contract hash 照合、staging→atomic rename→DB 確定、同 revision/digest
  の証拠再利用。
- 試験: 公開と再利用、別 principal 拒否、project 不一致拒否、contract 不一致拒否、欠落成果物は
  再生成せず明示エラー、比較不一致で非公開。

### C06（T04）projection/Wasm 比較

- `inspection/comparison.rs`: 最大 8 boolean（256 ケース）の全列挙、不一致は必ず fail、評価器エラーも
  不一致、`checkedCases`/`mismatches`/`inputDomain`/`projectionHash`/`artifactHash`、
  `semanticEquivalence: not-checked` を保存。
- 試験: 列挙順 `00,10,01,11`、8 field=256、意図的不一致、評価器エラー、9 field 拒否。

### C08（T05）retire/復帰

- `repository::retire` と `lifecycle::retire_revision`、`CapabilityService::retire_revision` を追加。
  active の retire を拒否、suspend→retire、catalog epoch 整合、package/履歴は保持。
- 試験: active 拒否、suspend→retire、stale epoch 拒否。

### C09（T06）生成の部品

- `generation/kit.rs`: kit manifest の hash/digest 検証、env 消去・wall-clock 制限つき実行。
- `generation/generator.rs`: 固定 system prompt、request 本文のみを渡し独立 acceptance を渡さない
  prompt builder、decision 的な `FakeGenerator`（要求から body を組立て、事前 package を選ばない）、
  `ProviderGenerator`。
- `generation/builder.rs`: request/suite/metadata の workspace copy、metadata の id/release をホスト生成、
  固定 `package` CLI 実行、`KitInspector`。
- `providers/openai_compatible/structured.rs::complete_generation`: 生成専用 max output 4096 と外側
  deadline（既存抽出 700/5s は不変）。
- 試験: prompt に acceptance 正解表が入らない、fake A/B の body 差、kit hash/digest 検証。
- 未接続: `GenerationService` からの実呼出し、実 provider 実行、import/verify/publication 接続。

### C10（T07）状態機械・復旧

- `generation/recovery.rs`: 起動時に requested/generating/building/importing/verifying → interrupted、
  `awaiting_activation` は保持、inspection orphan ディレクトリのみ削除。
- 試験: interrupted 化と awaiting_activation 保持、orphan 削除。
- 未実装: 正常系の状態遷移オーケストレーション（requested→…→active）、予算・取消・conflict の統合、
  import/verify/activate 接続。

### C11/C12（T08）会話入口

- `runtime/capability_commands.rs`: 3 command の厳密 parse（trim のみ、改行・追加引数・引用内 command
  拒否、通常文は NotACommand）、inspection 表示（fence 長の自動選択、64 KiB 超は summary +
  artifact-too-large）。
- 試験: 完全一致のみ command、update の base revision 必須、fence escape、表示上限。
- 未接続: `runtime/turns.rs` の分岐（job 起動、assistant message 永続化）。

## 4. 生成 job と実行 ID の対応

- 12.4 のとおり新たな ID 対応表は作らない。`BackendRequest.call_id` をそのまま
  `InvokeRequest.call_id` へ渡し、`generated_capability_calls` と `generated_capability_call_owners`
  を同一 call_id で join する。inspection は owners から principal/project を検査する。
- owner 不明の既存 call は現在ユーザーへ帰属させず、inspection は not-authorized。

## 5. 実モデル名・usage・latency

未実施。C14 は実 provider credential と接続済み kit を要する。本実装は fake generator と prompt
builder の決定試験までで、live 実行は行っていない。model 名・usage・latency は記録しない。

## 6. 独立ケース

- 独立 acceptance は `src-tauri/tests/fixtures/llang-capability-v2/acceptance/`（`enabled-user` /
  `enabled-only` の真偽値表）を既存 M1 経路から利用する。本実装は acceptance の正解表を model
  prompt へ含めない試験のみ追加した。
- 計画 12.8 の固定 fixture A/B（`enabled && !suspended` / `enabled`）は `FakeGenerator` の
  `FakeBody` として用意し、body 差を試験した。live 用の新規要求と独立正解表は未作成。

## 7. 自己レビュー

計画 R1 の観点で確認した事項:

1. 生成開始の権限: `GenerationContext` はホストから渡し、モデル応答からは受けない（C02 の
   `deny_unknown_fields`）。
2. モデルが触れる field: `SourceDocument` の header は登録要求と比較し、body のみ自由。suite/
   metadata/path/command は未知 field として拒否（試験済み）。
3. 検証前公開の不存在: inspection は比較不一致・report 不一致で directory も DB 行も作らない
   （試験済み）。生成の import/verify 前公開は C07/C10 未実装のため該当経路自体がまだ無い。
4. DB transaction 境界: owner は call と同一 transaction。inspection は file publish 後に DB 確定、
   DB 失敗時は directory を破棄。
5. 過去版 inspection の所有者: owner の principal/project 一致を必須とし、grant 撤回では過去履歴の
   閲覧権を消さない設計（owners は grant と独立）。
6. 失敗時 process/DB/file 終端: kit は wall-clock 超過で kill。job の terminal は CAS。inspection
   staging は rename 前のみ。未確定 directory は起動時に削除。

## 8. 未達と次の作業

| 未達 | 内容 | 次の一手 |
| --- | --- | --- |
| C07 | 両台帳・grant・job・epoch を 1 transaction で更新する `publication_sync::activate_and_publish`、suspend/retire 時の tool 非公開化 | `tool_selection` catalog の既存関数を transaction 内用に抽出し、`guards` の最終条件を再利用 |
| C09 live / C10 | `GenerationService` の状態遷移、予算/取消/conflict、import/verify/publication 接続 | C07 の上に generation service を載せ、`CapabilityService::import_candidate`/`verify_candidate` を呼ぶ |
| C11 接続 | `turns.rs` の command 分岐と assistant message 永続化 | `runtime/mod.rs` に service を保持し、ユーザー入力永続化後・最初の provider 呼出し前に分岐 |
| C13 | A→実行→B→A 検査→停止→復帰の会話/MCP 通し試験 | C10/C11 完了後 |
| C14 | 実モデルでの新規/変更生成の実証 | 実 credential・kit 配置後に live lane として実施 |
| G01〜G12 | 上記未達のため未検証 | 各カード完了後 |

## 9. 提出物

- 実装: `src-tauri/src/generated_capabilities/generation/**`、`inspection/**`、
  `runtime/capability_commands.rs`、`scripts/llang/build-generation-kit.ts`。
- 変更: `generated_capabilities/{schema,repository,lifecycle,service,contracts,tools}.rs`、
  `tool_selection/{backends/mod,backends/llang,gateway,service}.rs`、
  `providers/openai_compatible/{.rs,structured.rs}`、`providers/stream/agent_dispatch.rs`。
- 依存記録: `spec/evidence/llang-generation/dependencies.md`。

## 10. コードレビュー指摘と修正

実装後の自己レビューで見つけた不具合を、失敗試験を先に追加してから修正した。

| 指摘 | 内容 | 修正 | 追加試験 |
| --- | --- | --- | --- |
| R-01 | kit 実行で stdout/stderr を `wait_with_output` まで読まず、子が pipe を埋めるとデッドロックし timeout 誤判定になっていた | 両 pipe を専用 thread で同時 drain し、上限超過を別判定 | `drain_keeps_at_most_the_limit_and_reports_overflow` |
| R-02 | `KitInspector` が `inspect --out-dir` の出力先を事前作成しており、CLI の「新規ディレクトリ必須」で必ず失敗していた | 親のみ作成し、leaf は CLI に作らせて読み後に削除 | `builder` 経路のレビュー |
| R-03 | コマンド parse が前方一致のため `/capabilityfoo` を malformed command として拾っていた | 語境界を `/capability ` に固定 | `parse` の追加 assert |
| R-04 | inspection が revision の contract hash と report の contract を照合していなかった | `contracts::contract_hash` で照合し不一致を integrity 拒否 | `a_contract_mismatch_with_the_revision_is_refused` |
| R-05 | inspection の INSERT conflict が not-authorized に誤写像されていた | `conflict` を `InspectionErrorCode::Conflict` に分離 | `error_codes_round_trip_including_the_inspection_extras` |
| R-06 | 登録要求の request 本文と fields の順序・名前を照合していなかった | load 時に request id と contract fields を照合 | `request_contract_mismatch_is_rejected` |
| R-07 | 登録ファイルの byte を job へコピーする際に hash を再確認していなかった | `stage_registered_files` が load 時 hash を再検証し、改変を拒否 | `staged_files_are_rechecked_against_the_load_time_hash` |
| R-08 | モデル出力 64 KiB 上限が未実装だった | `parse_model_response` で超過を budget 拒否 | contract 試験 |
| R-09 | budget-exceeded が cancel と同じ error に写像されていた | `budget-exceeded` を timeout に分離 | `error_to_capability_mapping_keeps_budget_and_cancel_distinct` |
| R-10 | 未使用の helper/field（`completed_job_ids`、`coverage_marker`、`inspection_error`、`supported_profile`、`GenerationReceipt::refused`、`INSPECT_DIR`）が残っていた | 削除、または意味のある検証に置換（kit `commands` 検証、`within_budget` 利用） | 既存試験 |
| R-11 | project 付き call の拒否試験がなかった（G10） | project 不一致拒否と一致時成功の試験を追加 | `a_project_scoped_call_requires_the_same_project` |
| R-12 | 表示上限判定が事前見積で、fence 分だけ 64 KiB を超え得た | 完成後の message 長で判定 | `oversized_typescript_is_summarised_not_truncated_silently` |

未修正のまま残る既知の改善点（C07/C10 の実装対象）: 生成 service からの cancellation
伝播、inspection の runtime digest 照合、同 revision/digest の single-flight mutex。
