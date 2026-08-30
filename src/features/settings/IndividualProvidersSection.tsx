import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import type {
  CloudAsrProviderSettings,
  CloudTtsProviderSettings,
  ModelProviderSettings,
  ModelProvidersSettings,
  OpenAiCompatibleProviderSettings,
  ProviderCredentialState,
} from "../../lib/contracts";
import {
  deleteProviderApiKey,
  getProviderCredentialState,
  setProviderApiKey,
  testModelProvider,
} from "../../lib/runtime";
import { Field } from "./SettingsFields";
import { localizeProviderKind, localizeProviderLabel, localizeUiMessage } from "../../i18n/presentation";

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
  function addProvider(capability: "llm" | "asr" | "tts") {
    const id = `cloud-${capability}-${crypto.randomUUID()}`;
    // Persist a stable default identifier, then localize it at the display boundary.
    // This keeps a newly added provider's default label in sync with later language changes.
    const common = { id, enabled: false, label: `Cloud ${capability.toUpperCase()}`, location: "cloud" as const };
    const provider: ModelProviderSettings = capability === "llm"
      ? { ...common, kind: "openai-compatible", endpoint: "https://api.openai.com/v1", model: "", authentication: "api-key" }
      : capability === "asr"
        ? { ...common, kind: "cloud-asr", endpoint: "https://api.openai.com/v1", model: "", language: "auto", authentication: "api-key" }
        : { ...common, kind: "cloud-tts", endpoint: "https://api.openai.com/v1", model: "", voice: "", authentication: "api-key" };
    onChange({ ...settings, providers: [...settings.providers, provider] });
  }

  function replace(id: string, provider: ModelProviderSettings) {
    onChange({
      ...settings,
      providers: settings.providers.map((current) => current.id === id ? provider : current),
    });
  }

  function remove(provider: ModelProviderSettings) {
    onChange({ ...settings, providers: settings.providers.filter((current) => current.id !== provider.id) });
  }

  const visible = settings.providers.filter((provider) => provider.kind !== "dynamic-lan");
  return (
    <div className="settings-stack">
      <section className="settings-card">
        <p className="eyebrow">{t("settings.providers.eyebrow")}</p>
        <h3>{t("settings.providers.title")}</h3>
        <p className="settings-help">
          {t("settings.providers.description")}
        </p>
        <div className="provider-card-footer">
          <span>{t("settings.providers.stableId")}</span>
          <div>
            <button className="add-provider-button" type="button" onClick={() => addProvider("llm")}>＋ LLM</button>
            <button className="add-provider-button" type="button" onClick={() => addProvider("asr")}>＋ ASR</button>
            <button className="add-provider-button" type="button" onClick={() => addProvider("tts")}>＋ TTS</button>
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

function ProviderCard({
  provider,
  persisted,
  onChange,
  onRemove,
}: {
  provider: ModelProviderSettings;
  persisted: boolean;
  onChange: (value: ModelProviderSettings) => void;
  onRemove: () => void;
}) {
  const { t } = useTranslation();
  const [testResult, setTestResult] = useState<
    | { state: "idle" }
    | { state: "testing" }
    | { state: "success"; latency: number }
    | { state: "error"; message: string }
  >({ state: "idle" });

  async function test() {
    setTestResult({ state: "testing" });
    try {
      const result = await testModelProvider(provider);
      setTestResult(result.ok ? { state: "success", latency: result.latencyMs } : { state: "error", message: result.message });
    } catch (cause) {
      setTestResult({ state: "error", message: cause instanceof Error ? cause.message : String(cause) });
    }
  }

  if (provider.kind === "dynamic-lan") return null;
  if (provider.kind === "system-tts") {
    return (
      <section className="settings-card provider-card">
        <div className="card-title-row">
          <div><h3>{localizeProviderLabel(t, provider.label)}</h3><p className="muted">TTS · {provider.id}</p></div>
          <span className="provider-test-result success">{t("common.ready")}</span>
        </div>
        <div className="settings-form-grid">
          <Field label={t("settings.providers.voice")}><input value={provider.voice} disabled /></Field>
          <Field label={t("settings.providers.output")}><input value={t("common.systemDefault")} disabled /></Field>
        </div>
      </section>
    );
  }
  if (provider.kind === "larm") {
    return (
      <section className="settings-card provider-card">
        <ProviderHeader provider={provider} onChange={onChange} />
        <div className="settings-form-grid">
          <Field label={t("settings.providers.baseUrl")}><input value={provider.baseUrl} onChange={(event) => onChange({ ...provider, baseUrl: event.target.value })} /></Field>
          <Field label={t("settings.providers.runtime")}><input value={t("settings.providers.existingDeployment")} disabled /></Field>
        </div>
      </section>
    );
  }

  return (
    <section className="settings-card provider-card">
      <ProviderHeader provider={provider} onChange={onChange} />
      {provider.kind === "openai-compatible" && (
        <LlmFields provider={provider} onChange={onChange} />
      )}
      {provider.kind === "cloud-asr" && (
        <AsrFields provider={provider} onChange={onChange} />
      )}
      {provider.kind === "cloud-tts" && (
        <TtsFields provider={provider} onChange={onChange} />
      )}
      {testResult.state !== "idle" && <p className={`provider-test-result ${testResult.state}`}>{testResult.state === "testing" ? t("settings.providers.connecting") : testResult.state === "success" ? t("settings.providers.connectionSucceeded", { latency: testResult.latency }) : localizeUiMessage(t, testResult.message, "settings")}</p>}
      <div className="provider-card-footer">
        <ApiKeyControl provider={provider} persisted={persisted} />
        <div>
          <button className="text-button" type="button" disabled={testResult.state === "testing" || (provider.authentication === "api-key" && !persisted)} onClick={() => void test()}>{t("settings.providers.testConnection")}</button>
          <button className="text-button danger" type="button" onClick={onRemove}>{t("settings.providers.removeProvider")}</button>
        </div>
      </div>
    </section>
  );
}

function ProviderHeader({ provider, onChange }: { provider: ModelProviderSettings; onChange: (value: ModelProviderSettings) => void }) {
  const { t } = useTranslation();
  return (
    <div className="card-title-row">
      <div>
        <h3>{provider.label ? localizeProviderLabel(t, provider.label) : t("settings.providers.provider")}</h3>
        <p className="muted">{localizeProviderKind(t, provider.kind)} · {provider.id}</p>
      </div>
      <label className="toggle">
        <input type="checkbox" checked={provider.enabled} onChange={(event) => onChange({ ...provider, enabled: event.target.checked })} />
        <span />
      </label>
    </div>
  );
}

function CommonCloudFields({
  provider,
  onChange,
}: {
  provider: OpenAiCompatibleProviderSettings | CloudAsrProviderSettings | CloudTtsProviderSettings;
  onChange: (value: typeof provider) => void;
}) {
  const { t } = useTranslation();
  return (
    <>
      <Field label={t("settings.providers.displayName")}><input value={provider.label} onChange={(event) => onChange({ ...provider, label: event.target.value })} /></Field>
      <Field label={t("settings.providers.endpoint")}><input value={provider.endpoint} placeholder="https://api.example.com/v1" onChange={(event) => onChange({ ...provider, endpoint: event.target.value })} /></Field>
      <Field label={t("settings.providers.model")}><input value={provider.model} placeholder={t("settings.providers.modelPlaceholder")} onChange={(event) => onChange({ ...provider, model: event.target.value })} /></Field>
      <Field label={t("settings.providers.authentication")}>
        <select value={provider.authentication} onChange={(event) => onChange({ ...provider, authentication: event.target.value as "none" | "api-key" })}>
          <option value="api-key">{t("settings.providers.apiKey")}</option>
          <option value="none">{t("settings.providers.none")}</option>
        </select>
      </Field>
    </>
  );
}

function LlmFields({ provider, onChange }: { provider: OpenAiCompatibleProviderSettings; onChange: (value: ModelProviderSettings) => void }) {
  return <div className="settings-form-grid"><CommonCloudFields provider={provider} onChange={onChange} /></div>;
}

function AsrFields({ provider, onChange }: { provider: CloudAsrProviderSettings; onChange: (value: ModelProviderSettings) => void }) {
  const { t } = useTranslation();
  return (
    <div className="settings-form-grid">
      <CommonCloudFields provider={provider} onChange={onChange} />
      <Field label={t("settings.providers.language")}>
        <select value={provider.language} disabled>
          <option value="auto">{t("settings.providers.autoDetect")}</option>
        </select>
      </Field>
    </div>
  );
}

function TtsFields({ provider, onChange }: { provider: CloudTtsProviderSettings; onChange: (value: ModelProviderSettings) => void }) {
  const { t } = useTranslation();
  return (
    <div className="settings-form-grid">
      <CommonCloudFields provider={provider} onChange={onChange} />
      <Field label={t("settings.providers.voice")}><input value={provider.voice} placeholder={t("settings.providers.voicePlaceholder")} onChange={(event) => onChange({ ...provider, voice: event.target.value })} /></Field>
    </div>
  );
}

function ApiKeyControl({
  provider,
  persisted,
}: {
  provider: OpenAiCompatibleProviderSettings | CloudAsrProviderSettings | CloudTtsProviderSettings;
  persisted: boolean;
}) {
  const { t } = useTranslation();
  const [credential, setCredential] = useState<ProviderCredentialState["state"]>("missing");
  const [apiKey, setApiKey] = useState("");
  const [saving, setSaving] = useState(false);
  const [credentialError, setCredentialError] = useState<string | null>(null);
  useEffect(() => {
    if (provider.authentication !== "api-key" || !persisted) return;
    let active = true;
    setCredentialError(null);
    void getProviderCredentialState(provider.id)
      .then((result) => { if (active) setCredential(result.state); })
      .catch((cause) => {
        if (!active) return;
        setCredential("unavailable");
        setCredentialError(cause instanceof Error ? cause.message : String(cause));
      });
    return () => { active = false; };
  }, [persisted, provider.authentication, provider.id]);
  if (provider.authentication !== "api-key") return <span>{t("settings.providers.authNone")}</span>;
  if (!persisted) return <span>{t("settings.providers.saveBeforeKey")}</span>;
  async function save() {
    setSaving(true);
    setCredentialError(null);
    try {
      const result = await setProviderApiKey(provider.id, apiKey);
      setCredential(result.state);
      setApiKey("");
    } catch (cause) {
      setCredentialError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setSaving(false);
    }
  }
  async function remove() {
    setSaving(true);
    setCredentialError(null);
    try {
      const result = await deleteProviderApiKey(provider.id);
      setCredential(result.state);
      setApiKey("");
    } catch (cause) {
      setCredentialError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setSaving(false);
    }
  }
  return (
    <div>
      <span>{t("settings.providers.apiKeyState", { state: t(`common.${credential}`, { defaultValue: credential }) })}</span>
      {credentialError && <p className="provider-test-result error" aria-live="polite">{localizeUiMessage(t, credentialError, "settings")}</p>}
      <div>
        <input type="password" value={apiKey} autoComplete="off" placeholder={credential === "configured" ? t("settings.providers.replaceKey") : t("settings.providers.enterKey")} onChange={(event) => setApiKey(event.target.value)} />
        <button className="text-button" type="button" disabled={!apiKey || saving} onClick={() => void save()}>{t("settings.providers.saveKey")}</button>
        {credential === "configured" && <button className="text-button danger" type="button" disabled={saving} onClick={() => void remove()}>{t("settings.providers.deleteKey")}</button>}
      </div>
    </div>
  );
}
