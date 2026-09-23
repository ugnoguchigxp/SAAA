import { CodingConnectionFields } from "./CodingConnectionFields";
import { useEffect, useState } from "react";
import { codingApi, type CodingSettings } from "./api";
import "./CodingSettingsSection.css";

export function CodingSettingsSection() {
  const [settings, setSettings] = useState<CodingSettings | null>(null);
  const [savedSettings, setSavedSettings] = useState<CodingSettings | null>(null);
  const [notice, setNotice] = useState("");
  const [busy, setBusy] = useState(false);
  useEffect(() => {
    void codingApi
      .settings()
      .then((loaded) => {
        setSettings(loaded);
        setSavedSettings(loaded);
      })
      .catch((error: unknown) => setNotice(String(error)));
  }, []);

  async function save(probe: boolean) {
    if (!settings) return;
    setBusy(true);
    setNotice("");
    try {
      await codingApi.save(settings);
      setSavedSettings(settings);
      if (probe) {
        await codingApi.probe();
        setNotice(
          settings.implementationMethod === "codex-sdk"
            ? "Codexの起動を確認しました。モデル応答は実行時に確認します。"
            : "Piの起動・モデル登録を確認しました。モデル応答は実行時に確認します。",
        );
      } else {
        setNotice("実装方法の設定を保存しました。");
      }
    } catch (error) {
      setNotice(String(error));
    } finally {
      setBusy(false);
    }
  }

  if (!settings)
    return (
      <section className="settings-section">
        <h3>実装方法</h3>
        {notice && <p role="status">{notice}</p>}
      </section>
    );
  const method = settings.implementationMethod === "codex-sdk" ? "codex-sdk" : "pi";
  const savedMethod =
    savedSettings?.implementationMethod === "codex-sdk"
      ? "Codex SDK"
      : savedSettings?.profile.includes("codex-sdk")
        ? "Pi（Codex SDK拡張）"
        : "Pi";
  const dirty =
    savedSettings !== null && JSON.stringify(settings) !== JSON.stringify(savedSettings);
  return (
    <section className="settings-section coding-method-section" aria-label="実装方法">
      <p>コード変更を実行する方法を選びます。PiとCodex SDKの設定は別々に保持します。</p>
      <div className="coding-method-current">
        保存済みの実装方法: <strong>{savedMethod}</strong>
        {dirty && <span> · 未保存の変更があります</span>}
      </div>
      <label>
        <input
          type="checkbox"
          checked={settings.enabled}
          onChange={(event) => setSettings({ ...settings, enabled: event.target.checked })}
        />{" "}
        実装ツールを有効にする
      </label>
      <div className="coding-method-cards">
        <section className={method === "pi" ? "coding-method-card selected" : "coding-method-card"}>
          <label className="coding-method-choice">
            <input
              type="radio"
              name="coding-method"
              checked={method === "pi"}
              onChange={() => setSettings({ ...settings, implementationMethod: "pi" })}
            />
            <span>
              <strong>Piを使って実装</strong>
              <small>ローカル実装エージェント</small>
            </span>
          </label>
          <CodingConnectionFields settings={settings} onChange={setSettings} />
          {(
            [
              ["executable", "Pi実行ファイル"],
              ["provider", "Pi Provider"],
              ["model", "Piモデル"],
            ] as const
          ).map(([key, label]) => (
            <label key={key}>
              {label}
              <input
                value={settings[key]}
                onChange={(event) => setSettings({ ...settings, [key]: event.target.value })}
              />
            </label>
          ))}
          <p className="settings-help">作業フォルダーは会話ごとに選択します。</p>
          {settings.profile.includes("codex-sdk") && (
            <p className="settings-help">
              この既存設定はPi経由です。Codexを直接使う場合は右の方法を選びます。
            </p>
          )}
        </section>
        <section
          className={method === "codex-sdk" ? "coding-method-card selected" : "coding-method-card"}
        >
          <label className="coding-method-choice">
            <input
              type="radio"
              name="coding-method"
              checked={method === "codex-sdk"}
              onChange={() => setSettings({ ...settings, implementationMethod: "codex-sdk" })}
            />
            <span>
              <strong>Codex SDKを使って実装</strong>
              <small>Piを経由しない実装経路</small>
            </span>
          </label>
          <label>
            モデル
            <input
              value={settings.codexModel}
              onChange={(event) => setSettings({ ...settings, codexModel: event.target.value })}
            />
          </label>
          <p>既存のCodexログインを使用します。作業フォルダーは会話ごとに選択します。</p>
          <p className="settings-help">
            選択したGit作業フォルダーへの書き込みを許可し、ネットワークアクセスを無効にします。
          </p>
        </section>
      </div>
      <p className="settings-help">高度推論の役割設定と、コード実装の方法は別々に管理します。</p>
      <div className="coding-method-actions">
        <button disabled={busy} onClick={() => void save(false)}>
          実装設定を保存
        </button>
        <button disabled={busy} onClick={() => void save(true)}>
          保存して接続確認
        </button>
      </div>
      {notice && <p role="status">{notice}</p>}
    </section>
  );
}
