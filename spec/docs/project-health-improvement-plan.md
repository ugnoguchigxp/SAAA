# プロジェクト健全性 改善計画書

作成日: 2026-09-23
対象: SAAA リポジトリ全体
対象外: 認証情報の保存方式（開発効率を優先し、SQLite 平文保存を意図的に採用しているため変更しない）

## 0. 全タスク共通のルール（必ず守る）

1. **1 PR = 1 タスクの 1 単位。** 下の各タスクの「作業単位」ごとにコミットを分ける。複数単位をまとめない。
2. **振る舞いを変えない。** タスク1〜3はリファクタリングのみ。SQL、公開 API、IPC コマンド名、シリアライズ形式を変えない。
3. **各コミット前に必ず通すコマンド:**
   ```sh
   cargo fmt --check --manifest-path src-tauri/Cargo.toml
   cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
   cargo test --manifest-path src-tauri/Cargo.toml
   bun run size:check
   ```
   フロントエンドを触った場合は追加で `bun run typecheck && bun run lint && bun run test:frontend`。
4. 保存済みのユーザー設定・DB を消さない。DB を使う確認は一時ディレクトリのコピーで行う。
6. 迷ったら実装を止め、判断が必要な点を PR 説明に書く。推測で仕様を変えない。

---

## タスク1: `include!` による連番分割（`*.d/NN.rs`）の解消

### 背景
`src-tauri/src` 配下に `*.d/` ディレクトリが 64 個あり、65 箇所で `include!("xxx.d/01.rs")` が使われている。これはモジュールサイズ上限（`scripts/module-size.ts` の `rustProduction: 1600`）を満たすための機械的な分割で、次の問題がある。
- 分割境界に意味がなく、読む人がどこに何があるか分からない。
- `include!` は同じモジュールにテキスト展開されるため、可視性（`pub(crate)` など）による境界が効かない。
- rust-analyzer のジャンプや診断が不安定になる。

### ゴール
すべての `*.d/` を、意味のある名前のサブモジュール（`mod foo;`）に置き換え、`include!("*.d/` を 0 件にする。

### 手順（1 ディレクトリずつ）
1. 対象を列挙する（大きい順）:
   ```sh
   find src-tauri/src -type d -name '*.d' | while read d; do echo "$(cat $d/* | wc -l) $d"; done | sort -rn
   ```
2. 対象の親ファイル（例: `persistence/migrate.rs`）を開き、`include!` の位置を確認する。`mod tests { include!(...) }` のようにブロック内で展開しているものは、テスト用なので手順5へ。
3. `*.d/NN.rs` の中身を読み、**責務ごと**にまとめる。名前の例:
   - `persistence/migrate.d/*` → `persistence/migrate/{versions.rs, apply.rs, legacy.rs}`
   - `providers/dynamic_lan/mod.d/*` → `providers/dynamic_lan/{discovery.rs, client.rs, config.rs}`
   ファイル名は中の主要な型・関数から決める。`part1.rs`、`01.rs`、`misc.rs`、`helpers.rs` は禁止。
4. 親ファイルを `mod versions; mod apply;` 形式に変える。
   - 親が `foo.rs` の場合は `foo/versions.rs` に置く（Rust 2018 形式。`foo/mod.rs` へは変えない）。
   - 親が `mod.rs` の場合は同じディレクトリに置く。
   - 子モジュールから参照される項目は、必要最小限だけ `pub(super)` にする。`pub(crate)` や `pub` に広げない。
   - 以前は同一スコープで見えていた `use` が子モジュールで不足するので、各子ファイルの先頭に必要な `use super::*;` ではなく**個別の `use`** を書く。どうしても量が多い場合のみ `use super::*;` を許可する。
5. テスト用 `tests.d/` の場合は、`#[cfg(test)] mod tests;` とし、`tests/` 配下にテーマ別ファイル（例: `tests/migration_v12.rs`）として置く。
6. `.d` ディレクトリを削除する。
7. 共通コマンドを通す。`bun run size:check` で 1 ファイルが上限を超える場合は、さらに責務で分ける（再び連番にしない）。
8. `bun run size:register` は**新しく作ったファイルの登録**にのみ使い、既存の基準値を上げる目的で使わない。

### 作業順（大きい・影響が小さい順）
1. テスト系: `src-tauri/src/tests.d`, `tool_selection/mcp/tests.d`, `tool_selection/tests/mod.d`, `memory/personal_state/world/v2_tests.d`, `tool_selection/mcp_server/tests.d`, `memory/personal_state/world/tests.d`, `steward/tests.d`, `role_routing/repository_turns/tests.d`
2. 本体系: `providers/dynamic_lan/mod.d`, `persistence/migrate.d`, `memory/recall/mod.d`, `tool_selection/service.d`, `steward/repository.d`, `adaptive_improvement.d`, 以降は手順1の一覧順
3. 最後に `lib.d`（アプリ起動処理のため最も慎重に。`run()` 本体は `lib.rs` に残し、プラグイン登録・コマンド登録・状態初期化を `app_setup/{plugins.rs, commands.rs, state.rs}` に分ける）

### 再発防止
`scripts/module-size.ts` の `check` に、`include!(` で `.d/` を参照する行を検出したらエラーにする処理を追加する。テストを `tests/module-size.test.ts`（なければ新規）に追加する。

### 完了条件
- `rg 'include!\("[^"]*\.d/' src-tauri/src` の結果が 0 件
- 共通コマンドがすべて成功

---

## タスク2: 本番コードの `unwrap()` 削減

### 背景
テスト用ファイルを除いても、本番寄りのファイルに `unwrap()` が約 1,000 箇所ある（ファイル末尾の `#[cfg(test)] mod tests` 内を含む数字なので、実数はこれより少ない）。多いファイル: `adaptive_improvement.d/03.rs`(67), `runtime/context/generation.d/02.rs`(42), `voice/http_audio/mod.rs`(35), `generated_capabilities/generation/repository.rs`(27), `persistence/settings_migration.d/02.rs`(24), `persistence/remove_legacy_provider.rs`(23), `runtime/context/schema.rs`(21)。

### ゴール
本番コード（`#[cfg(test)]` 外）で、パニックすると常駐アプリが落ちる `unwrap()` を 0 にする。

### 手順
1. **タスク1の完了後に着手する**（ファイル構成が変わるため）。
2. 本番コードだけを数えるスクリプト `scripts/unwrap-audit.ts` を作る。
   - `scripts/module-size.ts` の `productionLines()` を再利用し、末尾の `#[cfg(test)] mod tests {}` を除いた範囲で `.unwrap()` と `.expect(` を数える。
   - パスに `test` を含むファイル、`tests/` 配下は除外。
   - 出力: `path<TAB>unwrap数<TAB>expect数`、多い順。
   - `package.json` に `"unwrap:audit": "bun scripts/unwrap-audit.ts"` を追加。
3. 1 ファイルずつ、各 `unwrap()` を次の基準で置き換える。
   | 状況 | 置き換え |
   | --- | --- |
   | 関数が `Result` を返す | `?`（必要なら `.map_err(...)` で既存のエラー型に変換） |
   | `Mutex::lock().unwrap()` | そのファイル・近隣で使われている方式に合わせる。例: `.lock().map_err(\|_\| "... lock unavailable".to_string())?`。近隣に方式がなければ `.unwrap_or_else(\|e\| e.into_inner())`（ポイズン回復） |
   | 定数文字列の正規表現・固定 JSON のパース | `LazyLock`/`OnceLock` に移し、`.expect("理由")` にする（起動時にしか失敗しない不変条件であることを明記） |
   | 直前で `is_some()`/長さ確認済み | `if let` / `let else` に書き換えて確認と取り出しを 1 回にまとめる |
   | 返り値が `()` で失敗を無視してよい処理（ログ・イベント送信） | `if let Err(e) = ... { log::warn!(...) }`。そのファイルで使われているログ方式に合わせる |
4. エラー文言は既存のスタイル（英語、先頭大文字、秘密情報を含めない）に合わせる。
5. 1 コミットで扱うのは 1〜3 ファイル。

### 再発防止
全ファイルの対応が終わった後、`src-tauri/Cargo.toml` に次を追加し、テストでは許可する。
```toml
[lints.clippy]
unwrap_used = "deny"
```
テストモジュール側には `#![allow(clippy::unwrap_used)]` を付ける（`src-tauri/src/tests.rs` など、テストのルートごと）。`clippy.toml` に `allow-unwrap-in-tests = true` を置けば個別の allow は不要なので、そちらを優先する。

### 完了条件
- `bun run unwrap:audit` の unwrap 列がすべて 0
- clippy の `unwrap_used = "deny"` を有効にして共通コマンドが成功

---

## タスク3: `unsafe` ブロックへの安全性コメント付与

### 背景
`unsafe` は 26 箇所あり、`// SAFETY:` コメントがあるのは 8 箇所だけ。主な場所: `situation/platform/macos.rs`, `voice/speaker.rs`, `runtime/pi/process.rs`, `coding/recovery.rs`, `persistence/sqlite/owner.rs`, `providers/dynamic_lan/credential.rs`, `generated_capabilities/generation/kit.rs`, `memory/context_still_*`。

### 手順
1. `rg -n "unsafe \{|unsafe fn|unsafe impl" src-tauri/src crates` で列挙する。
2. 各 `unsafe` の直前に `// SAFETY:` コメントを書く。内容は「呼び出し先の前提条件」と「それがここで満たされる理由」の 2 点。例:
   ```rust
   // SAFETY: `pid` は直前の fork で得た子プロセスで、まだ wait していないため有効。
   let result = unsafe { libc::kill(pid, libc::SIGTERM) };
   ```
3. 前提条件が満たされる理由が説明できない箇所は、コードを変えずに PR 説明の「要確認」欄に列挙する。
4. `unsafe fn` には `/// # Safety` ドキュメント節を付ける。
5. 最後に `src-tauri/Cargo.toml` の `[lints.clippy]` に `undocumented_unsafe_blocks = "deny"` を追加する。

### 完了条件
- clippy の `undocumented_unsafe_blocks` を有効にして共通コマンドが成功

---

## タスク4: Rust 側カバレッジの計測と可視化

### 背景
`coverage/frontend/lcov.info` はあるが、Rust 側の網羅率は計測していない。

### 手順
1. `cargo llvm-cov` を使う。未インストールなら `cargo install cargo-llvm-cov` と `rustup component add llvm-tools-preview`（`rust-toolchain.toml` の `components` にも追記）。
2. `scripts/test-coverage.ts` を読み、既存のフロントエンド計測に Rust 計測を追加する。
   ```sh
   cargo llvm-cov --manifest-path src-tauri/Cargo.toml --lcov --output-path coverage/rust/lcov.info
   ```
3. モジュール（`src-tauri/src` 直下のディレクトリ）ごとの行カバレッジを集計し、`coverage/rust/summary.md` に表で出力する。
4. `coverage/` が `.gitignore` に入っているか確認し、入っていなければ追加する。
5. 閾値による失敗はまだ入れない（現状把握が目的）。

### 完了条件
- `bun run test:coverage` で `coverage/rust/lcov.info` と `summary.md` が生成される

---

## タスク5: カバレッジの低い重要モジュールへのテスト追加

### 前提
タスク4の `summary.md` を見て、次の優先順位で対象を決める。
1. `persistence/`（マイグレーション・設定。壊れるとユーザーデータに影響）
2. `runtime/context/`（Context 構成・Scope 検査）
3. `records/`（`write.rs`, `forget.rs`, `read.rs`, `capture.rs`）
4. `tool_selection/`（権限と実行可否の判断）

### 手順
1. 対象モジュールで、公開関数（`pub`/`pub(crate)`）のうちテストから一度も呼ばれていないものを lcov から特定する。
2. 各関数に、正常系 1 件・境界値 1 件・エラー系 1 件を書く。既存テストのヘルパー（`test_support.rs`, `test_state.rs`, `test_environment.rs`）を使い、新しい共通ヘルパーは作らない。
3. DB が必要なテストは既存の一時 DB ヘルパーを使う。実ユーザーの DB パスを参照しない。
4. `persistence/` のマイグレーションは、「空 DB から最新まで」と「1 つ前のスキーマに既存データを入れてから最新まで」の 2 パターンを必ず含める。

### 完了条件
- 対象 4 モジュールの行カバレッジがそれぞれ着手前から +15 ポイント以上、または 70% 以上

---

## タスク6: `spec/docs` の整理

### 背景
`spec/docs` に 108 ファイル、`spec/docs/.archived` に 25 ファイルある。実装計画書や引き継ぎ文書が混在し、どれが最新の正本か分かりにくい。

### 手順
1. `spec/docs` 直下の各ファイルを次の 3 つに分類した一覧表 `spec/docs/INDEX.md` を作る。列: ファイル名 / 分類 / 一行要約 / 最終更新日（`git log -1 --format=%ad --date=short -- <file>`）。
   - **正本**: 現在の仕様・設計・コンセプト（例: `saaa-personal-ai-concept.md`, `adr/`）
   - **進行中**: 実装が終わっていない計画書
   - **完了・古い**: 実装済みの計画書、`*-handoff.md`、同じテーマの新しい版があるもの
2. 「完了・古い」の判定基準: 計画書に書かれた主要な型・関数・ファイルが `rg` でコードに存在する、または同テーマで日付が新しい文書がある。判定に迷うものは「進行中」にして INDEX の備考に理由を書く。
3. 「完了・古い」を `git mv` で `spec/docs/.archived/` に移す。内容は編集しない。
4. リポジトリ内のリンク切れを直す: `rg -l "spec/docs/<移動したファイル名>"` で参照元を探し、パスを更新する。README.md と README.ja.md も確認する。
5. `bun run spec:check` を通す。

### 完了条件
- `spec/docs` 直下が「正本」と「進行中」だけになり、`INDEX.md` から全ファイルに辿れる
- `bun run spec:check` が成功

---

## タスク7: 主要体験の通し受け入れテスト

### 背景
機能は多いが、初期状態でユーザーが得られる価値を最初から最後まで通して確認するテストがない。まず 1 本だけ作る。

### 対象シナリオ（この 1 本に限定する）
「ユーザーがテキストで依頼 → 応答がストリーミング表示 → 会話が永続化 → アプリ再起動後に `recall_conversation` でその会話を取得できる」

### 手順
1. 既存の `scripts/desktop-e2e.ts` と `scripts/desktop-smoke.ts` を読み、同じ起動方法（一時データディレクトリ、`SAAA_SMOKE_DATA_DIR` など）を使う。
2. プロバイダは `scripts/adaptive-improvement/mock-tool-selection.json` と同様のモックを使い、外部 API を呼ばない。モックの仕組みがなければ、Rust 側の既存テスト用プロバイダ（`test_support.rs` を確認）を使う Rust の統合テストとして書く。
3. 検証項目: ストリーミングで 2 回以上の部分応答を受け取る / DB の会話テーブルに依頼と応答が保存される / 再起動後の recall 結果に依頼文が含まれる。
4. `package.json` に `"acceptance:core": "..."` を追加する。
5. 初回応答の経路を変更する場合は、該当する回帰試験を行う。

### 完了条件
- `bun run acceptance:core` がモックだけで成功し、所要時間が 2 分以内

---

## 実施順とおおよその規模

| 順番 | タスク | 規模の目安 |
| --- | --- | --- |
| 1 | タスク3 `unsafe` コメント | 小（1 PR） |
| 2 | タスク4 Rust カバレッジ計測 | 小（1 PR） |
| 3 | タスク6 `spec/docs` 整理 | 中（2〜3 PR） |
| 4 | タスク1 連番分割の解消 | 大（60 PR 前後、1 ディレクトリ 1 PR） |
| 5 | タスク2 `unwrap()` 削減 | 大（タスク1完了後） |
| 6 | タスク5 テスト追加 | 中 |
| 7 | タスク7 通し受け入れテスト | 中 |

タスク3・4・6 は互いに独立しており並行してよい。タスク2 はタスク1の完了を待つ。
