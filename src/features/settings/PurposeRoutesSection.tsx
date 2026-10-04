import { useEffect, useState } from "react";
import {
  PURPOSES,
  addChatService,
  assignPrimary,
  candidatesFor,
  getServiceConnectionSecretState,
  getServiceRegistry,
  saveServiceRegistry,
  setConnectionEnabled,
  setServiceConnectionSecret,
  type NewChatService,
  type PurposeId,
  type RegistryView,
} from "../../lib/serviceRegistry";

const EMPTY_SERVICE: NewChatService = {
  label: "",
  endpoint: "",
  model: "",
  authentication: "api-key",
  location: "cloud",
};

/** Self-contained: registry changes save immediately and never touch the legacy settings draft. */
export function PurposeRoutesSection() {
  const [view, setView] = useState<RegistryView | null>(null);
  const [notice, setNotice] = useState("");
  const [busy, setBusy] = useState(false);
  const [form, setForm] = useState<NewChatService>(EMPTY_SERVICE);
  const [apiKey, setApiKey] = useState("");

  async function reload() {
    try {
      setView(await getServiceRegistry());
    } catch (error) {
      setNotice(String(error));
    }
  }
  useEffect(() => {
    void reload();
  }, []);

  async function run(action: () => Promise<void>) {
    setBusy(true);
    setNotice("");
    try {
      await action();
    } catch (error) {
      setNotice(`${String(error)}（競合した場合は再読込して確認してください）`);
    } finally {
      setBusy(false);
    }
  }

  if (!view) {
    return (
      <section className="settings-section" aria-label="用途別の接続先">
        <p role="status">{notice || "読み込み中…"}</p>
      </section>
    );
  }
  const { snapshot } = view;

  async function registerService() {
    await run(async () => {
      const added = addChatService(snapshot, form);
      // 1) save as a disabled draft, 2) store the key, 3) enable. Each step can resume.
      let saved = await saveServiceRegistry(added.snapshot, view!.revision);
      if (form.authentication === "api-key") {
        await setServiceConnectionSecret(added.connectionId, apiKey);
        const stored = await getServiceConnectionSecretState(
          added.connectionId,
        );
        if (stored.state !== "configured")
          throw new Error("APIキーを保存できませんでした");
      }
      saved = await saveServiceRegistry(
        setConnectionEnabled(saved.snapshot, added.connectionId, true),
        saved.revision,
      );
      setView(saved);
      setForm(EMPTY_SERVICE);
      setApiKey("");
      setNotice(
        "サービスを登録しました。用途から選ぶと次の新しい依頼から使います。",
      );
    });
  }

  async function choose(purpose: PurposeId, resourceId: string | null) {
    await run(async () => {
      const saved = await saveServiceRegistry(
        assignPrimary(snapshot, purpose, resourceId),
        view!.revision,
      );
      setView(saved);
      setNotice(
        "保存しました。実行中の依頼は変わらず、新しい依頼から反映されます。",
      );
    });
  }

  const canRegister =
    form.label.trim() !== "" &&
    form.endpoint.trim() !== "" &&
    form.model.trim() !== "" &&
    (form.authentication === "none" || apiKey.trim() !== "");

  return (
    <section className="settings-section" aria-label="用途別の接続先">
      <p>
        用途ごとに使うサービスを選びます。選んだ内容は保存後の新しい依頼から使われ、
        設定した接続先が使えない場合に別のサービスへ自動で切り替えることはありません。
      </p>
      {!view.persisted && (
        <p role="note">
          まだ用途別の設定は保存されていません。表示は従来の設定から作った案で、
          選択して保存するまで従来の動作のままです。
        </p>
      )}
      {PURPOSES.map((purpose) => {
        const binding = snapshot.bindings.find(
          (item) => item.purpose === purpose.id,
        );
        const options = candidatesFor(snapshot, purpose.id);
        const needsReview = binding?.review === "needs-review";
        const stored = options.find(
          (item) =>
            item.resource.resourceId === binding?.storedPrimaryResourceId,
        );
        return (
          <div key={purpose.id} className="settings-field">
            <label>
              {purpose.label}
              <select
                disabled={busy}
                value={
                  needsReview || !binding?.enabled
                    ? ""
                    : (binding.primaryResourceId ?? "")
                }
                onChange={(event) =>
                  void choose(purpose.id, event.target.value || null)
                }
              >
                <option value="">
                  {needsReview ? "選択して適用" : "使わない"}
                </option>
                {options.map(({ resource, connection }) => (
                  <option
                    key={resource.resourceId}
                    value={resource.resourceId}
                    disabled={!resource.enabled || !connection?.enabled}
                  >
                    {connection?.label ?? resource.connectionId}
                    {resource.model ? ` / ${resource.model}` : ""}
                    {connection?.location === "cloud" ? "（クラウド）" : ""}
                  </option>
                ))}
              </select>
            </label>
            {needsReview && (
              <p role="note">
                保存済みの選択（
                {stored?.connection?.label ?? binding?.storedPrimaryResourceId}
                ）は現在の実行方法と異なります。選んで適用するまで、従来の方法で動作します。
              </p>
            )}
          </div>
        );
      })}
      <h3>クラウドの会話サービスを登録</h3>
      <div className="settings-field">
        <label>
          表示名
          <input
            value={form.label}
            onChange={(event) =>
              setForm({ ...form, label: event.target.value })
            }
          />
        </label>
        <label>
          エンドポイント（Chat Completions形式、例: https://api.example.com/v1）
          <input
            value={form.endpoint}
            onChange={(event) =>
              setForm({ ...form, endpoint: event.target.value })
            }
          />
        </label>
        <label>
          モデル
          <input
            value={form.model}
            onChange={(event) =>
              setForm({ ...form, model: event.target.value })
            }
          />
        </label>
        <label>
          認証
          <select
            value={form.authentication}
            onChange={(event) =>
              setForm({
                ...form,
                authentication: event.target.value as "none" | "api-key",
              })
            }
          >
            <option value="api-key">APIキー</option>
            <option value="none">なし</option>
          </select>
        </label>
        {form.authentication === "api-key" && (
          <label>
            APIキー
            <input
              type="password"
              autoComplete="off"
              value={apiKey}
              onChange={(event) => setApiKey(event.target.value)}
            />
          </label>
        )}
        <button
          disabled={busy || !canRegister}
          onClick={() => void registerService()}
        >
          登録
        </button>
      </div>
      {notice && <p role="status">{notice}</p>}
      <button disabled={busy} onClick={() => void reload()}>
        再読込
      </button>
    </section>
  );
}
