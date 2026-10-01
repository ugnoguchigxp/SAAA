# 振り返り World Job の実装・検証記録

日付: 2026-10-01。対象: [実装計画](../../docs/saaa-world-retrospective-selection-implementation-plan.md)。

## 実装した範囲

- 既存 Personal State worker、排他的背景 slot、LocalLLM adapter、SqliteWriter を共有する有限 Job。別の常駐ワーカー・直接 DB 接続は追加していない。
- 確定した所有者会話を登録済み Scope ごとに差分取得。カーソル・版付き窓・lease・保存済み提案を永続化する。元の発言 Job はリセットしない。
- 一窓のメタデータ12件、実引用4発言・合計32,000 bytes、候補8件、全入力48,000 bytes。最後に含まれる実ユーザー発言を主引用とし、その元の観測時刻を保持する。
- 初期は preview。Memory画面の World 表示から停止、候補選定、自動適用を選べる。候補、条件、引用、根拠 ID・版・観測時刻を確認できる。
- 現在性が確認できない歴史窓、長文、不足する条件・根拠は保留する。保存済み提案の適用は再推論しない。無変更・既登録のみの窓では追加 LLM 呼出しを行わない。
- 通常抽出と振り返りの適用層で原根拠の重複支持を防ぐ。条件・比較対象・有効期間は既存 World の identity に従う。
- 単一 Writer 内で結果、receipt、カーソル、完了を原子的に確定。LLM 待ちには DB ロックを保持しない。版、Scope epoch、policy、lease、関連前提を再検査する。
- 忘却・編集・Scope失効で候補と参照を削除・無効化。既知の前提遷移は該当候補だけを再開する。policy改版は版付き窓で権限を再検査する。既存の不変SourceRefに旧policyが記録されている場合は再認可が必要として推論せず保留し、旧発言Jobは変更しない。
- 通常Jobと振り返りの dispatch 公平性を、各キュー内の新旧公平性と分けた。振り返り内でも最新と履歴を3:1で選び、最新処理で未処理履歴のカーソルを進めない。保留詳細の容量が逼迫した場合は疎な保留理由を残して詳細を減らし、最新候補用の枠を確保する。保留提案を含む候補容量を制限し、原ソース保存を止めない。
- 登録済み tokenizer による測定後、入力＋出力予約＋安全余裕が64,000 tokensを超える振り返り要求は chat dispatch 前に拒否する。物理モデルの64k枠確保とは区別する。

## 実装レビュー

レビュー1回と指摘修正を実施。追加のレビュー循環は行っていない。

1. 振り返りを通常Jobの「最古優先の番」に割り込ませると通常履歴が飢餓になるため、dispatch と新旧順序のカウンターを分離した。
2. 推論中に World 前提が変わる場合に備え、適用前の関連前提検査と該当前提遷移だけの再評価を加えた。
3. claim 前の編集・忘却でも窓を消せるよう、enqueue 時点から主入力参照を保存した。
4. byte 上限だけでは64kを保証しないため、実 tokenizer の測定値に出力予約・余裕を含めて検査する。
5. 保存済み提案の identity にモデル・設定 provenance を含め、異なるモデルの推論として再帰属させない。
6. 忘却後のJob IDを再利用しないようAUTOINCREMENTを採用し、古い応答が別Jobの所有権へ混入しないことを検証した。
7. IPC と read projection を機能側の小さなモジュールへ分け、今回変更のサイズ違反を解消した。新規ファイルだけを正規の supplementBaseline で登録し、既存閾値は変更していない。

自動承認レビューは、テストと同時に追加する予定だった未完了 registration の一括 `desired='deleted'` 遷移を拒否した。追加を取りやめ、既存 cleanup 契約を維持した。この一括遷移は実行も実装もしていない。

## 検証

- Personal State 全体: 許可された localhost の隔離 HTTP fixture を用いた実行で189件成功、失敗0件、既存の実機用3件は ignored。
- UI: `world-retrospective.test.tsx` と既存 `world-maintenance.test.tsx`、計3件成功。
- TypeScript: `tsc --noEmit` 成功。
- サイズ: 今回変更の違反なし。全体の size:check には別の既存・並行変更の違反が残る。
- 全体検証後のJob ID再利用防止修正も含め、振り返り対象13件成功、失敗0件、ignored 0件。プレビュー適用、無変更、忘却中の推論、版・lease・policy失効、容量、実token上限、最新と履歴の公平性、根拠表示を確認した。
- 並行変更による一時的なコンパイル不整合と、sandbox内localhost bind失敗は、最終の対象検証では解消。実装レビュー1回の指摘修正は完了。

本番DB、保存済み設定、実サービス、モデル構成は変更していない。commit/pushはしていない。

## 残る受入条件

ContextStillの永続根拠API契約、実Gemmaの品質・速度・認定LocalLLM binding、64k物理枠の共通allocator、24時間運用・DB増分測定は未実施。SAAA内の直列処理はこれらと独立して利用できるが、並列背景実行を有効にする前には正本PageとGemmaセッション計画の優先順位・中断方針を整合させる。

履歴窓の知識を無条件で現在 active にする機能は提供しない。型が過去の有効性を十分に表現できない候補は現在性未確認として保持し、後続の確定会話から再確認する。


## 主な実装箇所

- [Job所有者と新旧選定](../../../src-tauri/src/memory/personal_state/jobs/review.rs)
- [根拠窓と前提検査](../../../src-tauri/src/memory/personal_state/retrospective/window.rs)
- [推論・保存・原子的適用](../../../src-tauri/src/memory/personal_state/retrospective/execution.rs)
- [隔離受入テスト](../../../src-tauri/src/memory/personal_state/retrospective/tests.rs)
- [候補確認と運用モード](../../../src/features/memory/WorldReviewMaintenance.tsx)
