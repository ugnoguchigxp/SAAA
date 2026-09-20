# Steward loop — deferred

## 見送り中（準備が整っていない。禁止ではない）

- **M3C**（自然文からの候補抽出と commit 採用）。M3B 投入と明示 Goal 正本が先。今足すと Goal 推定が混ざる
- **Butler Schedule**（期限台帳・tick・Calendar）。最小循環の委任正本と Step 3 の hold が先。計画は残置。着手は SL-A から
- **M4**（live 品質評価）。M3B/C の結果を見てから決める
- 固定トリガ以外の自然文理解（ML-03 の実装漏れ。M3C へ）
- live Codex / 実テスト失敗 / 実会議（live未検証）

## 本フェーズ着手禁止

- Role Routing 全体（計画文書は残置、src-tauri/src/role_routing は作らない）
- World Model 五要素の新推定（Goal/相関/条件/confidence/ResearchGap）。M3C が見送りになる理由
- L-Lang の LLM コード生成配備、permissions 非空
- Tool Selection D6、MCP 新 transport/OAuth/resources
- 新しい会話 Runtime、第2 SqliteWriter、独立 daemon、TypeScript 永続化
- 定期ポーリングによるテスト監視（Butler tick が見送りになる理由）
- 委任範囲での自動コード修正
- Situation 分類器・シグナル・hysteresis 定数の変更
- 進行中 TTS チャンクの即時 mute（Step 3 は開始抑止のみ）
- M2B 会話外 Source の永続更新
