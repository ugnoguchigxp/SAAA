import { useEffect, useState } from "react";
import {
  probeServiceResource,
  type RegistrySnapshot,
  type RegistryView,
  type ServiceConnection,
  type ServiceResource,
} from "../../lib/serviceRegistry";

type Props = {
  connection: ServiceConnection;
  snapshot: RegistrySnapshot;
  probes?: RegistryView["probes"];
  busy: boolean;
  save: (next: RegistrySnapshot) => Promise<void>;
  run: (action: () => Promise<void>) => Promise<void>;
  reload: () => Promise<void>;
};

export function ServiceResourcesEditor({
  connection,
  snapshot,
  probes,
  busy,
  save,
  run,
  reload,
}: Props) {
  const [endpoint, setEndpoint] = useState(connection.endpoint);
  const [model, setModel] = useState("");
  const [mediaKind, setMediaKind] = useState<"image-generation" | "music-generation">(
    "image-generation",
  );
  useEffect(() => setEndpoint(connection.endpoint), [connection.endpoint]);
  const resources = snapshot.resources.filter((r) => r.connectionId === connection.connectionId);
  return (
    <details>
      <summary>接続先・モデル・動作確認</summary>
      <label>
        接続先{" "}
        <input disabled={busy} value={endpoint} onChange={(e) => setEndpoint(e.target.value)} />
      </label>
      <button
        disabled={busy || !endpoint.trim()}
        onClick={() =>
          void run(() =>
            save({
              ...snapshot,
              connections: snapshot.connections.map((c) =>
                c.connectionId === connection.connectionId
                  ? { ...c, endpoint: endpoint.trim().replace(/\/+$/, "") }
                  : c,
              ),
            }),
          )
        }
      >
        接続先を保存
      </button>
      {resources.map((resource) => (
        <ResourceEditor
          key={resource.resourceId}
          {...{ resource, snapshot, probes, busy, save, run, reload }}
        />
      ))}
      {connection.adapterKind === "replicate-media" && (
        <label>
          追加するモデルの用途{" "}
          <select
            value={mediaKind}
            onChange={(e) => setMediaKind(e.target.value as typeof mediaKind)}
          >
            <option value="image-generation">画像</option>
            <option value="music-generation">音楽</option>
          </select>
        </label>
      )}
      <label>
        このサービスへモデルを追加{" "}
        <input disabled={busy} value={model} onChange={(e) => setModel(e.target.value)} />
      </label>
      <button
        disabled={busy || !model.trim()}
        onClick={() =>
          void run(async () => {
            const taken = new Set(snapshot.resources.map((r) => r.resourceId));
            let suffix = 1;
            let id = `res:svc-${connection.connectionId.slice("conn:svc-".length)}-model-${suffix}`;
            while (taken.has(id))
              id = `res:svc-${connection.connectionId.slice("conn:svc-".length)}-model-${++suffix}`;
            await save({
              ...snapshot,
              resources: [
                ...snapshot.resources,
                {
                  resourceId: id,
                  connectionId: connection.connectionId,
                  capability:
                    connection.adapterKind === "replicate-media"
                      ? mediaKind
                      : connection.adapterKind === "http-asr"
                        ? "transcription"
                        : connection.adapterKind === "http-tts"
                          ? "speech"
                          : "text-generation",
                  detail: resources[0]?.detail,
                  model: model.trim(),
                  enabled: connection.enabled,
                },
              ],
            });
            setModel("");
          })
        }
      >
        モデルを追加
      </button>
      <p>モデルを追加しても用途は切り替わりません。APIキーは同じサービス内で共有します。</p>
    </details>
  );
}

type ResourceProps = Omit<Props, "connection"> & { resource: ServiceResource };
function ResourceEditor({ resource, snapshot, probes, busy, save, run, reload }: ResourceProps) {
  const [model, setModel] = useState(resource.model);
  const [detail, setDetail] = useState(resource.detail ?? "");
  const [notice, setNotice] = useState("");
  const [models, setModels] = useState<string[]>([]);
  useEffect(() => setModel(resource.model), [resource.model]);
  async function probe(kind: "models" | "generation") {
    await run(async () => {
      setNotice("");
      try {
        const result = await probeServiceResource(resource.resourceId, kind);
        setModels(result.models ?? []);
        setNotice(result.message);
      } finally {
        await reload();
      }
    });
  }
  const isChat =
    snapshot.connections.find((c) => c.connectionId === resource.connectionId)?.adapterKind ===
    "chat-completions";
  const [requestOptions, setRequestOptions] = useState<
    NonNullable<ServiceResource["requestOptions"]>
  >(
    resource.requestOptions ?? {
      tokenLimit: "auto",
      reasoning: "auto",
      tools: false,
      streaming: true,
      thinking: "auto",
    },
  );
  const used = snapshot.bindings.some((b) =>
    [b.primaryResourceId, b.storedPrimaryResourceId, ...b.fallbackResourceIds].includes(
      resource.resourceId,
    ),
  );
  const records = probes?.filter((p) => p.resourceId === resource.resourceId);
  return (
    <div className="settings-field">
      {isChat && (
        <details>
          <summary>会話APIの詳細設定</summary>
          <label>
            出力上限の指定方法{" "}
            <select
              value={requestOptions.tokenLimit}
              onChange={(e) =>
                setRequestOptions({
                  ...requestOptions,
                  tokenLimit: e.target.value as typeof requestOptions.tokenLimit,
                })
              }
            >
              <option value="auto">自動</option>
              <option value="legacy">max_tokens</option>
              <option value="completion">max_completion_tokens</option>
            </select>
          </label>
          <label>
            推論設定{" "}
            <select
              value={requestOptions.reasoning}
              onChange={(e) =>
                setRequestOptions({
                  ...requestOptions,
                  reasoning: e.target.value as typeof requestOptions.reasoning,
                })
              }
            >
              <option value="auto">自動</option>
              <option value="supported">対応あり</option>
              <option value="unsupported">対応なし</option>
            </select>
          </label>
          <label>
            <input
              type="checkbox"
              checked={requestOptions.streaming}
              onChange={(e) =>
                setRequestOptions({ ...requestOptions, streaming: e.target.checked })
              }
            />{" "}
            生成中に表示する
          </label>
          <label>
            thinking{" "}
            <select
              value={requestOptions.thinking ?? "auto"}
              onChange={(e) =>
                setRequestOptions({
                  ...requestOptions,
                  thinking: e.target.value as "auto" | "enabled" | "disabled",
                })
              }
            >
              <option value="auto">既定値</option>
              <option value="enabled">有効</option>
              <option value="disabled">無効</option>
            </select>
          </label>
          <button
            disabled={busy}
            onClick={() =>
              void run(() =>
                save({
                  ...snapshot,
                  resources: snapshot.resources.map((r) =>
                    r.resourceId === resource.resourceId ? { ...r, requestOptions } : r,
                  ),
                }),
              )
            }
          >
            会話APIの詳細を保存
          </button>
        </details>
      )}
      {resource.capability !== "text-generation" && (
        <label>
          {resource.capability.endsWith("-generation")
            ? "モデル固有の入力設定（JSON）"
            : resource.capability === "speech"
              ? "声の名前"
              : "言語（空欄なら自動）"}{" "}
          <input disabled={busy} value={detail} onChange={(e) => setDetail(e.target.value)} />
        </label>
      )}
      <label>
        モデル <input disabled={busy} value={model} onChange={(e) => setModel(e.target.value)} />
      </label>
      <button
        disabled={busy || !model.trim()}
        onClick={() =>
          void run(() =>
            save({
              ...snapshot,
              resources: snapshot.resources.map((r) =>
                r.resourceId === resource.resourceId
                  ? {
                      ...r,
                      model: model.trim(),
                      ...(resource.capability !== "text-generation" ? { detail } : {}),
                    }
                  : r,
              ),
            }),
          )
        }
      >
        モデルを保存
      </button>
      <button
        disabled={busy || used}
        onClick={() =>
          void run(() =>
            save({
              ...snapshot,
              resources: snapshot.resources.filter((r) => r.resourceId !== resource.resourceId),
            }),
          )
        }
      >
        モデルを削除
      </button>
      {resource.capability !== "text-generation" && (
        <p>
          音声・画像・音楽の動作確認は実際の入力と出力で確認してください。固定文の確認は会話モデル用です。
        </p>
      )}
      <p>
        動作確認はこの接続先に送信します。応答確認では固定文「Reply with
        OK.」だけを送り、出力上限は64です。
      </p>
      <button
        disabled={busy || resource.capability !== "text-generation"}
        onClick={() => void probe("models")}
      >
        モデル一覧を取得
      </button>
      <button
        disabled={busy || resource.capability !== "text-generation"}
        onClick={() => void probe("generation")}
      >
        固定文で応答を確認
      </button>
      {models.length > 0 && (
        <label>
          取得したモデル{" "}
          <select
            value=""
            disabled={busy}
            onChange={(e) => e.target.value && setModel(e.target.value)}
          >
            <option value="">選択して編集欄へ反映</option>
            {models.map((id) => (
              <option key={id} value={id}>
                {id}
              </option>
            ))}
          </select>
        </label>
      )}
      {notice && <p role="status">{notice}</p>}
      {records?.map((p) => (
        <p key={p.kind}>
          {p.kind === "models" ? "モデル一覧" : "固定文の応答"}:{" "}
          {p.matchesCurrentSettings
            ? p.outcome === "success"
              ? "確認済み"
              : "失敗"
            : "設定変更により再確認が必要"}
          （{new Date(p.occurredAt).toLocaleString()}）
        </p>
      ))}
    </div>
  );
}
