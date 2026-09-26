# 回答Runtime削除実装のD0基準

2026-09-26。削除実装開始時の読み取り専用確認。

- HEAD: `a809d94981bcf729f4c985e584ee4ad0011edeb1`。計画時のHEADからの差分は計画・提案・inventoryの3ファイルのみ。作業ツリーはクリーンだった。
- inventory 146件はすべて存在し、記録済みSHA-256と一致した。凍結対象のpath・hashは未変更。
- `bun run freeze:check` は成功した。
- 変更前の `bun run check` は `size:check` で失敗した。既存のratchet超過、`ChatPage.tsx` と `ambient-voice-session.test.tsx` のhard budget超過、`useLarmConnectionStatus.ts` と `ambient-barge-in.test.ts` のbaseline未登録を含む。後続のbuild・品質・型・Rust・testには到達していない。
- `crates/saaa-conversation-core` は存在せず、旧通常会話のIPC、Butler継続、LFM受付は登録されたまま。したがってD2・D3の新経路合格を確認できず、D4の旧入口・本体削除には着手できない。

DBの隔離backup、稼働sessionのdrain、実マイク・実Provider・実TTSの受入は未実施。削除の前に、計画のD1〜D3をbuild可能な単位で実装・検証する必要がある。本番DB、設定、資格情報には触れていない。
