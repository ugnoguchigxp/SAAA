import { PurposeRouteDetails } from "./PurposeRouteDetails";
import { RegisteredServices } from "./RegisteredServices";
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

const HARNESS_SUFFIX = {
  "conversation.respond": "llm",
  "voice.transcribe": "asr",
  "voice.speak": "tts",
  "media.image.generate": "image",
  "media.music.generate": "music",
} as const;

const EMPTY_SERVICE: NewChatService = {
  adapterKind: "chat-completions",
  detail: "",
  label: "",
  endpoint: "",
  model: "",
  authentication: "api-key",
  location: "cloud",
};

/** Self-contained: registry changes save immediately and never touch the legacy settings draft. */
export function PurposeRoutesSection({ onOpenCoding }: { onOpenCoding?: () => void } = {}) {
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
      setView(saved);
      if (form.authentication === "api-key") {
        await setServiceConnectionSecret(added.connectionId, apiKey);
        const stored = await getServiceConnectionSecretState(added.connectionId);
        if (stored.state !== "configured") throw new Error("APIキーを保存できませんでした");
      }
      saved = await saveServiceRegistry(
        setConnectionEnabled(saved.snapshot, added.connectionId, true),
        saved.revision,
      );
      setView(saved);
      setForm(EMPTY_SERVICE);
      setApiKey("");
      setNotice("サービスを登録しました。用途から選ぶと次の新しい依頼から使います。");
    });
  }

  async function choose(purpose: PurposeId, resourceId: string | null) {
    await run(async () => {
      const saved = await saveServiceRegistry(
        assignPrimary(snapshot, purpose, resourceId),
        view!.revision,
      );
      setView(saved);
      setNotice("保存しました。実行中の依頼は変わらず、新しい依頼から反映されます。");
    });
  }

  const canRegister =
    form.label.trim() !== "" &&
    form.endpoint.trim() !== "" &&
    form.model.trim() !== "" &&
    (form.adapterKind !== "http-tts" || !!form.detail?.trim()) &&
    (form.authentication === "none" || apiKey.trim() !== "");

  return (
    <section className="settings-section" aria-label="用途別の接続先">
      <p>
        用途ごとに使うサービスを選びます。選んだ内容は保存後の新しい依頼から使われ、
        代替先を設定した場合だけ、最初の通信が失敗した際に切り替えます。
      </p>
      <RegisteredServices
        probes={view.probes}
        reload={reload}
        snapshot={snapshot}
        busy={busy}
        run={run}
        save={async (next) => {
          setView(await saveServiceRegistry(next, view.revision));
        }}
      />
      {!view.persisted && (
        <p role="note">
          まだ用途別の設定は保存されていません。表示は従来の設定から作った案で、
          選択して保存するまで従来の動作のままです。
        </p>
      )}
      {PURPOSES.map((purpose) => {
        const binding = snapshot.bindings.find((item) => item.purpose === purpose.id);
        const options = candidatesFor(snapshot, purpose.id);
        const needsReview = binding?.review === "needs-review";
        const stored = options.find(
          (item) => item.resource.resourceId === binding?.storedPrimaryResourceId,
        );
        return (
          <div key={purpose.id} className="settings-field">
            <label>
              {purpose.label}
              <select
                disabled={busy}
                value={needsReview || !binding?.enabled ? "" : (binding.primaryResourceId ?? "")}
                onChange={(event) => void choose(purpose.id, event.target.value || null)}
              >
                <option value="" disabled={purpose.id.startsWith("voice.")}>
                  {needsReview
                    ? "選択して適用"
                    : !purpose.id.startsWith("voice.")
                      ? "使わない"
                      : "選択してください"}
                </option>
                {options.map(({ resource, connection, unsupportedReason }) => (
                  <option
                    key={resource.resourceId}
                    value={resource.resourceId}
                    disabled={!resource.enabled || !connection?.enabled || !!unsupportedReason}
                  >
                    {connection?.label ?? resource.connectionId}
                    {resource.model ? ` / ${resource.model}` : ""}
                    {connection?.location === "cloud" ? "（クラウド）" : ""}
                    {unsupportedReason ? ` — ${unsupportedReason}` : ""}
                  </option>
                ))}
              </select>
            </label>
            {binding && !needsReview && (
              <PurposeRouteDetails
                snapshot={snapshot}
                binding={binding}
                busy={busy}
                usage={view.latestUsage?.find((item) => item.purpose === purpose.id)}
                save={(nextBinding) =>
                  run(async () => {
                    setView(
                      await saveServiceRegistry(
                        {
                          ...snapshot,
                          bindings: snapshot.bindings.map((b) =>
                            b.purpose === purpose.id ? nextBinding : b,
                          ),
                        },
                        view.revision,
                      ),
                    );
                    setNotice("用途の詳細を保存しました。");
                  })
                }
              />
            )}
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
      <button
        disabled={
          busy ||
          !PURPOSES.every((p) =>
            snapshot.resources.some((r) => r.resourceId === `res:harness-${HARNESS_SUFFIX[p.id]}`),
          )
        }
        onClick={() =>
          void run(async () => {
            let next = snapshot;
            for (const p of PURPOSES)
              next = assignPrimary(next, p.id, `res:harness-${HARNESS_SUFFIX[p.id]}`);
            setView(await saveServiceRegistry(next, view.revision));
            setNotice("全用途にLARMを割り当てました。次の依頼から反映します。");
          })
        }
      >
        全用途にLARMを割り当てる
      </button>
      <details>
        <summary>記憶・索引・実装・外部ツールの接続</summary>
        <p>
          記憶とWorldの保存元はこのアプリが管理します。背景推論は、データの送信・削除・実行場所を確認した専用契約が必要で、ここで登録した会話APIへは切り替わりません。
        </p>
        <p>
          検索用の埋め込み・再順位付けは現在のローカル索引を使用します。モデル変更時には索引を別の版へ作り直す必要があるため、用途別画面からの変更にはまだ対応していません。
        </p>
        <p>
          コード実装は実装設定へ委譲します。作業フォルダー、実行権限、接続方法をそこで指定してください。会話モデルの登録ではコード実行を有効にしません。外部検索ツールは既存のツール登録と権限を使用します。
        </p>
        {onOpenCoding && (
          <button disabled={busy} onClick={onOpenCoding}>
            実装方法の設定を開く
          </button>
        )}
      </details>
      <h3>サービスを登録</h3>
      <p>
        HTTP音声の切り替えは次の発話から反映します。ライブASRとの通信方式の切り替えは、マイクを開始し直すと反映します。
      </p>
      <div className="settings-field">
        <label>
          サービスの形式
          <select
            disabled={busy}
            value={form.adapterKind ?? "chat-completions"}
            onChange={(e) =>
              setForm({
                ...form,
                adapterKind: e.target.value as NewChatService["adapterKind"],
                detail: "",
              })
            }
          >
            <option value="chat-completions">会話（Chat Completions形式）</option>
            <option value="anthropic-messages">会話（Anthropic Messages形式、完了後に表示）</option>
            <option value="replicate-media">画像・音楽（Replicate Predictions形式）</option>
            <option value="http-asr">文字起こし（HTTP、WAV送信）</option>
            <option value="http-tts">読み上げ（HTTP、WAV受信）</option>
          </select>
        </label>
        {form.adapterKind === "replicate-media" && (
          <>
            <label>
              生成するもの{" "}
              <select
                value={form.mediaKind ?? "image-generation"}
                onChange={(e) =>
                  setForm({ ...form, mediaKind: e.target.value as NewChatService["mediaKind"] })
                }
              >
                <option value="image-generation">画像</option>
                <option value="music-generation">音楽</option>
              </select>
            </label>
            <label>
              モデル固有の入力設定（JSON、任意）{" "}
              <textarea
                value={form.detail ?? ""}
                onChange={(e) => setForm({ ...form, detail: e.target.value })}
                placeholder='{"width": 1024}'
              />
            </label>
            <p>
              接続先は https://api.replicate.com/v1、モデルは owner/name。版を固定する場合は
              owner/name:版ID を指定します。prompt は作成画面で入力します。
            </p>
          </>
        )}
        <label>
          表示名
          <input
            value={form.label}
            onChange={(event) => setForm({ ...form, label: event.target.value })}
          />
        </label>
        <label>
          エンドポイント（例: https://api.example.com/v1）
          <input
            value={form.endpoint}
            onChange={(event) => setForm({ ...form, endpoint: event.target.value })}
          />
        </label>
        <label>
          モデル
          <input
            value={form.model}
            onChange={(event) => setForm({ ...form, model: event.target.value })}
          />
        </label>
        {["http-asr", "http-tts"].includes(form.adapterKind ?? "") && (
          <label>
            {form.adapterKind === "http-asr" ? "言語（空欄なら自動）" : "声の名前"}
            <input
              disabled={busy}
              value={form.detail ?? ""}
              onChange={(e) => setForm({ ...form, detail: e.target.value })}
            />
          </label>
        )}
        <label>
          送信先
          <select
            value={form.location}
            disabled={busy}
            onChange={(event) =>
              setForm({ ...form, location: event.target.value as "local" | "cloud" })
            }
          >
            <option value="cloud">クラウド</option>
            <option value="local">ローカル</option>
          </select>
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
        <button disabled={busy || !canRegister} onClick={() => void registerService()}>
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
