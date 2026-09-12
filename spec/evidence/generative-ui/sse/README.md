# AgentSession SSE GenUI検証 — 2026-09-12

現在の接続先 `http://127.0.0.1:44449`、モデル `muse/muse-spark-1.3-contributor` を使用。接続設定やユーザーの会話DBは変更していない。

- `probe-session.json` / `probe-events.txt`: 実モデルの単純な応答を確認する試験。429 Subscription quota exhausted。リセットは 2026-09-14T00:00:00Z（日本時間9月14日9時）。セッション解放済み。
- `live-workflow.json`: 本番のRust AgentSessionプロバイダーを使用した生成→編集→保存→再利用試験。約3.8秒でUpstream失敗、ツール結果0、cleanup Released。**実モデルでの成功を示す証拠ではない。**
- 通信統合テスト: 同一SSEセッションでpresent_ui→get_ui→present_ui→save_ui→search_ui→open_ui→通常の返答。SQLiteへの6件のツール結果、3件の表示通知、Deltaに制御フレームが含まれないことを確認。毎文字を別SSEイベントにしてフレーム分割を検証。
- 境界テスト: 部分フレーム、未登録ツール、余分なフィールド、後続文字列、サイズ制限、通常文の逐次表示、初回POSTの中断とタイムアウト。

別作業の未コミット差分にRustのfixture参照エラーとTypeScriptのnull型エラーがあったため、コミット240c747＋今回のSSE差分を `/tmp/saaa-sse-validation` に取り出してRust全体とbuildを検証した。別作業のファイルは変更していない。

利用枠回復後の再検証（合成入力・独立したインメモリDB、実際のモデル利用あり）:

```sh
SAAA_LIVE_SSE=1 SAAA_LIVE_SSE_EVIDENCE=/tmp/saaa-live-sse.json cargo test --manifest-path src-tauri/Cargo.toml live_muse_sse_ui_workflow --lib -- --ignored --nocapture
```

この試験では、生成・編集・保存・検索・再表示の完了を検査する。一般公開の基準となる新規20件＋編集20件×3回の成功率・修正率評価は未実施。
