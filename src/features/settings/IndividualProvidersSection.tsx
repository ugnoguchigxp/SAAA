import { ProviderCard } from "./ProviderCard";
import { useTranslation } from "react-i18next";
import type { ModelProviderSettings, ModelProvidersSettings } from "../../lib/contracts";
export function IndividualProvidersSection({
  settings,
  persistedProviderIds,
  onChange,
}: {
  settings: ModelProvidersSettings;
  persistedProviderIds: ReadonlySet<string>;
  onChange: (value: ModelProvidersSettings) => void;
}) {
  const { t } = useTranslation();
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
  }

  function replace(id: string, provider: ModelProviderSettings) {
    onChange({
      ...settings,
      providers: settings.providers.map((current) => (current.id === id ? provider : current)),
    });
  }

  function remove(provider: ModelProviderSettings) {
    onChange({
      ...settings,
      providers: settings.providers.filter((current) => current.id !== provider.id),
    });
  }

  const visible = settings.providers.filter((provider) => provider.kind !== "dynamic-lan");
  return (
    <div className="settings-stack">
      <section className="settings-card">
        <p className="eyebrow">{t("settings.providers.eyebrow")}</p>
        <h3>{t("settings.providers.title")}</h3>
        <p className="settings-help">{t("settings.providers.description")}</p>
        <div className="provider-card-footer">
          <span>{t("settings.providers.stableId")}</span>
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
        </div>
      </section>
      {visible.map((provider) => (
        <ProviderCard
          key={provider.id}
          provider={provider}
          persisted={persistedProviderIds.has(provider.id)}
          onChange={(next) => replace(provider.id, next)}
          onRemove={() => void remove(provider)}
        />
      ))}
    </div>
  );
}
