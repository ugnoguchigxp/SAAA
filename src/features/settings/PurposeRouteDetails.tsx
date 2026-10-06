import { useEffect, useState } from "react";
import {
  candidatesFor,
  type PurposeBinding,
  type RegistrySnapshot,
  type RouteUsage,
} from "../../lib/serviceRegistry";

type Props = {
  snapshot: RegistrySnapshot;
  binding: PurposeBinding;
  usage?: RouteUsage;
  busy: boolean;
  save: (binding: PurposeBinding) => Promise<void>;
};

const STATUS = {
  accepted: "完了・結果を採用済み",
  sending: "通信を開始（完了は未確認）",
  "inference-completed": "推論が完了（回答の保存は未確認）",
  failed: "処理に失敗・中止",
};

export function PurposeRouteDetails({ snapshot, binding, usage, busy, save }: Props) {
  const [draft, setDraft] = useState(binding);
  useEffect(() => setDraft(binding), [binding]);
  const options = candidatesFor(snapshot, binding.purpose).filter(
    ({ connection, resource, unsupportedReason }) =>
      connection?.enabled &&
      resource.enabled &&
      !unsupportedReason &&
      connection.adapterKind !== "larm" &&
      resource.resourceId !== draft.primaryResourceId,
  );
  const primaryResource = snapshot.resources.find(
    (r) => r.resourceId === binding.primaryResourceId,
  );
  const primaryAdapter =
    snapshot.connections.find((c) => c.connectionId === primaryResource?.connectionId)
      ?.adapterKind ?? "";
  // LARM is only used at home, so its fallback is what runs when LARM cannot be reached.
  // A conversation on a direct model API may also fall back to another direct API.
  const canFallback =
    primaryAdapter === "larm" ||
    (binding.purpose === "conversation.respond" &&
      ["chat-completions", "anthropic-messages"].includes(primaryAdapter));
  const fallbackIsCloud = draft.fallbackResourceIds.some((id) => {
    const r = snapshot.resources.find((item) => item.resourceId === id);
    return (
      snapshot.connections.find((c) => c.connectionId === r?.connectionId)?.location === "cloud"
    );
  });
  const label = (id: string) => {
    const r = snapshot.resources.find((r) => r.resourceId === id);
    return `${snapshot.connections.find((c) => c.connectionId === r?.connectionId)?.label ?? id}${r?.model ? ` / ${r.model}` : ""}`;
  };
  return (
    <details className="settings-field">
      <summary>時間制限・送信許可・利用記録</summary>
      <label>
        <input
          type="checkbox"
          disabled={busy}
          checked={draft.cloudAllowed ?? true}
          onChange={(e) => setDraft({ ...draft, cloudAllowed: e.target.checked })}
        />
        この用途のクラウド送信を許可する
      </label>
      <label>
        処理全体の時間制限（秒）
        <input
          type="number"
          min={1}
          max={3600}
          disabled={busy}
          value={draft.timeoutMs / 1000}
          onChange={(e) => setDraft({ ...draft, timeoutMs: Number(e.target.value) * 1000 })}
        />
      </label>
      <label>
        1回の通信の時間制限（秒、空欄なら全体の残り時間）
        <input
          type="number"
          min={1}
          max={draft.timeoutMs / 1000}
          disabled={busy}
          value={draft.attemptTimeoutMs === undefined ? "" : draft.attemptTimeoutMs / 1000}
          onChange={(e) =>
            setDraft({
              ...draft,
              attemptTimeoutMs: e.target.value === "" ? undefined : Number(e.target.value) * 1000,
            })
          }
        />
      </label>
      {canFallback && (
        <>
          <p>
            {primaryAdapter === "larm"
              ? "LARMに接続できない時（外出時）は、代替先を下の順で使います。自宅に戻ると、次の依頼からLARMに戻ります。実行中の依頼は切り替えません。"
              : "代替先は下の順で使います。最初の通信が接続失敗または混雑で拒否された場合に限ります。回答の表示やツール実行後は切り替えません。"}
          </p>
          {fallbackIsCloud && draft.cloudAllowed === false && (
            <p role="alert">
              代替先がクラウドですが、この用途のクラウド送信が許可されていません。上の「この用途のクラウド送信を許可する」をオンにしないと、外出時は使えません。
            </p>
          )}
          {draft.fallbackResourceIds.map((id) => (
            <div key={id}>
              {label(id)}{" "}
              <button
                disabled={busy}
                onClick={() =>
                  setDraft({
                    ...draft,
                    fallbackResourceIds: draft.fallbackResourceIds.filter((r) => r !== id),
                  })
                }
              >
                代替先から外す
              </button>
            </div>
          ))}
          <label>
            代替先を追加
            <select
              disabled={busy}
              value=""
              onChange={(e) =>
                e.target.value &&
                setDraft({
                  ...draft,
                  fallbackResourceIds: [...draft.fallbackResourceIds, e.target.value],
                })
              }
            >
              <option value="">選択してください</option>
              {options
                .filter(({ resource }) => !draft.fallbackResourceIds.includes(resource.resourceId))
                .map(({ resource }) => (
                  <option key={resource.resourceId} value={resource.resourceId}>
                    {label(resource.resourceId)}
                  </option>
                ))}
            </select>
          </label>
        </>
      )}
      <button
        disabled={
          busy ||
          !Number.isFinite(draft.timeoutMs) ||
          draft.timeoutMs < 1000 ||
          draft.timeoutMs > 3600000 ||
          (draft.attemptTimeoutMs !== undefined &&
            (!Number.isFinite(draft.attemptTimeoutMs) ||
              draft.attemptTimeoutMs < 1000 ||
              draft.attemptTimeoutMs > draft.timeoutMs))
        }
        onClick={() => void save(draft)}
      >
        用途の詳細を保存
      </button>
      <p>
        送信許可の取消しは実行中の依頼にも反映します。時間制限と代替先は次の依頼から反映します。
      </p>
      {usage ? (
        <p>
          直近の利用先: {usage.connectionLabel ?? usage.connectionId}
          {usage.model ? ` / ${usage.model}` : ""}
          {usage.selection === "local-unreachable" ? "（LARMに接続できないため代替先）" : ""} —{" "}
          {STATUS[usage.status]}（{new Date(usage.occurredAt).toLocaleString()}）。
          {usage.matchesCurrentSettings
            ? "現在の設定による記録です。"
            : "変更前の設定による記録です。現在の設定は未実行です。"}
        </p>
      ) : (
        <p>利用記録はありません。登録や動作確認だけでは実行済みになりません。</p>
      )}
    </details>
  );
}
