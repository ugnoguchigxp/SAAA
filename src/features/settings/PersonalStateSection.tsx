import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { z } from "zod";
import { RemoteCleanupStatus } from "./RemoteCleanupStatus";
const snapshotSchema = z
  .object({
    enabled: z.boolean(),
    revision: z.number(),
    inputEpoch: z.number(),
    pendingCount: z.number(),
    pendingBytes: z.number(),
    contractReady: z.boolean(),
    contractReason: z.string().nullable(),
    items: z.array(
      z
        .object({
          id: z.string(),
          key: z.string(),
          status: z.string(),
          value: z.unknown(),
          source: z.array(z.object({ id: z.string() }).passthrough()),
        })
        .passthrough(),
    ),
    cleanup: z.array(
      z.object({ incarnation: z.string(), stage: z.string(), reason: z.string() }).passthrough(),
    ),
  })
  .passthrough();
const sourcePageSchema = z.object({
  sources: z.array(
    z.object({
      id: z.string(),
      version: z.number(),
      sequence: z.number(),
      preview: z.string(),
      bytes: z.number(),
    }),
  ),
  nextSequence: z.number(),
  hasMore: z.boolean(),
});
type SourcePage = z.infer<typeof sourcePageSchema>;
type Snapshot = z.infer<typeof snapshotSchema>;
export function PersonalStateSection() {
  const [snapshot, setSnapshot] = useState<Snapshot | null>(null);
  const [sourcePage, setSourcePage] = useState<SourcePage | null>(null);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  async function refresh() {
    try {
      setSnapshot(snapshotSchema.parse(await invoke<unknown>("personal_state_snapshot")));
      setSourcePage(
        sourcePageSchema.parse(await invoke<unknown>("personal_source_page", { afterSequence: 0 })),
      );
    } catch (e) {
      setError(String(e));
    }
  }
  useEffect(() => {
    void refresh();
  }, []);
  async function forget(sourceId: string) {
    setBusy(true);
    setError("");
    try {
      setSnapshot(
        snapshotSchema.parse(await invoke<unknown>("forget_personal_source", { sourceId })),
      );
      setSourcePage(null);
      await refresh();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }
  return (
    <section className="settings-section">
      <h3>会話の現在状態</h3>
      {snapshot?.enabled && (
        <p role="status">
          LARM管理メモリを使用中です。LLMはメモリ整合性を保証するLARM経路を使用し、接続設定のLLM選択・クラウド自動切替は適用されません。ASRとTTSは個別に選択できます。
        </p>
      )}
      <p>目的・制約・未決事項と、その根拠を確認できます。抽出は初期状態では停止しています。</p>
      <button type="button" onClick={() => void refresh()} disabled={busy}>
        更新
      </button>
      {error && <p role="alert">{error}</p>}
      {sourcePage && (
        <details>
          <summary>出典の原文を確認する</summary>
          <ul>
            {sourcePage.sources.map((source) => (
              <li key={source.sequence}>
                <p>{source.preview}</p>
                <button type="button" disabled={busy} onClick={() => void forget(source.id)}>
                  この原文と派生状態を忘れる
                </button>
              </li>
            ))}
          </ul>
          {sourcePage.hasMore && (
            <button
              type="button"
              onClick={() => {
                void invoke<unknown>("personal_source_page", {
                  afterSequence: sourcePage.nextSequence,
                })
                  .then((value) => setSourcePage(sourcePageSchema.parse(value)))
                  .catch((e) => setError(String(e)));
              }}
            >
              次の出典
            </button>
          )}
        </details>
      )}

      {snapshot && (
        <>
          <p>
            抽出: {snapshot.enabled ? "有効" : "停止中"} / 接続:{" "}
            {snapshot.contractReady ? "契約確認済み" : "認定待ち"}
          </p>
          <p>
            未反映 {snapshot.pendingCount}件（{snapshot.pendingBytes} bytes） / 状態版{" "}
            {snapshot.revision}
          </p>
          {!snapshot.contractReady && (
            <p>配送・消去・モデルの認定が完了するまで追加の推論を開始しません。</p>
          )}
          <ul>
            {snapshot.items.map((item) => (
              <li key={item.id}>
                {item.key}: {JSON.stringify(item.value)}（{item.status}）
                {item.source.map((source) => (
                  <button
                    type="button"
                    key={source.id}
                    disabled={busy}
                    onClick={() => void forget(source.id)}
                  >
                    この出典と派生状態を忘れる
                  </button>
                ))}
              </li>
            ))}
          </ul>
          <RemoteCleanupStatus operations={snapshot.remoteCleanup} />
          {snapshot.cleanup.some((item) => item.stage !== "complete") && (
            <p>遠隔の削除確認が残っています。確認が終わるまで削除完了とは表示しません。</p>
          )}
        </>
      )}
    </section>
  );
}
