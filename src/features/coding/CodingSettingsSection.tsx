import { CodingConnectionFields } from "./CodingConnectionFields";
import { useEffect, useState } from "react";
import { codingApi, type CodingSettings } from "./api";
export function CodingSettingsSection() {
  const [settings, setSettings] = useState<CodingSettings | null>(null);
  const [notice, setNotice] = useState("");
  const [busy, setBusy] = useState(false);
  useEffect(() => {
    void codingApi
      .settings()
      .then(setSettings)
      .catch((e: unknown) => setNotice(String(e)));
  }, []);
  async function save(probe: boolean) {
    if (!settings) return;
    setBusy(true);
    setNotice("");
    try {
      await codingApi.save(settings);
      if (probe) {
        await codingApi.probe();
        setNotice("piの起動・モデル登録を確認しました。実際のモデル応答は実行時に確認します。");
      } else {
        setNotice("保存しました。");
      }
    } catch (e) {
      setNotice(String(e));
    } finally {
      setBusy(false);
    }
  }
  return (
    <section className="settings-section">
      <h3>piによる実装</h3>
      <p>
        選択したGitフォルダーで、会話から実装を依頼できます。信頼済みのローカル作業が対象です。作業フォルダー外への書き込みは隔離されません。
      </p>
      {settings && (
        <>
          <label>
            <input
              type="checkbox"
              checked={settings.enabled}
              onChange={(e) => setSettings({ ...settings, enabled: e.target.checked })}
            />
            実装ツールを有効にする
          </label>
          <CodingConnectionFields settings={settings} onChange={setSettings} />
          {(
            [
              ["executable", "pi実行ファイルの絶対パス"],
              ["provider", "pi provider"],
              ["model", "pi model"],
            ] as const
          ).map(([key, label]) => (
            <label key={key}>
              {label}
              <input
                value={settings[key]}
                onChange={(e) => setSettings({ ...settings, [key]: e.target.value })}
              />
            </label>
          ))}
          <p>
            検証対象: {settings.version} / {settings.profile}。
            {settings.profile === "codex-sdk-v1"
              ? "認証には既存のCodexログインを使用します。"
              : "認証にはpi側の設定を使用します。"}
          </p>
          <button disabled={busy} onClick={() => void save(false)}>
            実装設定を保存
          </button>
          <button disabled={busy} onClick={() => void save(true)}>
            保存して接続確認
          </button>
        </>
      )}
      {notice && <p role="status">{notice}</p>}
    </section>
  );
}
