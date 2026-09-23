# spec/docs 索引

作成日: 2026-09-23
目的: `spec/docs` 直下と `.archived/` の文書を正本 / 進行中 / 完了・古い に分類し、現役文書と完了文書を辿れるようにする。

分類基準:
- **正本**: 現行の仕様・設計・コンセプト（`adr/` 含む）
- **進行中**: 主作業がコードに明確にない、または判定が曖昧な計画（備考に理由）
- **完了・古い**: `*-handoff.md`、主成果がリポジトリに存在する計画、同テーマの新版があるもの（`.archived/` へ移動済み）

## 正本

| ファイル名 | 分類 | 一行要約 | 最終更新日 | 備考 |
| --- | --- | --- | --- | --- |
| `INDEX.md` | 正本 | 本索引（分類表） | 2026-09-23 |  |
| `README.html` | 正本 | 内部設計書ディレクトリの案内 | 2026-09-21 |  |
| `plan.html` | 正本 | Concept & Direction の主計画索引 | 2026-08-29 |  |
| `product-readiness-status.html` | 正本 | 製品準備状況の正本 | 2026-09-21 |  |
| `product-readiness-acceptance-runbook.html` | 正本 | 実機・配布物の手動受入手順 | 2026-09-17 |  |
| `saaa-personal-ai-concept.md` | 正本 | Personal AI 上位コンセプト | 2026-09-17 |  |
| `saaa-capability-tool-runtime-concept.md` | 正本 | Capability/Tool Runtime コンセプト | 2026-09-17 |  |
| `saaa-adaptive-learning-memory-concept.html` | 正本 | 適応学習・選択的 Memory コンセプト | 2026-09-17 |  |
| `saaa-interface-artifact-runtime-concept.md` | 正本 | Interface/Artifact Runtime コンセプト | 2026-09-17 |  |
| `saaa-llang-dynamic-capability-concept.md` | 正本 | L-Lang 動的能力コンセプト | 2026-09-20 |  |
| `saaa-personal-world-model-concept.md` | 正本 | Personal World Model（五要素）コンセプト | 2026-09-20 |  |
| `context-window-records-memory-design.md` | 正本 | Context/records/memory 統合の現行設計 | 2026-09-22 |  |
| `larm-http-api-review.md` | 正本 | LARM HTTP/設定の現行参照 | 2026-09-22 |  |
| `saaa-ui-concept-v1.png` | 正本 | UI コンセプト画像 | 2026-08-29 |  |
| `saaa-settings-harness-concept-v1.png` | 正本 | Provider Harness 設定コンセプト画像 | 2026-08-30 |  |
| `saaa-settings-individual-cloud-services-concept-v1.png` | 正本 | 個別 Cloud 設定コンセプト画像 | 2026-08-30 |  |
| `saaa-settings-individual-services-redesign-v2.png` | 正本 | 個別サービス設定再設計 v2 画像 | 2026-09-23 |  |
| `saaa-implementation-methods-pi-codex-sdk-concept-v1.png` | 正本 | pi / Codex SDK 実装方式コンセプト画像 | 2026-09-23 |  |
| `adr/0001-mvp-runtime-boundaries.html` | 正本 | MVP Runtime 境界 ADR | 2026-09-13 |  |
| `adr/0002-situation-signal-privacy.html` | 正本 | Situation 信号プライバシー ADR | 2026-08-28 |  |
| `adr/0003-input-activity-signal-privacy.html` | 正本 | 入力活動信号プライバシー ADR | 2026-09-13 |  |

## 進行中

| ファイル名 | 分類 | 一行要約 | 最終更新日 | 備考 |
| --- | --- | --- | --- | --- |
| `project-health-improvement-plan.md` | 進行中 | リポジトリ健全性改善の現行タスク計画 | 2026-09-23 | アクティブな計画のため未移動（タスク6対象外） |
| `personal-state-architecture-roadmap.md` | 進行中 | Personal State 全体ロードマップ | 2026-09-17 | 実装中・実機受入待ちの記載あり |
| `personal-state-phase-1-plan.md` | 進行中 | Personal State P1 接続計画 | 2026-09-16 | 基盤はあるが P1 完了判定は未確定 |
| `saaa-required-context-completion-plan.md` | 進行中 | 必須 Context 欠落防止計画 | 2026-09-21 | コードはあるが実モデル/ネイティブ UI 受入が未完 |
| `saaa-role-routing-plan.md` | 進行中 | Role Routing v3 計画 | 2026-09-21 | モジュールはあるがカード受入・製品受入が未完 |
| `saaa-role-routing-work-cards.md` | 進行中 | Role Routing 作業カード | 2026-09-22 | 監査済み・未完了カードあり |
| `saaa-role-routing-completion-roadmap.md` | 進行中 | Role Routing 残作業ロードマップ | 2026-09-22 | 計画のみで実装完了を示さない |
| `saaa-role-routing-execution-contract.md` | 進行中 | Role Routing 実行契約 | 2026-09-21 | 部分実装・受入未完了の現行契約 |
| `saaa-role-routing-learning-contract.md` | 進行中 | Role Routing 学習契約 | 2026-09-20 | 設計段階。本番適用は後続 |
| `saaa-role-routing-acceptance.md` | 進行中 | Role Routing 受入マトリクス | 2026-09-21 | 受入未完了 |
| `saaa-role-routing-voice-integration.md` | 進行中 | 音声会話向け Role Routing 改訂 | 2026-09-21 | 実装予定・受入未完了 |
| `saaa-role-routing-location-switch-plan.md` | 進行中 | 在宅 LARM / 外出 Cloud 切替計画 | 2026-09-22 | Phase3 未接続 |
| `voicevox-expressive-tts-terra-plan.md` | 進行中 | VOICEVOX 韻律・予約語計画 | 2026-09-23 | VE-10/11 など残作業あり |
| `codex-sdk-realtime-second-opinion-implementation-plan.html` | 進行中 | Codex second opinion 計画 | 2026-09-01 | Status: Proposed。実装痕跡なし |
| `context-window-records-memory-implementation-plan.md` | 進行中 | Context Window / records 実装計画 | 2026-09-22 | Segment 既定 OFF・CW-55 等未了で完了断定不可 |
| `saaa-review-five-improvements-terra-plan.md` | 進行中 | 再レビュー改善5点の統合計画 | 2026-09-21 | evidence は content root 外のためリンク不可。パス表記で残置（進行中） |

## 完了・古い（`.archived/`）

| ファイル名 | 分類 | 一行要約 | 最終更新日 | 備考 |
| --- | --- | --- | --- | --- |
| `.archived/context-window-records-memory-handoff.md` | 完了・古い | Context Window 担当引き継ぎ | 2026-09-22 | 本タスクで移動 |
| `.archived/context-memory-unified-runtime-implementation-plan.md` | 完了・古い | Context/PS 共通 Runtime 統合計画 | 2026-09-17 | 本タスクで移動 |
| `.archived/always-on-streaming-asr-implementation-plan.html` | 完了・古い | 常時ストリーミング ASR 計画 | 2026-08-31 | 本タスクで移動 |
| `.archived/code-quality-module-size-and-coverage-implementation-plan.html` | 完了・古い | モジュール分割・coverage 計画 | 2026-08-29 | 本タスクで移動 |
| `.archived/conversation-voice-quality-implementation-plan.html` | 完了・古い | 会話音声品質契約の実装計画 | 2026-08-30 | 本タスクで移動 |
| `.archived/conversation-reasoning-mvp-implementation-plan.html` | 完了・古い | 会話+MCP 推論 MVP 計画 | 2026-09-12 | 本タスクで移動 |
| `.archived/conversation-reasoning-mvp-implementation.html` | 完了・古い | 会話+MCP 推論 MVP 実装記録 | 2026-09-17 | 本タスクで移動 |
| `.archived/generative-inline-ui-implementation-plan.html` | 完了・古い | Generative Inline UI 計画 | 2026-09-12 | 本タスクで移動 |
| `.archived/generative-inline-ui-implementation.html` | 完了・古い | Generative Inline UI 実装結果 | 2026-09-12 | 本タスクで移動 |
| `.archived/generative-inline-ui-review.html` | 完了・古い | Generative Inline UI レビュー記録 | 2026-09-12 | 本タスクで移動 |
| `.archived/http-provider-standardization-implementation.html` | 完了・古い | HTTP Provider 標準化の実装記録 | 2026-09-16 | 本タスクで移動 |
| `.archived/http-provider-live-verification-2026-09-06.html` | 完了・古い | HTTP Provider 実機検証メモ | 2026-09-12 | 本タスクで移動 |
| `.archived/larm-live-feature-verification.md` | 完了・古い | LARM 公開機能の実機検証記録 | 2026-09-16 | 本タスクで移動 |
| `.archived/larm-personal-state-contract-review.md` | 完了・古い | LARM Personal State 契約照合レビュー | 2026-09-16 | 本タスクで移動 |
| `.archived/maximum-performance-streaming-implementation-plan.html` | 完了・古い | 最大性能ストリーミング計画 | 2026-09-16 | 本タスクで移動 |
| `.archived/maximum-performance-streaming-release-evidence.html` | 完了・古い | 最大性能ストリーミング release evidence | 2026-09-16 | 本タスクで移動 |
| `.archived/mvp-2-release-evidence.html` | 完了・古い | MVP2 release evidence | 2026-09-13 | 本タスクで移動 |
| `.archived/mvp-2.5-release-evidence.html` | 完了・古い | MVP2.5 release evidence | 2026-09-13 | 本タスクで移動 |
| `.archived/mvp-2.6-release-evidence.html` | 完了・古い | MVP2.6 release evidence | 2026-08-29 | 本タスクで移動 |
| `.archived/mvp-2.6-larm-operations-runbook.html` | 完了・古い | MVP2.6 LARM 運用 runbook | 2026-09-16 | 本タスクで移動 |
| `.archived/mvp-3-memory-architecture-implementation-plan.html` | 完了・古い | MVP3 Memory アーキテクチャ計画 | 2026-09-13 | 本タスクで移動 |
| `.archived/product-readiness-improvement-plan.html` | 完了・古い | 製品品質・導入性改善計画 | 2026-09-21 | 本タスクで移動 |
| `.archived/settings-provider-harness-cloud-voice-implementation-plan.html` | 完了・古い | Harness/個別 Cloud 設定計画 | 2026-08-30 | 本タスクで移動 |
| `.archived/sqlite-single-writer-implementation-plan.html` | 完了・古い | SQLite Single Writer 計画 | 2026-09-01 | 本タスクで移動 |
| `.archived/streaming-tts-sentence-boundary-implementation-plan.html` | 完了・古い | Streaming TTS 文境界計画 | 2026-09-01 | 本タスクで移動 |
| `.archived/target-speaker-enrollment-implementation-plan.html` | 完了・古い | 話者登録・ゲート計画 | 2026-09-06 | 本タスクで移動 |
| `.archived/verified-quality-improvements-implementation-plan.html` | 完了・古い | 検証済み品質改善の統合計画 | 2026-09-13 | 本タスクで移動 |
| `.archived/voice-behavior-tool-implementation-plan.html` | 完了・古い | Voice Behavior 構想草案 | 2026-08-29 | 本タスクで移動 |
| `.archived/continuity-world-model-direction.md` | 完了・古い | 中間メモリ方向性の評価メモ | 2026-09-13 | 本タスクで移動 |
| `.archived/voicemem-integration-assessment.md` | 完了・古い | VoiceMem 連携調査 | 2026-09-13 | 本タスクで移動 |
| `.archived/pi-mission-pilot-interface-implementation-plan.md` | 完了・古い | Mission Pilot / pi 計画（置換済み） | 2026-09-16 | 本タスクで移動 |
| `.archived/saaa-pi-tools-implementation-plan.md` | 完了・古い | pi ツール実装計画 | 2026-09-16 | 本タスクで移動 |
| `.archived/saaa-connect-and-module-budget-plan.md` | 完了・古い | 接続とモジュール予算計画 | 2026-09-21 | 本タスクで移動 |
| `.archived/saaa-generation-closeout-plan.md` | 完了・古い | 生成検査クローズアウト計画 | 2026-09-21 | 本タスクで移動 |
| `.archived/saaa-llang-dynamic-capability-initial-plan.md` | 完了・古い | L-Lang 動的能力の初期計画 | 2026-09-20 | 本タスクで移動 |
| `.archived/saaa-llang-dynamic-capability-implementation-guide.md` | 完了・古い | L-Lang M0/M1 実装ガイド | 2026-09-20 | 本タスクで移動 |
| `.archived/saaa-llang-dynamic-capability-m2a-plan.md` | 完了・古い | L-Lang M2A 計画 | 2026-09-20 | 本タスクで移動 |
| `.archived/saaa-llang-dynamic-capability-m2b-plan.md` | 完了・古い | L-Lang M2B 計画 | 2026-09-20 | 本タスクで移動 |
| `.archived/saaa-llang-dynamic-capability-m2b-transport-reference.md` | 完了・古い | 旧 M2B HTTP 輸送参考 | 2026-09-20 | 本タスクで移動 |
| `.archived/saaa-llang-generation-inspection-plan.md` | 完了・古い | L-Lang 生成・検査統合計画 | 2026-09-20 | 本タスクで移動 |
| `.archived/saaa-tool-selection-d0-d3-implementation-guide.md` | 完了・古い | ツール選択 D0–D3 実装ガイド | 2026-09-20 | 本タスクで移動 |
| `.archived/saaa-tool-selection-d4-mcp-implementation-plan.md` | 完了・古い | ツール選択 D4 外部 MCP 計画 | 2026-09-20 | 本タスクで移動 |
| `.archived/saaa-tool-selection-d5-mcp-server-plan.md` | 完了・古い | ツール選択 D5 MCP 公開計画 | 2026-09-20 | 本タスクで移動 |
| `.archived/saaa-steward-loop-phase-plan.md` | 完了・古い | 執事循環フェーズ計画 | 2026-09-20 | 本タスクで移動 |
| `.archived/saaa-steward-loop-phase-work-cards.md` | 完了・古い | 執事循環作業カード | 2026-09-20 | 本タスクで移動 |
| `.archived/saaa-minimal-loop-plan.md` | 完了・古い | 最小執事循環計画 | 2026-09-20 | 本タスクで移動 |
| `.archived/saaa-situation-tts-gate-plan.md` | 完了・古い | Situation TTS ゲート計画 | 2026-09-20 | 本タスクで移動 |
| `.archived/saaa-butler-schedule-ledger-plan.md` | 完了・古い | Butler Schedule Ledger 計画 | 2026-09-21 | 本タスクで移動 |
| `.archived/saaa-butler-schedule-ledger-work-cards.md` | 完了・古い | Schedule Ledger 作業カード | 2026-09-21 | 本タスクで移動 |
| `.archived/saaa-delegated-work-completion-plan.md` | 完了・古い | 委任仕事完了計画 | 2026-09-21 | 本タスクで移動 |
| `.archived/saaa-delegated-work-repair-terra-plan.md` | 完了・古い | 委任仕事残件の Terra 計画 | 2026-09-21 | 本タスクで移動 |
| `.archived/saaa-steward-correctness-repair-plan.md` | 完了・古い | Steward 整合性改修計画 | 2026-09-22 | 本タスクで移動 |
| `.archived/saaa-adaptive-improvement-completion-plan.md` | 完了・古い | 適応改善完了計画 | 2026-09-21 | 本タスクで移動 |
| `.archived/saaa-world-delivery-completion-plan.md` | 完了・古い | World 全経路供給計画 | 2026-09-21 | 本タスクで移動 |
| `.archived/saaa-world-model-remaining-terra-plan.md` | 完了・古い | World Model 残件 Terra 計画 | 2026-09-21 | 本タスクで移動 |
| `.archived/saaa-personal-world-model-initial-plan.md` | 完了・古い | Personal World Model 初期改訂計画 | 2026-09-20 | 本タスクで移動 |
| `.archived/saaa-personal-world-model-v2-execution-contract.md` | 完了・古い | WM v2 実行契約 | 2026-09-20 | 本タスクで移動 |
| `.archived/saaa-personal-world-model-v2-work-cards.md` | 完了・古い | WM v2 作業カード | 2026-09-20 | 本タスクで移動 |
| `.archived/saaa-personal-world-model-m2-plan.md` | 完了・古い | WM M2 計画 | 2026-09-20 | 本タスクで移動 |
| `.archived/saaa-personal-world-model-m2-execution-contract.md` | 完了・古い | WM M2 実行契約 | 2026-09-20 | 本タスクで移動 |
| `.archived/saaa-personal-world-model-m2-work-cards.md` | 完了・古い | WM M2 作業カード | 2026-09-20 | 本タスクで移動 |
| `.archived/saaa-personal-world-model-m3-plan.md` | 完了・古い | WM M3A 計画 | 2026-09-20 | 本タスクで移動 |
| `.archived/saaa-personal-world-model-m3-execution-contract.md` | 完了・古い | WM M3 実行契約 | 2026-09-20 | 本タスクで移動 |
| `.archived/saaa-personal-world-model-m3-work-cards.md` | 完了・古い | WM M3 作業カード | 2026-09-20 | 本タスクで移動 |
| `.archived/saaa-personal-world-model-m3b-plan.md` | 完了・古い | WM M3B 計画 | 2026-09-20 | 本タスクで移動 |
| `.archived/saaa-personal-world-model-m4a-plan.md` | 完了・古い | WM M4A 計画 | 2026-09-21 | 本タスクで移動 |
| `.archived/saaa-personal-world-model-graph-answer-plan.md` | 完了・古い | グラフ回答（G1）計画 | 2026-09-21 | 本タスクで移動 |
| `.archived/saaa-artifact-viewer-plan.md` | 完了・古い | Artifact Viewer 計画 | 2026-09-22 | 本タスクで移動 |
| `.archived/saaa-artifact-webview-implementation-plan.md` | 完了・古い | Artifact WebView 実装計画 | 2026-09-22 | 本タスクで移動 |
| `.archived/saaa-startup-self-diagnosis-implementation-plan.md` | 完了・古い | 起動時自己診断計画 | 2026-09-22 | 本タスクで移動 |
| `.archived/saaa-ui-overhaul-implementation-plan.md` | 完了・古い | UI 抜本改修計画 | 2026-09-21 | 本タスクで移動 |
| `.archived/saaa-webview-webfetch-implementation-plan.md` | 完了・古い | WebFetch WebView 移行計画 | 2026-09-22 | 本タスクで移動 |
| `.archived/contextstill-mcp-typed-memory-tools-change-request.html` | 完了・古い | ContextStill 型別 Memory 変更依頼 | 2026-08-29 | 本タスクで移動 |
| `.archived/ARCHIVE.html` | 完了・古い | アーカイブ方針 | 2026-08-29 | 既存アーカイブ |
| `.archived/adaptive-learning-concept-correction-implementation-plan.html` | 完了・古い | 適応学習コンセプト修正計画 | 2026-08-31 | 既存アーカイブ |
| `.archived/llm-server-dynamic-connection-api-recovery-request.html` | 完了・古い | LLM サーバ動的接続 API 復旧依頼 | 2026-08-31 | 既存アーカイブ |
| `.archived/mvp-0-implementation-plan.html` | 完了・古い | MVP0 実装計画 | 2026-08-28 | 既存アーカイブ |
| `.archived/mvp-0-implementation-plan.md` | 完了・古い | MVP0 実装計画（Markdown） | 2026-08-28 | 既存アーカイブ |
| `.archived/mvp-0-release-evidence.html` | 完了・古い | MVP0 release evidence | 2026-08-28 | 既存アーカイブ |
| `.archived/mvp-0-release-evidence.md` | 完了・古い | MVP0 release evidence（Markdown） | 2026-08-28 | 既存アーカイブ |
| `.archived/mvp-1-implementation-plan.html` | 完了・古い | MVP1 実装計画 | 2026-08-28 | 既存アーカイブ |
| `.archived/mvp-1-implementation-plan.md` | 完了・古い | MVP1 実装計画（Markdown） | 2026-08-28 | 既存アーカイブ |
| `.archived/mvp-1-release-evidence.html` | 完了・古い | MVP1 release evidence | 2026-08-28 | 既存アーカイブ |
| `.archived/mvp-1-release-evidence.md` | 完了・古い | MVP1 release evidence（Markdown） | 2026-08-28 | 既存アーカイブ |
| `.archived/mvp-1.5-implementation-plan.html` | 完了・古い | MVP1.5 実装計画 | 2026-08-29 | 既存アーカイブ |
| `.archived/mvp-1.5-implementation-plan.md` | 完了・古い | MVP1.5 実装計画（Markdown） | 2026-08-28 | 既存アーカイブ |
| `.archived/mvp-1.5-release-evidence.html` | 完了・古い | MVP1.5 release evidence | 2026-08-28 | 既存アーカイブ |
| `.archived/mvp-1.5-release-evidence.md` | 完了・古い | MVP1.5 release evidence（Markdown） | 2026-08-28 | 既存アーカイブ |
| `.archived/mvp-2-2.5-completion-implementation-plan.html` | 完了・古い | MVP2–2.5 完了実装計画 | 2026-08-30 | 既存アーカイブ |
| `.archived/mvp-2-implementation-plan.html` | 完了・古い | MVP2 実装計画 | 2026-08-30 | 既存アーカイブ |
| `.archived/mvp-2-implementation-plan.md` | 完了・古い | MVP2 実装計画（Markdown） | 2026-08-28 | 既存アーカイブ |
| `.archived/mvp-2-release-evidence.md` | 完了・古い | MVP2 release evidence（Markdown） | 2026-08-28 | 既存アーカイブ |
| `.archived/mvp-2.5-implementation-plan.html` | 完了・古い | MVP2.5 実装計画 | 2026-08-29 | 既存アーカイブ |
| `.archived/mvp-2.5-implementation-plan.md` | 完了・古い | MVP2.5 実装計画（Markdown） | 2026-08-28 | 既存アーカイブ |
| `.archived/mvp-2.6-implementation-plan.html` | 完了・古い | MVP2.6 実装計画 | 2026-08-30 | 既存アーカイブ |
| `.archived/mvp-2.6.1-implementation-plan.html` | 完了・古い | MVP2.6.1 実装計画 | 2026-08-30 | 既存アーカイブ |
| `.archived/plan.md` | 完了・古い | 旧主計画（Markdown） | 2026-08-28 | 既存アーカイブ |
| `.archived/saaa-larm-rest-connection-alignment.html` | 完了・古い | LARM REST 接続整合 | 2026-08-31 | 既存アーカイブ |

## 参考ディレクトリ

| パス | 分類 | 一行要約 | 備考 |
| --- | --- | --- | --- |
| `adr/` | 正本 | Architecture Decision Records | 上記 ADR 3 件 |
| `spikes/` | （参考） | 調査スパイク | 直下分類対象外 |
| `verification/` | （参考） | 検証記録 | 直下分類対象外 |
