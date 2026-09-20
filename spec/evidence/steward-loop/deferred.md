# Steward loop — deferred

本フェーズでは実装しない。一行で止める。

- Role Routing 全体（計画文書は残置、src-tauri/src/role_routing は作らない）
- World Model 五要素の新推定（Goal/相関/条件/confidence/ResearchGap）
- L-Lang の LLM コード生成配備、permissions 非空
- Tool Selection D6、MCP 新 transport/OAuth/resources
- 新しい会話 Runtime、第2 SqliteWriter、独立 daemon、TypeScript 永続化
- 定期ポーリングによるテスト監視
- 委任範囲での自動コード修正
- Situation 分類器・シグナル・hysteresis 定数の変更
- 進行中 TTS チャンクの即時 mute（Step 3 は開始抑止のみ）
- 自然文からの Goal 抽出（M3C）
- live「今会議中ですか」の正答ゲート（M4。M3B は envelope 検査まで）
- M2B 会話外 Source の永続更新
