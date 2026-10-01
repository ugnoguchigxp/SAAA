# World継続保守の実装結果

SAAA内の独立実装と最終レビュー1回・指摘修正を実施。本番DB・保存設定・モデル・サービスの変更、commit/push、worktree/複製は実施していない。

旧API参照の試験コンパイル不整合9件は全件解消。廃止Runtimeを復元せず現行World構成・provider境界・会話キューに対応付けた。未登録Scopeの2試験も会話状態に関する元の検証を保ち、Worldのheldを検証する現仕様へ移行した。

## 最新の対象検証

- Personal State: 168件合格。既存実機/性能等のignored 3件は未実施。
- World runtime: 55件合格。
- SQLite Writer/Reader: 16件合格。既存helper ignored 1件は未実施。
- core: unit 106件、integration 15件合格。
- 会話キューE2E: 2件+再接続/後続失敗1件合格。
- World結果観測: 1件（6方向/条件、冪等再送、Scope認可取消で非表示、Forgetで消去）合格。
- World UI: 2件合格。Frontend typecheckと対象lint合格。
- 先行のIPC契約2件、診断1件、通常Rust checkも合格。

最新runner結果は [offline-report.json](offline-report.json)。全体completeはfalse。ContextStill依存4項目と実機受入をfixture合格で代替しない。P6の終了コード1は未完了受入を示す。最終runnerの診断再実行結果は同JSONを正とする。

## 実装とレビュー

接続前admission、前景予約/cancel、段階checkpoint・再開、backoff、1024 pending上限とrefill、新規/過去3:1、本人user/登録Project Scope、版付き複数引用、Objective後のWorld処理、同条件反例比較、型付き結果観測、関係根拠履歴、Local binding照合、World表示・質問/訂正キュー接続、Memory設定保持・overrideを実装した。新しい別プロセスWriterや本番DBへの直接writeは追加していない。

最終レビュー1回の指摘: Scope未解決時の旧試験期待、非UTF-8のMemory override、Scope認可/原資料の可用性を失った観測のsnapshot表示。修正し対象検証した。追加レビュー循環は行っていない。

## 残る条件

ContextStillのworld_evidence_v1契約（immutable版、原文UTF-8範囲、Scope認可、lineage、変更cursor、失効/削除イベント、commit前再検証）と別側実装が必要。依頼書は spec/docs/contextstill-world-evidence-contract-request.md に保存済み・未送信。外部取得の本番接続は未実装。

実環境のLocal binding登録・runtimeがcloud転送しないことの確認、実LocalLLM品質・音声併用・障害注入・24時間常時起動受入は未実施。今回設定の変更や実機サービス起動は行わない。

## 追加依頼によるsize修正

今回変更した69 sourceファイルの違反は0件。World抽出commit、job lifecycle、World stage、Scope解決、snapshot、前景予約、音声job、UI保守操作、schema/翻訳を責務別の実submodule/componentへ分割した。include!分割・閾値引上げ・チェック無効化は行っていない。

既定の `scripts/module-size-baseline.ts` の `supplementBaseline` を今回の新規21ファイルに限定して実行した。全体add-missingは無関係な既存違反で停止するため、同関数の対象限定で登録した。既存baseline値の変更0件。変更対象全体を既存evaluate関数で検証し、新規登録・ratchet・hard budget・禁止include分割に違反がないことを確認した。[size-report.json](size-report.json)を証拠とする。

全体size:checkは今回の変更対象外の既存違反により不合格のまま。既存例: artifact_preview/webview_ops.rs 610/ratchet172、diagnosis/runner.rs 278/200、persistence/settings_migration/stored_document.rs 494/438、既存未登録・削除済baseline・禁止include!分割。無関係な修復には広げていない。

分割後の対象検証: Personal State 168件合格・既存ignored3件、Frontend typecheck、World UI2件とsize実装/登録試験14件（計16件）、対象lint、git diff --check合格。会話キューE2E2件と後続失敗1件も合格。新しいレビュー循環は実施していない。上記の外部契約・Local実行・実機受入条件は変更しない。
