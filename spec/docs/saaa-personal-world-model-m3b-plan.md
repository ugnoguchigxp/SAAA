# Personal World Model M3B 実装計画 — 通常 generation への限定投入

作成日: 2026-09-20。状態: Step 2 の実装計画。コードは本計画の作成では変更しない。

前提は M3A 完了記録 `spec/evidence/world-model/m3-results.md`（文書内の引き渡し節）。上位は [M3A計画](saaa-personal-world-model-m3-plan.md) と [執事循環フェーズ](saaa-steward-loop-phase-plan.md) §6。カードは本書 §8（10枚、M3B-00〜09）。

## 1. 次に完成させるもの

Memory が有効な `conversation.respond` だけ、WorldFrame を **一つの非命令 Candidate** として実 Broker へ渡す。対象は次の三つに限る。グラフ探索はしない。

1. 現在の会議（scope 上の `resource` + MeetingSession RuntimeRef）
2. 進行中 Task（scope 上の `task` + CodingJob RuntimeRef。既存 coding snapshot）
3. 明示 Project（resolved な focus/parent の **一つ**）

`source_kind` は本番用 `world-model`。M3A の `world-model-shadow` は拒否のまま残す。Frame TTL 1,000 ms は延ばさない。Expired を Current 扱いにしない。

この段階の利用者は開発環境（`SAAA_MEMORY_ENABLED=1`）。通常インストールの default は OFF。live Provider の回答品質は完了条件にしない。

## 2. M3A から固定する判断

完了記録 `spec/evidence/world-model/m3-results.md` の引き渡し 5 項への答え。カード内で再議論しない。

| # | 判断 |
| --- | --- |
| 1 鮮度と生成中検証 | **別型**。`prepare`/`revalidate` の Frame TTL（≤1,000 ms）は dispatch 直前の鮮度に使う。生成中は `WorldReceipt`（process 内、Prepared を保持）で再検証する。M2A の `Expired` を無視して投入しない。TTL 延長 0。 |
| 2 境界 | **投入する**: `turns.rs` の compose と `chat_completions` の `generation_inputs::record`（初回 reasoning と tool-followup。followup 前に receipt 再検証）。**投入しない**: `agent_session`、音声 TTS、coding/Codex Provider 本文。World から Tool 権限を作らない。保存は既存の会話 persist。World 失効で本文を書き換えない。 |
| 3 receipt | `GenerationHandle` が任意で `PreparedWorldFrame` を保持する。SQLite に Frame 本文を保存しない。`context_generation_inputs` には既存どおり identity（kind/id/version/digest）だけ。再起動後に旧 Prepared を復元して再利用しない。Drop は既存の interrupted。 |
| 4 途中失効 | 再生成 **0 回**。追加 Provider 費用 0。dispatch 後の Expired/Changed は turn を失敗させず、generation 完了を通し、receipt を `expired-after-dispatch` / `changed-after-dispatch` として記録する（既存 `complete()` の scope/policy 失敗とは別）。ユーザー向け中断 UI は作らない。Activity 1 行は可。 |
| 5 要求の供給元 | LLM とユーザー本文から Project / seed / authorized を採らない。`scope::load` 済みの snapshot だけ。Project 0 件または 2 件以上は World 省略。RuntimeRef は meeting と coding の current/focus だけ、最大 8。`graph_request` は常に `None`。ExactName・自由文抽出は禁止のまま。 |

M3A の p95 ゲートは満た済み。本計画はそれを緩めない。

## 3. 実装調査で分かった接続点

- `conversation_inputs::load` は `sqlite_readers.read` の内側で Personal State 候補を作る。World の `prepare_frame` は Readers を使う。**同じロックの入れ子は deadlock**（M3A で確認済み）。World 取得は `load` の **外** で行う。
- `turns.rs` は `personal_candidates` を Broker に渡している。World はここに高々 1 件追加する。would_displace なら追加しない。本番では shadow の「比較用二系統」は走らせない。World 付き compose のあと、既存 selected の identity が欠けたら World なしで再 compose 1 回まで。合計 compose 最大 2。
- `generation_inputs::record` は先頭で `world-model-shadow` を拒否する。`world-model` は通す。shadow 拒否は削除しない。
- `GenerationHandle::complete` の `validate_dependencies` は scope / policy / personal-state を見る。World 本文は見ない。生成中検証は Handle 上の Prepared に対する `revalidate_frame` とする。
- `WorldFrameService::new` は `AppState.sqlite_readers` と `AppState.meeting` と `personal_state::now` で足りる。新しい Reader 実装は作らない。
- `m3_18` は `turns.rs` に `prepare_candidate` が無いことを文字列検査している。M3B で配線したら、断言を本番 kind / shadow kind の区別に更新する。shadow 文字列と `run_shadow` が turns に無いことは残す。

## 4. 範囲

含める:

- `world-model` Candidate の組み立て（既存 `prepare_candidate` / `render_world_frame` を再利用。kind だけ本番に差し替える入口を 1 関数にする）。
- Memory OFF なら取得 0、compose 追加 0。
- dispatch 直前の `revalidate` + TTL。失敗は省略。turn は World なしで継続。
- Handle 上の receipt と complete 時の再検証。
- 実DB試験。試験名は `m3b_NN_具体条件`。

含めない:

- 新 IPC、新環境変数、新 schema version、第 2 Writer。
- Frame TTL 延長、グラフ seeds、五要素の新規推定。
- `world-model-shadow` の製品有効化。
- agent_session / 音声 / Codex 本文への World 注入。
- live LLM の正答率。固定日本語 corpus の **envelope 検査** は含める。モデル出力の採点は M4。
- 途中失効の自動再生成と中断 UI。

## 5. 固定契約

### B0. 依存方向

`runtime/context/world` が `WorldFrameService` を呼ぶ。core は Broker / AppState / Provider に依存しない。通常会話の入口は `conversation_inputs` の外、`turns.rs` の compose 直前。`run_shadow` は評価用のまま残し、turns から呼ばない。

### B1. 有効化

`memory::control_plane::memory_enabled()` が false なら World 経路に入らない。新しい flag 名は作らない。Personal State 候補が空でも、Memory ON かつ明示 Project と RuntimeRef があれば World だけを試みてよい。

### B2. 要求

trusted caller が `ScopeSnapshot` から次を埋める。

- `project_scope`: kind=`project` かつ relation が `focus` または `parent` の key。件数 1 のみ。0 は `no_explicit_project`、2 以上は `ambiguous_project`。文字列分割をしない。
- `runtime_refs`: kind が meeting の `resource:` と coding の `task:` で、relation が `current` または `focus`。最大 8。それ以外の scope は載せない。
- `graph_request`: 常に `None`。
- `ttl_ms`: 既存上限（1,000）。`max_bytes`: 8,192。

空の RuntimeRef でも Project があれば prepare してよい。両方空かつ graph なしなら既存どおり `empty_request`。

AccessRequest は既存 Personal State と同じ principal / policy_revision / authorized 経路。DB に principal があるだけで authorized=true を捏造しない。

### B3. Candidate

一 Frame につき一 Candidate。field は M3A S3 と同じ（May / UntrustedData / Base / utility 0）。`source_kind` だけ `world-model`。`source_id` は `world-frame:<run_id>:<hex>`。wrapper は既存 WORLD_MODEL。Broker の PERSONAL_STATE 内側に入ることを許容する。

### B4. 本番 compose

1. World を付けた compose のあと、M3A と同じ identity 比較で既存 selected が欠けたら World を外してもう一度 compose する（would_displace）。
2. compose 最大 2、prepare 最大 1、revalidate は dispatch 前 1 + followup ごと 1 + complete 1。自動再 prepare 0。
3. current instruction count は 1 のまま。World は system/user を増やさない。

### B5. Dispatch 鮮度

`record` の前（set_health より前、かつ shadow 拒否の後）に、選択された `world-model` があれば同一 service で `revalidate_frame`。Current 以外はその Candidate を selected から外し、World を除いた候補で compose し直す（上記の 2 回目枠）。枠を超える再取得はしない。送れないなら World なしで record する。

### B6. 生成中 receipt

型は Serialize しない。Handle が所有する。

```text
WorldReceipt { prepared: PreparedWorldFrame, dispatched_at_ms }
```

- tool-followup の `record` 前: receipt があれば再検証。非 Current なら今回の followup に World を載せない。Prepared を捨てる。
- `complete` / `fail` / `cancel`: receipt があれば再検証する。結果を固定 enum の観測（Activity 可）に残してよい。Frame JSON は残さない。
- dispatch 後 Expired で `complete()` 自体は成功させてよい。scope/policy の stale は従来どおりエラー。

### B7. 漏洩と品質（offline）

Scope 漏洩 0: Candidate.scope_refs は allowed_scope_keys の部分集合。Broker の any-scope 通過を認可の代替にしない。

誤断定 0（本 Step の意味）: empty/expired/omitted を「知識なし」とユーザー向け文面へ変換しない。モデルが何を言うかは測らない。

命令位置 1: envelope の `current_instruction_count == 1`。

固定日本語は fixture の会議名 / 作業名 / Project を Frame JSON に載せ、envelope から parse して一致させる。live の「今会議中ですか」は evidence に live未検証 と書く。

## 6. 検証

```sh
cargo test --locked --manifest-path src-tauri/Cargo.toml --lib m3b_
cargo test --locked --manifest-path src-tauri/Cargo.toml --lib m3_
cargo test --locked --manifest-path src-tauri/Cargo.toml --lib runtime::context
bun run size:check
bun run spec:check
bun run check:local
```

対象 0 件を合格にしない。M3A 試験を壊さない。`m3_18` の turns 非配線検査は B0 に合わせて更新する。性能の新ゲートは設けない。投入経路の prepare が M3A 全経路 p95（350 ms）を超える実装は採用しない。

## 7. 完了記録

`spec/evidence/world-model/m3b-progress.md` と `m3b-results.md`。各カードの試験名・件数、Memory ON/OFF、compose 回数、receipt の扱い、未接続、live未検証を分ける。本文・会議名をログに残さない。

完了条件は 10 カードと §5、全体ゲート、通常会話 default OFF の証拠。M3C・M4 は準備が整っていないので見送り中（禁止ではない）。TTS・最小循環は本フェーズの後続 Step。

## 8. 作業カード（10枚）

各カードの前提は直前カードと指定 B 節。実装 1〜3 ファイル＋試験 1〜2 を標準とする。5 実装ファイルを超えるなら枝番へ分割する。commit は必須にしない。

| ID | 対象 | 契約 | 実装すること | 合格条件 |
| --- | --- | --- | --- | --- |
| M3B-00 | spec/evidence/world-model/m3b-progress.md | §3/6 | HEAD、dirty、M3A 結果、`m3_` 再実行可否を記録 | 並行差分を巻き戻さない。0 件実行を合格にしない |
| M3B-01 | context/world/source.rs | B1/B2/B3 | scope から要求を組む入口。kind=`world-model`。graph なし。Memory OFF で early return | Project 0/2 件省略。meeting/coding ref のみ。ExactName 経路を開かない |
| M3B-02 | turns.rs compose（load の外） | B0/B4 | lock 外で prepare。Personal に高々 1 World を足す。would_displace なら外して再 compose | deadlock 0。compose≤2。prepare≤1。instruction 1 |
| M3B-03 | generation_inputs.rs | B5 | shadow 拒否は残す。`world-model` は通す。selected の World は record 直前に revalidate | shadow は planned のまま。非 Current は送らない |
| M3B-04 | generation.rs Handle | B6 | receipt を Handle に載せる。complete 時 revalidate。本文を DB に書かない | 再起動再利用 0。dispatch 後 Expired で complete 成功可 |
| M3B-05 | chat_completions generation.rs | B0/B5/B6 | reasoning と tool-followup の record 前に receipt を見る。agent_session / voice は未配線 | 試験で非配線。followup に stale World 0 |
| M3B-06 | world 試験 | B2/B7 | 実DB: Memory OFF、会議のみ、coding のみ、Project のみ、期限切れ省略 | `m3b_06_*`。他 Project を envelope に出さない |
| M3B-07 | world 試験 | B4/B7 | would_displace、yellow 保持、日本語 fixture の JSON 一致、`m3_18` 更新 | 省略を知識なしへ変換しない。命令 1。turns に shadow 文字列が無い |
| M3B-08 | 関連試験・size | §6 | `m3b_` と `m3_` と context suite、size 登録、閾値未緩和 | 0 件成功扱い禁止 |
| M3B-09 | spec/evidence/world-model/m3b-results.md | §7 | カード証拠、未接続、live未検証 | 品質完了扱いにしない。TTS/循環に進める判断を一行 |

段階ゲート:

- M3B-02: Memory ON の compose に World が載る／載らないを区別できる。
- M3B-05: Provider 本文へ行く経路の鮮度と非配線が固定。
- M3B-09: 記録完了。live 回答は未検証。

指示例: 「M3B-02 だけを実装してください。`conversation_inputs::load` の read ロック内で `prepare_frame` しないでください。World が既存 selected を落とすなら外して compose し直し、3 回目の compose はしないでください。」

## 9. 本計画で足さないもの

思いつきは `spec/evidence/steward-loop/deferred.md` へ一行。

- live「今会議中ですか」の正答ゲート（M4）
- 生成中 Expired の自動再 generate
- World 専用 Runtime flag
- agent_session への横展開
- グラフ seeds の復活
