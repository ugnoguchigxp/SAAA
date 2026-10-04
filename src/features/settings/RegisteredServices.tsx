import { ServiceResourcesEditor } from "./ServiceResourcesEditor";
import { useState } from "react";
import {
  getServiceConnectionSecretState,
  purposesUsing,
  setConnectionEnabled,
  setServiceConnectionSecret,
  type RegistrySnapshot,
  type ServiceConnection,
  type RegistryView,
} from "../../lib/serviceRegistry";

type Props = {
  snapshot: RegistrySnapshot;
  probes?: RegistryView["probes"];
  reload: () => Promise<void>;
  busy: boolean;
  save: (next: RegistrySnapshot) => Promise<void>;
  run: (action: () => Promise<void>) => Promise<void>;
};

/** Drafts survive failed registration and app restarts; the backend owns their state. */
export function RegisteredServices({ snapshot, probes, reload, busy, save, run }: Props) {
  const [keys, setKeys] = useState<Record<string, string>>({});
  async function enable(connection: ServiceConnection) {
    await run(async () => {
      if (connection.authentication === "api-key") {
        const key = keys[connection.connectionId];
        if (key) {
          await setServiceConnectionSecret(connection.connectionId, key);
          setKeys((values) => ({ ...values, [connection.connectionId]: "" }));
        }
        const state = await getServiceConnectionSecretState(connection.connectionId);
        if (state.state !== "configured")
          throw new Error("APIキーを入力してから有効にしてください");
      }
      await save(setConnectionEnabled(snapshot, connection.connectionId, true));
    });
  }
  const services = snapshot.connections.filter((c) => c.connectionId.startsWith("conn:svc-"));
  if (services.length === 0) return null;
  return (
    <div className="settings-section" aria-label="登録したサービス">
      <h3>登録したサービス</h3>
      {services.map((connection) => {
        const uses = purposesUsing(snapshot, connection.connectionId);
        return (
          <div className="settings-field" key={connection.connectionId}>
            <strong>{connection.label}</strong>
            <p>
              {connection.endpoint} — {connection.enabled ? "有効" : "無効・登録を再開できます"}
            </p>
            {!connection.enabled && connection.authentication === "api-key" && (
              <label>
                APIキー（保存済みなら再入力は不要）
                <input
                  type="password"
                  autoComplete="off"
                  disabled={busy}
                  value={keys[connection.connectionId] ?? ""}
                  onChange={(event) =>
                    setKeys({ ...keys, [connection.connectionId]: event.target.value })
                  }
                />
              </label>
            )}
            <button
              disabled={busy}
              onClick={() =>
                void (connection.enabled
                  ? run(() => save(setConnectionEnabled(snapshot, connection.connectionId, false)))
                  : enable(connection))
              }
            >
              {connection.enabled ? "無効にする" : "登録を再開・有効にする"}
            </button>
            <button
              disabled={busy || uses.length > 0}
              onClick={() =>
                void run(() =>
                  save({
                    ...snapshot,
                    connections: snapshot.connections.filter(
                      (c) => c.connectionId !== connection.connectionId,
                    ),
                    resources: snapshot.resources.filter(
                      (r) => r.connectionId !== connection.connectionId,
                    ),
                  }),
                )
              }
            >
              削除
            </button>
            <ServiceResourcesEditor
              {...{ connection, snapshot, probes, reload, busy, save, run }}
            />
            {uses.length > 0 && (
              <p>利用中: {uses.join("、")}。削除するには用途の接続先を変更してください。</p>
            )}
          </div>
        );
      })}
    </div>
  );
}
