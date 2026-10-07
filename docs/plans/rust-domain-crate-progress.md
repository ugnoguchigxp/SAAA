# Rustドメイン分割の作業記録

作成日: 2026年10月7日 JST  
基点HEAD: `cced8b8177b615b50167841fe760a823c0ecbf8b`  
作業ツリーには、この分割とは別のメモリー実装の未コミット変更がある。それらを巻戻していない。

`initial_instructions` はこのセッションのツール一覧に無く、実行していない。

## B00 現行境界

| 操作 | 入口 | 台帳 | 実行状態 |
| --- | --- | --- | --- |
| generate_media | `generation.rs` | reserve → attempt → finish | `MediaService` の run map |
| cancel_media_generation | `mod.rs` | 未登録なら cancelled 行、進行中は cancel_requested | 先着の cancel flag |
| read_generated_media | `artifacts.rs` | accepted の cache、無ければ download 枠2 | 同じ instance の runs |
| list_media_generations | `recovery.rs` | history query、最大20件 | なし |
| reconcile_media_generation | `recovery.rs` | 再照合の attempt と finish | 進行中 run は拒否 |

資格情報は desktop の `credential_secrets` と LARM の env/file。lab は暗黙読出ししない。音声契約は未変更。

既存テストの主な行先:

| 旧 | 新 | gate |
| --- | --- | --- |
| `media_generation` の取消・UUID | `crates/saaa-media` の `two_services_do_not_share_cancellation` | `verify advance --package crates/saaa-media` |
| registry の選択・legacy | desktop の `service_registry/tests.rs` が `saaa-provider-routing` を呼ぶ | desktop 全体。今回は未実行 |
| Replicate HTTP | `src-tauri` の `replicate_tests.rs` | desktop。今回は未実行 |
| media IPC | `ipc_tests.rs`、feature `conversation-queue-e2e` | 未実行 |
| TS 生成・復旧 | `tests/media-generation.test.tsx`、`tests/media-recovery.test.tsx` | 未実行 |

## B01 境界

非media（会話、音声、coding、persistence）が registry を使う。registry を `saaa-media` に置くと、それらが media crate に機能依存する。そのため選択・検証・legacy 導出は `crates/saaa-provider-routing` に置いた。`LocalAvailability::of(AppState)` は desktop に残し、到達性の値だけを渡す。legacy 設定の収集も desktop が行い、導出アルゴリズムは routing crate の一つだけである。

media の SQL は借用 transaction 上の `saaa-media` ledger。commit は desktop writer。RUNS と download 枠は `AppState.media` の `MediaService` 一つ。

HTTP host の5操作とブラウザ adapter は未接続。生成の調停本体はまだ `src-tauri/src/media_generation/generation.rs` にある。台帳と別の調停下書きは削除した。

## 作業状態

| ID | 状態 | 根拠 |
| --- | --- | --- |
| B00, B01 | 完了 | この記録 |
| B02 | 実装済・受入未完了 | report に step 時間、lock 待ち、指紋の一致、試行回数を書く。一致しても入力全体の不変は証明しない。RSS・CPU・compile/link・watcher は未計測。移行前の desktop 全体時間は未測定 |
| U01 | 実装済・受入未完了 | `mediaApiModel.ts` は Tauri を import しない。desktop は `mediaApi.ts`。生成・復旧の TS テストは未実行 |
| U02 | 実装済・受入未完了 | `FeatureLabPreview` と `scripts/feature-lab-preview.tsx`。jsdom の unknown 場面は通過。ブラウザの手動操作は未確認 |
| M01–M07 | 実装済・受入未完了 | routing の静的 verify は OK。media の advance は OK。desktop 全体 advance は未実行 |
| M08 | 未完了 | LARM の discover/generate は desktop の `generation.rs` に残る |
| M09 | 実装済・受入未完了 | Replicate HTTP は `saaa-media` の `replicate.rs`。desktop は `ReplicateIo` を渡す。`media_generation::replicate_tests` は OK |
| M10–M12 | 未完了 | 生成の調停順は desktop。5操作の command は同じ `AppState.media` を使うが、調停関数の正本はまだ desktop |
| H01–H05 | 未着手 | `services/feature-lab` は空。第二の台帳は作っていない |
| V01–V04, V06 | 実装済・受入未完了 | 選択は shadow。`affected` は既存 level を実行し、`--explain` は計画表示で OK を出さない |
| V05 | 実装済・受入未完了 | ロック後と終了時に HEAD、index の diff、汚れたファイルのバイト、規則を比べる。変化なら1回再実行し、なお変化なら OK を出さない。未測定や一致は入力全体の不変証明にしない。watcher は未接続 |
| G01–G03 | 未完了 | 受入の全体 gate と前値比較が無い。改善率は出さない |

## 実行した検証

- `bun run --silent verify --package crates/saaa-provider-routing` → OK
- `bun run --silent verify advance --package crates/saaa-media` → OK（進捗 callback の Send + Sync を含めて再実行）
- `bun run --silent verify test --package src-tauri -- --lib media_generation::replicate_tests` → OK
- `bun run --silent verify test --scope typescript -- tests/verify-affected.test.ts` → OK
- `bun run --silent verify test --scope typescript -- tests/feature-lab-preview.test.ts` → OK
- `bun run --silent verify test --scope typescript -- tests/verify-fingerprint.test.ts` → OK
- desktop 全体、IPC、generated、size、quality、advance、full は未実行

## 次に着手できる ID

M10。desktop の生成順を `saaa-media` の一つの関数へ移し、command は入力と Channel だけにする。自動では始めない。第9節の後続候補は始めない。
