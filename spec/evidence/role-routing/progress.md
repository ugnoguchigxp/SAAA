# Role Routing 実装進捗

## R1 基盤（進行中）

| カード | 実装 | 検証 |
| --- | --- | --- |
| RR-01 | `role_routing` module、設定契約、決定的なrules選択を追加 | Rust unit testは並行変更中の生成機能の未解決importによりcrate compile前に停止 |
| RR-02 | SQLiteへpolicy/root/input/decision/step/output/event/feedback ledgerを追加 | migrationはidempotent unit testを追加。crate全体のcompile blockerが解消後に実行する |
| RR-03 | `routing.roles/default` を無効既定で追加。保存時にimmutable policy snapshotを採番 | `bun test tests/settings-regressions.test.ts tests/settings-review.test.ts`（11 pass） |
| RR-05〜09 | enabled時は`respond` recipeのprovider actorを既存会話経路へ適用。disabled時は旧経路を維持 | `enabled_direct_recipe_overrides_the_legacy_conversation_route`（pass） |

`npm run typecheck` は本変更の型不整合を解消済み。現在は既存の
`src/features/voice/useAmbientVoiceSession.ts` にある `meetingBlocked` 未指定で停止する。
