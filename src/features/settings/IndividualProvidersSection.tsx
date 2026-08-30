import { useEffect, useState } from "react";
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

export function IndividualProvidersSection({
  settings,
  persistedProviderIds,
  onChange,
}: {
  settings: ModelProvidersSettings;
  persistedProviderIds: ReadonlySet<string>;
  onChange: (value: ModelProvidersSettings) => void;
}) {
  function addProvider(capability: "llm" | "asr" | "tts") {
    const id = `cloud-${capability}-${crypto.randomUUID()}`;
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
        <p className="eyebrow">INDIVIDUAL SERVICES</p>
        <h3>Cloud Provider catalog</h3>
        <p className="settings-help">
          Harnessを使わないサービスだけ個別に登録します。API keyはmacOS Keychainへ保存し、設定・SQLite・診断には含めません。
        </p>
        <div className="provider-card-footer">
          <span>Provider IDは作成後に変更しません。</span>
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
  const [testState, setTestState] = useState<"idle" | "testing" | "success" | "error">("idle");
  const [testMessage, setTestMessage] = useState<string | null>(null);

  async function test() {
    setTestState("testing");
    setTestMessage("Connecting…");
    try {
      const result = await testModelProvider(provider);
      setTestState(result.ok ? "success" : "error");
      setTestMessage(`${result.message} · ${result.latencyMs} ms`);
    } catch (cause) {
      setTestState("error");
      setTestMessage(cause instanceof Error ? cause.message : String(cause));
    }
  }

  if (provider.kind === "dynamic-lan") return null;
  if (provider.kind === "system-tts") {
    return (
      <section className="settings-card provider-card">
        <div className="card-title-row">
          <div><h3>{provider.label}</h3><p className="muted">TTS · {provider.id}</p></div>
          <span className="provider-test-result success">ready</span>
        </div>
        <div className="settings-form-grid">
          <Field label="Voice"><input value={provider.voice} disabled /></Field>
          <Field label="Output"><input value="System default" disabled /></Field>
        </div>
      </section>
    );
  }
  if (provider.kind === "larm") {
    return (
      <section className="settings-card provider-card">
        <ProviderHeader provider={provider} onChange={onChange} />
        <div className="settings-form-grid">
          <Field label="Base URL"><input value={provider.baseUrl} onChange={(event) => onChange({ ...provider, baseUrl: event.target.value })} /></Field>
          <Field label="Runtime"><input value="LARM · existing deployment" disabled /></Field>
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
      {testMessage && <p className={`provider-test-result ${testState}`}>{testMessage}</p>}
      <div className="provider-card-footer">
        <ApiKeyControl provider={provider} persisted={persisted} />
        <div>
          <button className="text-button" type="button" disabled={testState === "testing" || (provider.authentication === "api-key" && !persisted)} onClick={() => void test()}>Test connection</button>
          <button className="text-button danger" type="button" onClick={onRemove}>Remove provider</button>
        </div>
      </div>
    </section>
  );
}

function ProviderHeader({ provider, onChange }: { provider: ModelProviderSettings; onChange: (value: ModelProviderSettings) => void }) {
  return (
    <div className="card-title-row">
      <div>
        <h3>{provider.label || "Provider"}</h3>
        <p className="muted">{provider.kind} · {provider.id}</p>
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
  return (
    <>
      <Field label="Display name"><input value={provider.label} onChange={(event) => onChange({ ...provider, label: event.target.value })} /></Field>
      <Field label="Endpoint"><input value={provider.endpoint} placeholder="https://api.example.com/v1" onChange={(event) => onChange({ ...provider, endpoint: event.target.value })} /></Field>
      <Field label="Model"><input value={provider.model} placeholder="model name" onChange={(event) => onChange({ ...provider, model: event.target.value })} /></Field>
      <Field label="Authentication">
        <select value={provider.authentication} onChange={(event) => onChange({ ...provider, authentication: event.target.value as "none" | "api-key" })}>
          <option value="api-key">API key</option>
          <option value="none">None</option>
        </select>
      </Field>
    </>
  );
}

function LlmFields({ provider, onChange }: { provider: OpenAiCompatibleProviderSettings; onChange: (value: ModelProviderSettings) => void }) {
  return <div className="settings-form-grid"><CommonCloudFields provider={provider} onChange={onChange} /></div>;
}

function AsrFields({ provider, onChange }: { provider: CloudAsrProviderSettings; onChange: (value: ModelProviderSettings) => void }) {
  return (
    <div className="settings-form-grid">
      <CommonCloudFields provider={provider} onChange={onChange} />
      <Field label="Language">
        <select value={provider.language} disabled>
          <option value="auto">Auto detect</option>
        </select>
      </Field>
    </div>
  );
}

function TtsFields({ provider, onChange }: { provider: CloudTtsProviderSettings; onChange: (value: ModelProviderSettings) => void }) {
  return (
    <div className="settings-form-grid">
      <CommonCloudFields provider={provider} onChange={onChange} />
      <Field label="Voice"><input value={provider.voice} placeholder="voice name" onChange={(event) => onChange({ ...provider, voice: event.target.value })} /></Field>
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
  if (provider.authentication !== "api-key") return <span>Authentication: none</span>;
  if (!persisted) return <span>Provider設定を保存するとAPI keyを登録できます。</span>;
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
      <span>API key: {credential}</span>
      {credentialError && <p className="provider-test-result error" aria-live="polite">{credentialError}</p>}
      <div>
        <input type="password" value={apiKey} autoComplete="off" placeholder={credential === "configured" ? "Replace API key" : "Enter API key"} onChange={(event) => setApiKey(event.target.value)} />
        <button className="text-button" type="button" disabled={!apiKey || saving} onClick={() => void save()}>Save key</button>
        {credential === "configured" && <button className="text-button danger" type="button" disabled={saving} onClick={() => void remove()}>Delete key</button>}
      </div>
    </div>
  );
}
