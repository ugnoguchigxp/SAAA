import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { z } from "zod";

const schema = z.object({
  consolidationEnabled: z.boolean(),
  scopes: z.array(z.object({ scope: z.string(), name: z.string(), enabled: z.boolean() })),
});
export function EpisodeSyncSection() {
  const [scopes, setScopes] = useState<z.infer<typeof schema>["scopes"]>([]);
  const [consolidation, setConsolidation] = useState(false);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  useEffect(() => {
    void invoke("episode_sync_status")
      .then((value) => {
        const parsed = schema.parse(value);
        setScopes(parsed.scopes);
        setConsolidation(parsed.consolidationEnabled);
      })
      .catch((e: unknown) => setError(String(e)));
  }, []);
  async function setEnabled(scope: string, enabled: boolean) {
    setBusy(true);
    try {
      setScopes(schema.parse(await invoke("set_episode_sync_scope", { scope, enabled })).scopes);
      setError("");
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }
  return (
    <section className="settings-section">
      <h3>出来事の記憶</h3>
      <p>
        許可した範囲の会話をContextStillが読み取り、出来事の記憶に整理します。訂正・忘却した記録は回答に再利用しません。
      </p>
      {scopes
        .filter((s) => !s.scope.startsWith("request:"))
        .map((s) => (
          <label key={s.scope}>
            <input
              type="checkbox"
              checked={s.enabled}
              disabled={busy}
              onChange={(e) => void setEnabled(s.scope, e.currentTarget.checked)}
            />
            {s.scope.startsWith("user:") ? "本人との会話" : s.name}
          </label>
        ))}
      <label>
        <input
          type="checkbox"
          checked={consolidation}
          disabled={busy}
          onChange={(e) => {
            const enabled = e.currentTarget.checked;
            setBusy(true);
            void invoke("set_memory_consolidation", { enabled })
              .then((value) => {
                setConsolidation(schema.parse(value).consolidationEnabled);
                setError("");
              })
              .catch((e: unknown) => setError(String(e)))
              .finally(() => setBusy(false));
          }}
        />
        会話から傾向を整理する（試験運用・推測として保持）
      </label>
      {error && <p role="alert">{error}</p>}
    </section>
  );
}
