import { ProviderCard } from "./ProviderCard";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import type { CodexAgentSettings, ModelProviderSettings, ModelProvidersSettings } from "../../lib/contracts";
import { localizeProviderLabel } from "../../i18n/presentation";
import "./IndividualProvidersSection.css";

const mimoPreset: ModelProviderSettings = {
  kind: "openai-compatible",
  id: "mimo-api",
  enabled: true,
  label: "MiMo API",
  location: "cloud",
  endpoint: "https://api.xiaomimimo.com/v1",
  model: "mimo-v2.6-pro",
  authentication: "api-key",
};

export function IndividualProvidersSection({
  settings,
  codex,
  persistedProviderIds,
  primaryProviderId,
  onChange,
  onCodexChange,
}: {
  settings: ModelProvidersSettings;
  codex: CodexAgentSettings;
  persistedProviderIds: ReadonlySet<string>;
  primaryProviderId: string | null;
  onChange: (value: ModelProvidersSettings) => void;
  onCodexChange: (value: CodexAgentSettings) => void;
}) {
  const { t } = useTranslation();
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [filter, setFilter] = useState<"all" | "llm" | "asr" | "tts">("all");
  function addProvider(capability: "llm" | "agent-llm" | "asr" | "tts") {
    const isAgentSession = capability === "agent-llm";
    const id = `${isAgentSession ? "agent" : "cloud"}-${capability}-${crypto.randomUUID()}`;
    // Persist a stable default identifier, then localize it at the display boundary.
    // This keeps a newly added provider's default label in sync with later language changes.
    const common = {
      id,
      enabled: false,
      label: `Cloud ${capability.toUpperCase()}`,
      location: "cloud" as const,
    };
    const provider: ModelProviderSettings = isAgentSession
      ? {
          id,
          enabled: false,
          label: "Agent Session LLM",
          location: "local",
          kind: "agent-session",
          baseUrl: "http://127.0.0.1:44449",
          model: "",
          modelsPath: "/v1/agents/models?runtime=muse",
          sessionsPath: "/v1/agents/sessions",
          authentication: "none",
        }
      : capability === "llm"
        ? {
            ...common,
            kind: "openai-compatible",
            endpoint: "https://api.openai.com/v1",
            model: "",
            authentication: "api-key",
          }
        : capability === "asr"
          ? {
              ...common,
              kind: "cloud-asr",
              endpoint: "https://api.openai.com/v1",
              model: "",
              language: "auto",
              authentication: "api-key",
            }
          : {
              ...common,
              kind: "cloud-tts",
              endpoint: "https://api.openai.com/v1",
              model: "",
              voice: "",
              authentication: "api-key",
            };
    onChange({ ...settings, providers: [...settings.providers, provider] });
    setSelectedId(id);
  }

  function replace(id: string, provider: ModelProviderSettings) {
    onChange({
      ...settings,
      providers: settings.providers.some((current) => current.id === id)
        ? settings.providers.map((current) => (current.id === id ? provider : current))
        : [...settings.providers, provider],
    });
  }

  function remove(provider: ModelProviderSettings) {
    onChange({
      ...settings,
      providers: settings.providers.filter((current) => current.id !== provider.id),
    });
    setSelectedId(null);
  }

  const configuredMiMo = settings.providers.find((provider) => provider.id === mimoPreset.id);
  const individualProviders = settings.providers.filter((provider) => provider.kind !== "dynamic-lan");
  const allProviders = [
    configuredMiMo ?? mimoPreset,
    ...individualProviders.filter((provider) => provider.id !== mimoPreset.id),
  ];
  const visible = allProviders.filter((provider) => {
    if (filter === "all") return true;
    if (filter === "llm") return provider.kind === "openai-compatible" || provider.kind === "agent-session";
    if (filter === "asr") return provider.kind === "cloud-asr";
    return provider.kind === "cloud-tts" || provider.kind === "system-tts";
  });
  const codexVisible = filter === "all" || filter === "llm";
  const codexSelected = selectedId === "codex-sdk" && codexVisible;
  const selected = codexSelected ? undefined : visible.find((provider) => provider.id === selectedId)
    ?? visible.find((provider) => provider.id === mimoPreset.id)
    ?? visible[0];
  return (
    <div className="individual-services">
      <div className="individual-services-summary">
        <span>会話の既定接続先: <strong>{settings.providers.find((provider) => provider.id === primaryProviderId)?.label ?? "未設定"}</strong></span>
        <span>登録済み個別プロバイダー: {individualProviders.length + 1}</span>
      </div>
      <div className="individual-services-grid">
        <section className="settings-card individual-services-list">
          <h3>{t("settings.providers.title")}</h3>
          <div className="individual-services-filters" aria-label="プロバイダーの種類">
            {(["all", "llm", "asr", "tts"] as const).map((kind) => (
              <button key={kind} type="button" className={filter === kind ? "active" : ""} onClick={() => setFilter(kind)}>
                {kind === "all" ? "すべて" : kind.toUpperCase()}
              </button>
            ))}
          </div>
          <div className="individual-services-items">
            {visible.map((provider) => (
              <button key={provider.id} type="button" className={selected?.id === provider.id ? "individual-services-item selected" : "individual-services-item"} onClick={() => setSelectedId(provider.id)}>
                <strong>{localizeProviderLabel(t, provider.label)}</strong>
                {provider.id === primaryProviderId && <span className="individual-services-default">会話の既定</span>}
                <small>{provider.kind === "cloud-asr" ? "ASR" : provider.kind === "cloud-tts" || provider.kind === "system-tts" ? "TTS" : "LLM"} · {persistedProviderIds.has(provider.id) ? provider.id : "保存前"}</small>
              </button>
            ))}
            {codexVisible && (
              <button type="button" className={codexSelected ? "individual-services-item selected" : "individual-services-item"} onClick={() => setSelectedId("codex-sdk") }>
                <strong>Codex SDK</strong>
                <small>高度推論 · {codex.model}</small>
              </button>
            )}
          </div>
          <details className="individual-services-add">
            <summary>＋ プロバイダーを追加</summary>
            <div>
            <button
              className="add-provider-button"
              type="button"
              onClick={() => addProvider("llm")}
            >
              ＋ LLM
            </button>
            <button
              className="add-provider-button"
              type="button"
              onClick={() => addProvider("agent-llm")}
            >
              ＋ Agent LLM
            </button>
            <button
              className="add-provider-button"
              type="button"
              onClick={() => addProvider("asr")}
            >
              ＋ ASR
            </button>
            <button
              className="add-provider-button"
              type="button"
              onClick={() => addProvider("tts")}
            >
              ＋ TTS
            </button>
            </div>
          </details>
        </section>
        <div className="individual-services-detail">
          {codexSelected && (
            <section className="settings-card provider-card">
              <h3>Codex SDK</h3>
              <p className="settings-help">高度推論の候補です。会話の既定接続先はここでは変更しません。役割への割当は「Role routing」で設定します。</p>
              <div className="settings-form-grid">
                <label className="settings-field">
                  <span>利用状態</span>
                  <select value={codex.enabled ? "enabled" : "disabled"} onChange={(event) => onCodexChange({ ...codex, enabled: event.target.value === "enabled" })}>
                    <option value="disabled">無効</option>
                    <option value="enabled">有効</option>
                  </select>
                </label>
                <label className="settings-field">
                  <span>モデル</span>
                  <input value={codex.model} onChange={(event) => onCodexChange({ ...codex, model: event.target.value })} />
                </label>
                <label className="settings-field">
                  <span>ランタイム</span>
                  <input value={codex.runtimeMode} disabled />
                </label>
                <label className="settings-field">
                  <span>接続状態</span>
                  <input value={codex.health} disabled />
                </label>
              </div>
            </section>
          )}
          {selected?.id === mimoPreset.id && !settings.providers.some((provider) => provider.id === mimoPreset.id) && (
            <div className="individual-services-preset">
              <span>MiMo API の設定を保存すると、APIキーを入力できます。</span>
              <button type="button" onClick={() => replace(mimoPreset.id, mimoPreset)}>MiMo APIを登録</button>
            </div>
          )}
          {selected && <ProviderCard
            key={selected.id}
            provider={selected}
            persisted={persistedProviderIds.has(selected.id)}
            removable={selected.id !== mimoPreset.id || persistedProviderIds.has(selected.id)}
            onChange={(next) => replace(selected.id, next)}
            onRemove={() => remove(selected)}
          />}
        </div>
      </div>
    </div>
  );
}
