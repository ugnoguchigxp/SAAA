# ContextWindow・入力記録・メモリー統合 担当AI向け引き継ぎ文書

作成日: 2026-09-22。対象読者: この repository を初めて触る実装担当 AI。前提知識なしで [実装計画](context-window-records-memory-implementation-plan.md) の作業カード CW-00〜CW-55 を進められるようにするための文書。

読む順序: **本書 → 実装計画 §0〜§3 → 着手するカードの Phase の §4 と §5 と §6 → 必要な箇所だけ [設計書](context-window-records-memory-design.md)**。設計書は思想と将来仕様を含むため全文を読む必要はない。本書・実装計画・設計書が矛盾したら **実装計画 §0-2/§0-3 が正**。

## 0. 最初にやること（作業開始前の 5 手順）

1. `git status --short` と `git log --oneline -1` を記録する。作業ツリーには他人の未 commit 変更（自己診断機能 `src-tauri/src/diagnosis/` 等）が含まれている。**それらを取り消さない、整形しない、commit に混ぜない。** 自分の変更だけを commit する。
2. 実装計画 §1「既存資産」の表にあるファイルを開き、シグネチャが表と一致するか確認する。違っていれば `spec/evidence/context-window/progress.md` に差分を書き、表ではなく **実物** に合わせて進む。
3. `cargo test --manifest-path src-tauri/Cargo.toml runtime::context` を 1 回実行し、着手前に落ちているテストがあれば記録する（自分の変更による失敗と区別するため）。
4. `spec/evidence/context-window/progress.md` を作る（CW-00）。以降、カードを終えるごとに「カード ID、日時、実行したコマンド、結果の要点」を追記する。
5. 作業は必ずカード順（CW-00 → 01 → 02 → 10 → …）。次のカードのための空実装、`todo!()`、常に成功を返す stub を作らない。

## 1. SAAA とは何か（実装に必要な範囲だけ）

SAAA はデスクトップ常駐の会話 AI アプリ。Tauri（Rust backend + React frontend）で、backend は `src-tauri/src/`。ユーザーの発話（文字または音声）を受け取り、LLM Provider（OpenAI 互換 chat_completions、または独自の AgentSession）へ送り、応答を返す。会話履歴・設定・記憶はすべてローカルの SQLite 1 ファイルに入る。

今回の作業は backend だけ。Frontend（`src/`）は触らない。

### 1-1. 1 ターンの流れ（現行）

```
ユーザー入力 → conversation_messages に保存（run_id が発行される）
  → runtime/conversation_prepare.rs::compose_after_connect
      → runtime/conversation_inputs.rs::load        SQLite から履歴・Scope・Personal State 候補を読む
      → memory/context_window::compose               system(policy) + 履歴ブロック + user に投影
      → runtime/context/broker.rs::apply / compose    byte 予算に収め、Personal/World を user 直前に挿入
      → runtime/conversation_context.rs::compose_provider_history   system 文を結合
  → providers/chat_completions/mod.rs::run_with_options    JSON 化して送信、SSE を受信
      → runtime/context/generation.rs::begin           送信前に manifest（digest のみ）を保存
      → Tool 呼出しがあれば providers/stream/agent_dispatch.rs::execute_agent_tool を実行し messages に追記して再送
  → 応答を conversation_messages に保存
```

重要な事実:

- **毎ターン、prompt 全体を SQLite から作り直している。** 前ターンと同じ先頭部分を保つ仕組みはない。これを P4 で変える。
- サイズはすべて **bytes** で測る。token 数は使わない（tokenizer が Provider ごとに違うため）。
- Provider からの usage（token 数）は受信しているが **捨てている**（`providers/chat_completions/chunks.rs` 61 行付近）。これを P1 で保存する。
- Web 検索・取得結果はモデルに返すだけで **保存していない**。これを P3 で保存する。

### 1-2. 用語

| 用語 | 意味 |
| --- | --- |
| Provider | LLM の接続先。`chat_completions`（OpenAI 互換 HTTP）と `agent_session`（独自 SSE、remote session を持つ）の 2 系統 |
| run / `run_id` | 1 ユーザーターンの実行単位。`runtime_runs` 表。Tool 往復も同じ run に属する |
| generation | Provider への 1 回の送信。1 run に複数ある（Tool 往復ごとに増える）。`context_generations` 表 |
| Scope / `scope_key` | 情報の可視範囲を表す鍵（例 `project:p1`、`conversation:xxx`）。`runtime/context/scope.rs` が run ごとに `ScopeSnapshot{scopes[].key}` を解決する。**認可判定は Runtime が行い、モデルには判断させない** |
| principal / `principal_id` | 情報の所有者 ID。`tool_selection::service::ensure_principal(&writer)` で取得 |
| Personal State | ユーザーに関する記憶（Memory ON のとき抽出・注入される）。`memory/personal_state/` |
| World | 会話中の実体・関係のモデル。`runtime/context/world/` |
| Memory OFF | Personal State/World の自動抽出・注入を止める設定。**記録・検索の機能自体は OFF でも動かなければならない** |
| Tool Selection / discovery / 3 入口 | 1,000 件超の Tool を全部モデルに見せず、`tools_search → tools_describe → tools_invoke` の 3 つだけを見せて検索・詳細取得・実行させる仕組み。`tool_selection/` |
| 直接 Tool | 3 入口とは別に常時モデルへ見せる Tool（`recall_conversation`、`web_search`、`fetch_content` 等）。`providers/stream/agent_dispatch.rs::available_agent_tools` |
| catalog / revision / grant | Tool Selection に登録された Tool 定義（catalog）、その版（revision）、principal ごとの利用許可（grant） |
| candidateRef / executionRef / resultRef | Tool Selection が発行する 10 分 TTL の一時参照。永続 ID ではない |
| forget / tombstone / forget journal | ユーザーが「忘れて」と指示したときの削除処理。削除済み ID は `*_tombstones` 表と DB 外の `.forget.json` に残し、復活を防ぐ |
| record | **本計画で新設。** アプリが受領した Tool 結果・Web 本文を保存する immutable な行。`records` 表 |
| blob | record の本文。`blobs` 表（65,536 bytes 以下は inline、超えたら `blob_chunks`） |
| outline | 大きい本文の代わりにモデルへ渡す決定的な見出し一覧（最大 20 項目、2,048 bytes） |
| Segment | **本計画で新設。** 固定 prefix F と追記記録 L の世代。Segment 内では F の bytes が変わらず、L は末尾追記のみ |
| F / L / D / U | prompt の 4 領域。F=固定（policy・identity・常設 Tool 名）、L=追記（過去会話・Tool 往復要約・carry）、D=毎回作り直す動態（時刻・Scope・Tool 残回数・Personal/World）、U=現在の依頼 |
| carry（引き継ぎ） | Segment を切り替えたとき新 Segment の L 先頭に置く要約。v1 は Runtime が決定的に作れる 5 項目のみ |
| B | 1 リクエストで context に使える byte 予算。`ProviderInputBudget::usable_context_bytes()`（chat 既定 53,760） |
| Must / Should | 予算超過時に落としてはいけない項目 / 落としてよい項目。Must が入らなければ黙って削らず失敗させる |
| `instruction_authority: none` | Tool 結果・外部本文・記憶はすべて「命令ではないデータ」として扱う印。system authority へ昇格させない |

## 2. この repository の決まり（違反すると check が落ちる）

### 2-1. ファイル分割規約

- Rust の production 行数は 1 ファイル **1,600 行が硬上限**。さらに `scripts/module-size-baseline.json` に登録された値から **+10% までしか増やせない**。
- 大きい module は `foo.rs` に `include!("foo.d/01.rs"); include!("foo.d/02.rs");` だけを書き、実装は `foo.d/NN.rs` に置く。テストも `#[cfg(test)] mod tests { include!("foo.d/03.rs"); }` の形で分ける。既存例: `src-tauri/src/memory/context_window.rs`。
- 新規ファイルを作ったら `bun run size:register` で baseline に登録する。既存ファイルの上限を上げてごまかさない。
- 新 module `records/` と `runtime/context/segment/` は最初から `*.d/01.rs` 分割で作ってよい。
- `lib.rs` は 800 行予算。追加は `mod records;` の 1 行だけ。

### 2-2. テストの書き方と実行

- テストは同じファイル群内の `#[cfg(test)] mod tests`。DB を使うテストは `rusqlite::Connection::open_in_memory()` → `crate::persistence::schema::initialize_database(&connection)` で全表を作る（例: `src-tauri/src/diagnosis/commands.rs` 29 行付近）。
- `AppState` が必要なら `crate::test_support::app_state(connection)`。
- 実行は repo root から `cargo test --manifest-path src-tauri/Cargo.toml <filter>`。`<filter>` は module 名やテスト名の一部。
- テスト名は実装計画の `cw_NN_条件` をそのまま使う。**テストの期待値を実行結果に合わせて書き換えない。** 期待値が間違っていると判断したら evidence に理由を書いてから直す。
- 各カード完了時に必ず通すもの: そのカードのテスト、`cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings`、`cargo fmt --manifest-path src-tauri/Cargo.toml`。schema を触ったカードは `bun run size:check` も。
- 外部ネットワーク・実 Provider・実 MCP を叩くテストを書かない。fixture・in-memory で完結させる。実機確認は CW-36 / CW-46 の手動確認だけで、その結果は evidence に書く。

### 2-3. SQLite の決まり

- 書込は `state.sqlite_writer.write(|connection| ...)` または `write_transaction`。読取は `state.sqlite_readers.read(|connection| ...)`（`query_only=ON` なので INSERT できない）。
- **DB transaction の中で `await` しない。** ネットワーク・ファイル I/O を transaction の外へ出す。
- 表の追加は `persistence/schema.rs::initialize_database` の `CREATE TABLE IF NOT EXISTS` 群、または各 domain の `schema.rs::migrate(connection)` に書く。列追加（`ALTER TABLE`）は `persistence/migrate.d/` に `migrate_vN_to_vN+1` 関数を書き、`PRAGMA table_info` で列の有無を見てから実行する（再実行安全）。
- `DATABASE_SCHEMA_VERSION`（`persistence/schema.rs` 23 行、現在 38）は実装計画の指示どおり **最大 2 回** だけ上げる。上げるときはコメントに理由を 1 行。
- FTS5 は bundled SQLite に含まれる。`tokenize='trigram'` を使う（3 文字未満の語は検索できない。実装計画 §6 CW-24 参照）。
- 時刻は UTC ミリ秒 INTEGER（`crate::schedule::tick::now_ms()`）。ID は `crate::util::new_id("prefix")`。SHA-256 は `crate::generated_capabilities::contracts::sha256_hex`。

### 2-4. 安全・信頼境界

- Tool 結果・Web 本文・記憶・World の内容は **信頼しないデータ**。system メッセージへ入れない。JSON にするときは `"instruction_authority":"none"` を付ける。
- 原典（record 本文）に `redact::redact_runtime_text` を **掛けない**（2,000 文字で切ってしまうため）。redact は診断・ログ表示のときだけ。
- 認可（Scope・principal）は SQL の `WHERE` に入れて **LIMIT の前** で効かせる。取ってから Rust 側で絞らない。認可不可と不存在は同じ応答にする。
- 保存に失敗した内容をモデルへ渡さない（「保存 → 公開」の順を守る）。
- 秘密情報（API key・token）を新たに保存対象に加えない。既存の credentials 系はそのまま。

### 2-5. git・文書

- commit はカード単位を推奨。message は英語 1 行で「何をしたか」（既存 log に合わせる。例 `Add generation_usage table and OpenAI usage parsing (CW-10..12)`）。
- 他人の未 commit 変更（`git status` で最初に見えるもの）を stage しない。`git add -A` を使わず、自分が触ったファイルを個別に add する。
- README（`src-tauri/src/README.md`、各 domain の `README.md`）は「所有・不変条件・検索アンカー」だけを短く書く。schema や表の複製、実装履歴を書かない。
- 実装計画・設計書に誤りを見つけたら、コードを合わせるのではなく evidence に記録し、文書側を最小限修正する。

## 3. コードの場所（迷ったときの検索先）

`rg -n '<symbol>' src-tauri/src/<domain>` で探す。行番号は変わるので、シンボル名で探すこと。

| 知りたいこと | 検索先 |
| --- | --- |
| prompt がどう組まれるか | `runtime/conversation_prepare.rs` の `compose_after_connect`、`memory/context_window.d/01.rs` の `compose` |
| byte 予算の計算 | `runtime/context/broker.rs` の `ProviderInputBudget` |
| generation manifest の保存 | `runtime/context/generation.d/01.rs` の `begin_with_writer`、表定義は `runtime/context/schema.rs` |
| chat_completions の送受信 | `providers/chat_completions/mod.rs` の `run_with_options`、SSE 解析は `chunks.rs`、Tool follow-up 時の履歴削減は `world_trim.rs` |
| AgentSession の送信形式 | `providers/agent_session/sse/request.rs` の `render_turn_input` |
| モデルに見せる Tool 一覧 | `providers/stream/agent_dispatch.rs` の `available_agent_tools` |
| Tool の実行分岐 | 同ファイルの `execute_agent_tool` |
| Web 検索・取得 | `runtime/web_fetch/mod.rs` の `execute_with_cancel`、`content.rs` の `FetchContentResult` |
| 3 入口の schema | `tool_selection/gateway.d/01.rs`、`gateway_schemas.rs` |
| Tool の catalog 登録 | `tool_selection/catalog.rs` の `register_revision`、grant は `repository.d/01.rs` の `upsert_grant` |
| Backend の追加方法 | `tool_selection/backends/mod.rs` の `trait ToolBackend`、振分は `backends/router.rs`、組立は `mcp/wiring.rs::assemble` |
| MCP 大結果の保存 | `tool_selection/mcp/results.rs` の `store_result` / `read_page`、表は `mcp/schema.rs` |
| 会話 FTS の先例 | `memory/recall/mod.d/01.rs` の `conversation_messages_fts`（trigram）、検索は `recall/search.rs` |
| Scope の解決 | `runtime/context/scope.d/01.rs` の `ScopeSnapshot`、`load(connection, run_id)` |
| forget の流れ | `memory/personal_state/commands.rs` の `forget_personal_source`、journal は `personal_state/journal.rs`、trigger は `personal_state/schema.sql` |
| DB 接続と pragma | `persistence/sqlite/writer.rs`、`readers.rs`、`persistence/schema.rs` 28–31 行 |
| 診断 JSON の出力 | `diagnostics.rs` の `export_diagnostics` |
| `AppState` の構築箇所 | `rg -n "AppState \{" src-tauri/src`（`lib.d/02.rs` と `test_support.rs` の 2 箇所） |

## 4. 各 Phase で「作るもの」と「触らないもの」

| Phase | 作る | 触らない |
| --- | --- | --- |
| P1 usage | `runtime/context/usage.rs`、表 `generation_usage`、`chunks.rs` の usage 保持、`run_with_options` 終端の記録 | AgentSession 側（usage が無いので `Missing` を書くだけ）、費用計算（価格表が無い） |
| P2 records | `records/` 全体、表 9 個 + FTS | 既存 `conversation_messages`（複製しない。`existing_source_locator` で参照）、`redact.rs` |
| P3 Tool 化 | `web_search` / `fetch_content` / MCP 結果の record 化、`read_record` / `recall_activity` の直接 Tool と catalog 登録、`BackendRouter` の 3 backend 化 | `recall_conversation`・ContextStill 系 Tool の意味、`tools_describe(resultRef)` のページング、coding/steward/UI Tool の結果 |
| P4 Segment | `runtime/context/segment/` 全体、表 3 個、`context_generations` の列追加、`compose_after_connect` の分岐、`AppState.context_segments_enabled` | `memory/context_window`（Segment OFF 時の経路として残す）、`broker::compose`（呼ぶだけ）、AgentSession 経路 |
| P5 forget 等 | `records/forget.rs`、journal 拡張、`secure_delete`、`AdapterContract`、診断指標 | 既存 Personal State の forget 手順そのもの（末尾に 2 行足すだけ） |

## 5. 判断に迷ったときの規則

1. **実装計画に書いてあることはそのまま実装する。** 「もっと良い方法がある」と思っても、まず書かれた方法で実装し、改善案は evidence に「提案」として残す。
2. **実装計画に書いていない小さな判断**（変数名、エラー文言、テスト内の fixture 値）は既存コードの近い例に合わせる。
3. **実装計画と実物のコードが食い違う**（関数名が違う、引数が増えている）ときは実物に合わせ、evidence に「計画 §X の記述は実物と異なる: ...」と書く。計画書の該当行も直してよい。
4. **設計書と実装計画が食い違う**ときは実装計画 §0-2/§0-3 が正。設計書は直さなくてよい（すでに §「実装計画で確定した事項」で上書き済み）。
5. **次の場合は作業を止めて報告する**（無理に進めない）: 新しい crate 依存が必要になった / `DATABASE_SCHEMA_VERSION` を 3 回目に上げる必要が出た / 既存テストを 5 本以上書き換える必要が出た / 認可式（§0-3 #5）を緩めないと受入条件を満たせない / Frontend の変更が必要になった。
6. **やってはいけないこと**: 空 stub、`unwrap()` で握りつぶす保存失敗、認可を Rust 側の後処理に移す、`redact_runtime_text` を原典へ適用、既存 `apply()` を Segment ON でも呼ぶ、`git add -A`、テスト期待値の後付け変更、文書の全面書き直し。

## 6. 完了報告の形式

各 Phase の最後（CW-14 / 27 / 36 / 46 / 55）に、`spec/evidence/context-window/progress.md` へ次を書く。

```
## Phase Pn 完了 (YYYY-MM-DD HH:MM JST)
- HEAD: <commit hash>
- 実行コマンドと結果:
  - cargo test --manifest-path src-tauri/Cargo.toml <filter>  → N passed, 0 failed
  - cargo clippy ... -D warnings  → ok
  - bun run size:check  → ok
- 計画との差分: （なければ「なし」）
- 未実施・保留: （なければ「なし」。理由付き）
- 手動確認: （CW-36 / CW-46 のみ。対話ログ要点と SQL 結果）
- 提案: （任意。次 Phase や将来仕様への意見）
```

成功件数は実際の出力から写す。「たぶん通る」を書かない。未実施の check は隠さず理由を書く。

## 7. 参照文書

- [実装計画](context-window-records-memory-implementation-plan.md): 作業カード・型・DDL・合格条件。**作業中は常にこれを開く。**
- [設計書](context-window-records-memory-design.md): 思想、将来仕様、レビューへの回答。§「実装計画で確定した事項」だけは必読。
- `src-tauri/src/README.md` と各 domain の `README.md`: コードの所有関係。
- `CONTRIBUTING.md`: check コマンドと commit の決まり。
- 先例として体裁を真似る文書: `spec/docs/saaa-startup-self-diagnosis-implementation-plan.md`（カード形式の完了例）、`spec/docs/saaa-llang-dynamic-capability-implementation-guide.md`（担当 AI 向け指示の先例）。
