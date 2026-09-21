# Artifact Viewer 実装計画 — OpenUI アーティファクト、Markdown ビューア、Mermaid

作成日: 2026-09-21。状態: **実装済み（AV-12 live のみ未実施）。** 進捗の正本は §0。

前提: 執事に「ブログ記事を事前に書いてもらう」体験を live の一日で成立させる。記事は複数本を同時に開いて読み比べられ、改訂が追える。画像同梱・エクスポート・リモート画像・KaTeX は本計画の範囲外（§8）。

上位は[Personal AI Concept](saaa-personal-ai-concept.md) §9（委任の成果物）と [Generative Inline UI 実装](generative-inline-ui-implementation.html)。本計画は既存の Generative UI（`@openuidev/react-lang` 0.2.15、`src-tauri/src/generative_ui/`、`src/features/chat/ui/`）を**拡張**するもので、並行する新しい成果物ストアは作らない。

## 0. 現在の実装状況

| 状態 | 範囲 | 実装済み | 未実装・不足 |
| --- | --- | --- | --- |
| 実装済み | Artifact Viewer | 既存 view ストア上の `Markdown`、安全な Mermaid 描画、右側 Artifact 画面、最大 8 タブ、改訂切替と行差分。Artifact 表示時はチャット欄を縮める。幅ポリシーは `50 / 60 / 100%` に対応し、現在は全種 50% | AV-12 live の実施・記録 |
| 既存 | チャット Markdown | `renderSafeMarkdown`（見出し・フェンス・表・リスト・引用・インライン）。生 HTML 常時エスケープ。Worker 描画 | 画像、Mermaid、フェンス言語別の扱い |
| 既存 | レイアウト | `Surface = chat \| settings` の全面切替。Drawer コンポーネント無し | Artifact Drawer |
| 未着手 | AV-00〜AV-12 | — | 全部 |

## 1. 次に完成させるもの

三つを足す。混ぜない。

1. **OpenUI アーティファクト**: 保存済み UI view（`ui_views`）を「アーティファクト」として扱い、チャットのインラインカードから**右側の Artifact 画面に開く**。Artifact の幅だけチャット欄を縮める。複数タブ。改訂の切替。新しいテーブルは作らない。
2. **Markdown ビューア**: OpenUI 語彙に `Markdown` kind を追加する（`args: [text]`）。描画は `renderSafeMarkdown` の拡張版。チャット本文と同じセーフレンダラーを使い、外部 Markdown ライブラリは入れない。
3. **Mermaid**: ` ```mermaid ` フェンスを図として描画する。`mermaid` を lazy import、`securityLevel: "strict"`、`htmlLabels: false`。描画失敗時はフェンスをコードとして残す。

「記事 = `Markdown` ノード 1 個を持つ saved view」と定義する。改訂・名前・検索・published は既存の view 機構がそのまま効く。

## 2. 正本と派生

| 種類 | 正本 | 派生 |
| --- | --- | --- |
| 記事本文 | `ui_view_revisions.definition`（JSON、`Markdown` ノード） | Drawer の描画、Mermaid SVG |
| 記事の改訂 | `ui_view_revisions.revision` | Drawer の改訂セレクタ、簡易 diff |
| 会話との対応 | `ui_instances` + `conversation_message_parts` | インラインカード |
| Artifact 画面の開閉・タブ順 | フロントの一時状態 | — 。DB に置かない |

Mermaid SVG はキャッシュしない。定義文字列から毎回描く。

## 3. バックエンド（Rust）

### 3.1 `Markdown` kind

- `parser.rs`: `"Markdown"` を追加。`args` は文字列 1 つ。`children` 不可。`span` は `Cell` 内でのみ意味を持つ。
- 本文サイズ: `args[0]` 単体で 48 KiB 上限（定義全体 64 KiB の内側）。超過は既存の `UI exceeds limits` と同じ拒否。
- 本文の検証はサイズと UTF-8 だけ。Markdown 構文は検証しない（描画側が安全に落とす）。
- `library_version` は据え置き。既存 view は影響を受けない。

### 3.2 ツール定義（`tools.rs`）

- `present_ui` の説明に `Markdown(args:[markdown text])` を追加。Mermaid フェンスが使えることを 1 文で書く。
- 記事の提示は既存の `present_ui` → `save_ui` の 2 段で足りる。**新しいツールは追加しない。** 「ブログ記事を書いて」に対して LLM は `present_ui` で `{kind:'Markdown',args:['...']}` を出し、必要なら `save_ui` で名前を付ける。
- 「1 ターン最大 3 new UI」の上限は据え置き。記事 3 本を 1 ターンで出せる。

### 3.3 IPC

- 既存 `get_ui` / `open_ui` / `search_ui` に加え、Artifact 画面用に `list_ui_view_revisions(view_id)` を 1 本追加（`revision, summary, created_at` の一覧）。定義本文は改訂ごとに `get_ui` 相当で取る。
- `ipc_contract.rs` と `bun run ipc:check` の生成物を更新。

## 4. フロントエンド

### 4.1 Markdown 描画の拡張（`markdownRenderer.ts`）

- フェンスの言語を `<pre data-lang="…">` に保持する。`mermaid` の場合は `<div class="mermaid-source" hidden>` に**テキストとして**入れ、後段で置換する。HTML には決してならない。
- 画像タグは本計画では追加しない（`![]()` はリンクと同じ扱い。テキストとして残る）。
- Worker 描画は据え置き。Mermaid の描画は Worker ではなくメインスレッドで、DOM 挿入後に行う。

### 4.2 Mermaid（`src/features/chat/ui/mermaid.ts`、新規）

- `import("mermaid")` を初回だけ。`initialize({ startOnLoad: false, securityLevel: "strict", htmlLabels: false, theme: "neutral" })`。
- `render(id, source)` の結果 SVG を `DOMPurify` 無しで挿入するのは不可。`securityLevel: "strict"` でも `<foreignObject>` や `on*` 属性の混入を防ぐため、SVG を `DOMParser` で解析して**許可タグ・属性のホワイトリスト**（`svg g path rect circle ellipse line polyline polygon text tspan marker defs style` と `class id d x y width height fill stroke transform viewBox …`）だけ残して再構築する。`<style>` は mermaid 生成分のみ許可し、`url(` を含む場合は落とす。
- 描画失敗、タイムアウト（3 秒）、ソース 8 KiB 超はフェンスをコードのまま表示し、右上に「図として描けませんでした」を出す。
- CSP は現行のまま（`style-src 'unsafe-inline'` が既にある）。`img-src` に追加しない。

### 4.3 `Markdown` コンポーネント（`SemanticRenderer.tsx`）

- `case "Markdown"`: `renderSafeMarkdown(args[0])` を `dangerouslySetInnerHTML`、マウント後に mermaid 置換。
- インライン表示（チャット内）では高さ 320px で切り、「開く」ボタンで Artifact 画面へ。Artifact 画面内は全文。

### 4.4 Artifact Workspace（`src/features/chat/artifacts/ArtifactDrawer.tsx`、新規）

- chat 面の右側に分割表示し、その幅だけチャット欄を縮める。幅ポリシーは 50% / 60% / 100% を返せる境界に分離し、現時点は全アーティファクト 50%。狭幅では全面。
- タブ: `instance_id` 単位。同じ view の別 instance は同一タブに束ねる。上限 8。閉じる、順序はドラッグ無し（並び替えはしない）。
- 各タブ: タイトル（`ui_views.name` か `summary` の先頭）、改訂セレクタ、本文。改訂を切り替えると `get_ui` で定義を取り直す。
- 簡易 diff: 前改訂との行単位 diff（追加・削除の色付け）。ライブラリを入れず、LCS の最小実装（`src/features/chat/artifacts/lineDiff.ts`）。本文 48 KiB × 2 で十分に速い。
- 開閉状態、タブ、選択改訂はコンポーネント状態のみ。再起動で消えてよい。
- インラインカード（`InlineUi.tsx`）に「ビューアーで開く」を追加。`Markdown` 以外の view（Grid など）も同じ Artifact 画面に開ける。これが「OpenUI アーティファクト」。

### 4.5 i18n

`translations.ts` に ja/en を追加（開く、閉じる、改訂、差分、描画失敗）。

## 5. 依存追加

| パッケージ | 用途 | 判断 |
| --- | --- | --- |
| `mermaid` | 図描画 | 追加。lazy import で初期バンドルに含めない。`bun run size:check` の対象外にする理由を baseline に書く |
| Markdown ライブラリ | — | 追加しない |
| DOMPurify | — | 追加しない。§4.2 の自前ホワイトリストで閉じる |

## 6. 安全境界

- LLM が触れるのは `Markdown` の本文文字列だけ。URL・パス・HTML は本文中のテキストとして扱われ、実行されない。
- Markdown 内リンクは既存規則（`https?:` / `mailto:` のみ、`rel="noreferrer noopener"`）。
- Mermaid の `click` ディレクティブは `securityLevel: "strict"` で無効。加えて SVG 再構築で `<a>` と `on*` を落とす。
- Artifact 画面は読み取り専用。編集は会話で頼む（`get_ui` → `present_ui` with `baseInstanceId`）。

## 7. 作業カード

| ID | 内容 | 受入 |
| --- | --- | --- |
| AV-00 | `Markdown` kind を parser に追加、サイズ上限、単体試験 | 既存試験全通過。48 KiB 超が拒否される |
| AV-01 | `present_ui` 説明を更新 | 説明に Markdown と mermaid フェンスが 1 文ずつ |
| AV-02 | `list_ui_view_revisions` IPC、契約生成 | `bun run ipc:check` 通過 |
| AV-03 | `markdownRenderer` フェンス言語保持、mermaid ソースをテキストとして退避 | 既存 `tests/markdown*.test.ts` 通過。mermaid フェンスが HTML にならない試験 |
| AV-04 | `mermaid.ts`: lazy import、strict、SVG ホワイトリスト再構築、失敗時フォールバック | `<script>` / `on*` / `<a>` / `<foreignObject>` を含む SVG 文字列が落ちる試験 |
| AV-05 | `SemanticRenderer` に `Markdown`。インライン 320px + 開く | 記事 1 本がチャット内に出て、開くで Drawer |
| AV-06 | `ArtifactDrawer`: タブ、閉じる、上限 8 | 3 本を同時に開けて切り替えられる |
| AV-07 | 改訂セレクタ + `lineDiff.ts` | r1→r2 の追加・削除行が色付きで見える |
| AV-08 | `InlineUi` の全 view に「Drawer で開く」 | Grid の view も Drawer に出る |
| AV-09 | i18n ja/en | 文字列直書き無し |
| AV-10 | CSS、狭幅の全面化 | 720px 幅で Drawer が全面 |
| AV-11 | `size:check` baseline、`mermaid` の lazy chunk 確認 | 初期バンドル増分が 10 KB 以内 |
| AV-12 | live: 「〇〇について記事を 3 本書いて。1 本には構成図を mermaid で入れて」→ 3 タブ、図が描かれ、「2 本目をもう少し短く」→ r2 の diff が見える | `spec/evidence/artifact-viewer/live-YYYYMMDD.md` に記録 |

順序は AV-00 → 01 → 03 → 04 → 05 → 06 → 02 → 07 → 08 → 09 → 10 → 11 → 12。AV-05 完了で最小の体験が成立する。

## 8. 範囲外（次の計画）

- 画像同梱（`attachments/`、asset protocol のスコープ、`convertFileSrc`、capability 追加）
- coding workspace へのエクスポート（MD と画像のコピー）
- Steward 成果物との接続（`steward_task_artifacts.kind='ui_view'`、`reference=view_id`）。テーブル変更は不要で 1 行の追加になる
- リモート画像（CSP `img-src` に `https:` を足さない）
- KaTeX、シンタックスハイライト
- Drawer 状態の永続化

## 9. 検証ゲート

| コマンド | 要件 |
| --- | --- |
| `cargo test --lib generative_ui` | 通過 |
| `bun test tests/markdown*.test.ts tests/mermaid*.test.ts` | 通過 |
| `bun run ipc:check` | 通過 |
| `bun run size:check` | 通過（baseline 更新は AV-11 の理由付きのみ） |
| `bun run check:local` | 通過 |
| AV-12 live | evidence 記録 |
