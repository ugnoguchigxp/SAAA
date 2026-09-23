import { WorldProviderCapabilities } from "./WorldProviderCapabilities";
import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import type {
  AgentSessionProviderSettings,
  CloudAsrProviderSettings,
  CloudTtsProviderSettings,
  ModelProviderSettings,
  OpenAiCompatibleProviderSettings,
} from "../../lib/contracts";
import { testModelProvider } from "../../lib/runtime";
import { TtsVoiceControls } from "./TtsVoiceControls";
import { LlmRequestFields } from "./LlmRequestFields";
import { Field } from "./SettingsFields";
import {
  localizeProviderKind,
  localizeProviderLabel,
  localizeUiMessage,
} from "../../i18n/presentation";
import { classifyProviderTestFailure } from "./providerTestPresentation";
import { ApiKeyControl } from "./ApiKeyControl";

export function ProviderCard({
  provider,
  persisted,
  onChange,
  onRemove,
  removable = true,
  testProvider = testModelProvider,
}: {
  provider: ModelProviderSettings;
  testProvider?: typeof testModelProvider;
  persisted: boolean;
  onChange: (value: ModelProviderSettings) => void;
  onRemove: () => void;
  removable?: boolean;
}) {
  const { t } = useTranslation();
  const fingerprint = JSON.stringify(provider);
  const generation = useRef(0);
  const currentFingerprint = useRef(fingerprint);
  if (currentFingerprint.current !== fingerprint) {
    generation.current += 1;
    currentFingerprint.current = fingerprint;
  }
  useEffect(
    () => () => {
      generation.current += 1;
    },
    [],
  );
  const [testedFingerprint, setTestedFingerprint] = useState("");
  const [storedTestResult, setTestResult] = useState<
    | { state: "idle" }
    | { state: "testing" }
    | { state: "success"; latency: number }
    | { state: "error"; message: string }
  >({ state: "idle" });

  const testResult =
    testedFingerprint === fingerprint ? storedTestResult : { state: "idle" as const };
  function invalidateTest() {
    generation.current += 1;
    setTestResult({ state: "idle" });
  }
  async function test() {
    const requestGeneration = ++generation.current;
    setTestedFingerprint(fingerprint);
    setTestResult({ state: "testing" });
    try {
      const result = await testProvider(provider);
      if (requestGeneration !== generation.current) return;
      setTestResult(
        result.ok
          ? { state: "success", latency: result.latencyMs }
          : { state: "error", message: result.message },
      );
    } catch (cause) {
      if (requestGeneration !== generation.current) return;
      setTestResult({
        state: "error",
        message: cause instanceof Error ? cause.message : String(cause),
      });
    }
  }

  if (provider.kind === "dynamic-lan")
    return (
      <section className="settings-card">
        <h3>{provider.label}</h3>
        <LlmRequestFields
          value={provider.requestOptions}
          dynamic
          onChange={(requestOptions) => onChange({ ...provider, requestOptions })}
        />
      </section>
    );
  if (provider.kind === "system-tts") {
    return (
      <section className="settings-card provider-card">
        {persisted && <WorldProviderCapabilities providerId={provider.id} revision={fingerprint} />}
        <div className="card-title-row">
          <div>
            <h3>{localizeProviderLabel(t, provider.label)}</h3>
            <p className="muted">TTS · {provider.id}</p>
          </div>
          <span className="provider-test-result success">{t("common.ready")}</span>
        </div>
        <div className="settings-form-grid">
          <Field label={t("settings.providers.voice")}>
            <input value={provider.voice} disabled />
          </Field>
          <Field label={t("settings.providers.output")}>
            <input value={t("common.systemDefault")} disabled />
          </Field>
        </div>
      </section>
    );
  }

  return (
    <section className="settings-card provider-card">
      {persisted && <WorldProviderCapabilities providerId={provider.id} revision={fingerprint} />}
      <ProviderHeader provider={provider} onChange={onChange} />
      {provider.kind === "openai-compatible" && (
        <LlmFields provider={provider} onChange={onChange} />
      )}
      {provider.kind === "agent-session" && (
        <AgentSessionFields provider={provider} onChange={onChange} />
      )}
      {provider.kind === "cloud-asr" && <AsrFields provider={provider} onChange={onChange} />}
      {provider.kind === "cloud-tts" && <TtsFields provider={provider} onChange={onChange} />}
      <div className="provider-credential-section">
        <ApiKeyControl
          provider={provider}
          persisted={persisted}
          onCredentialChange={invalidateTest}
        />
      </div>
      {provider.kind === "openai-compatible" && (
        <LlmRequestFields
          value={provider.requestOptions}
          onChange={(requestOptions) => onChange({ ...provider, requestOptions })}
        />
      )}
      {testResult.state !== "idle" && (
        <div
          className={`provider-test-result detailed ${testResult.state}`}
          role="status"
          aria-live="polite"
        >
          <strong>
            {testResult.state === "testing"
              ? t("settings.providers.statusChecking")
              : testResult.state === "success"
                ? t("settings.providers.statusAvailable")
                : t(
                    `settings.providers.failure.${classifyProviderTestFailure(testResult.message)}.title`,
                  )}
          </strong>
          <span>
            {testResult.state === "testing"
              ? t("settings.providers.connecting")
              : testResult.state === "success"
                ? t("settings.providers.connectionSucceeded", { latency: testResult.latency })
                : t(
                    `settings.providers.failure.${classifyProviderTestFailure(testResult.message)}.recovery`,
                    { detail: localizeUiMessage(t, testResult.message, "settings") },
                  )}
          </span>
        </div>
      )}
      <div className="provider-card-footer">
        <div>
          <button
            className="text-button"
            type="button"
            disabled={
              testResult.state === "testing" ||
              (provider.authentication === "api-key" && !persisted)
            }
            onClick={() => void test()}
          >
            {t("settings.providers.testConnection")}
          </button>
          {removable && <button className="text-button danger" type="button" onClick={onRemove}>
            {t("settings.providers.removeProvider")}
          </button>}
        </div>
      </div>
    </section>
  );
}

function ProviderHeader({
  provider,
  onChange,
}: {
  provider: ModelProviderSettings;
  onChange: (value: ModelProviderSettings) => void;
}) {
  const { t } = useTranslation();
  return (
    <div className="card-title-row">
      <div>
        <h3>
          {provider.label
            ? localizeProviderLabel(t, provider.label)
            : t("settings.providers.provider")}
        </h3>
        <p className="muted">
          {localizeProviderKind(t, provider.kind)} · {provider.id}
        </p>
      </div>
      <label className="toggle">
        <input
          type="checkbox"
          checked={provider.enabled}
          onChange={(event) => onChange({ ...provider, enabled: event.target.checked })}
        />
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
      <Field label={t("settings.providers.displayName")}>
        <input
          value={provider.label}
          onChange={(event) => onChange({ ...provider, label: event.target.value })}
        />
      </Field>
      <Field label={t("settings.providers.location")}>
        <select
          value={provider.location}
          onChange={(event) =>
            onChange({ ...provider, location: event.target.value as "local" | "cloud" })
          }
        >
          <option value="local">{t("common.localProcessing")}</option>
          <option value="cloud">{t("common.cloudProcessing")}</option>
        </select>
      </Field>
      <Field label={t("settings.providers.endpoint")}>
        <input
          value={provider.endpoint}
          placeholder="https://api.example.com/v1"
          onChange={(event) => onChange({ ...provider, endpoint: event.target.value })}
        />
      </Field>
      <Field label={t("settings.providers.model")}>
        <input
          value={provider.model}
          placeholder={t("settings.providers.modelPlaceholder")}
          onChange={(event) => onChange({ ...provider, model: event.target.value })}
        />
      </Field>
      <Field label={t("settings.providers.authentication")}>
        <select
          value={provider.authentication}
          onChange={(event) =>
            onChange({ ...provider, authentication: event.target.value as "none" | "api-key" })
          }
        >
          <option value="api-key">{t("settings.providers.apiKey")}</option>
          <option value="none">{t("settings.providers.none")}</option>
        </select>
      </Field>
    </>
  );
}

function LlmFields({
  provider,
  onChange,
}: {
  provider: OpenAiCompatibleProviderSettings;
  onChange: (value: ModelProviderSettings) => void;
}) {
  return (
    <div className="settings-form-grid">
      <CommonCloudFields provider={provider} onChange={onChange} />
    </div>
  );
}

function AgentSessionFields({
  provider,
  onChange,
}: {
  provider: AgentSessionProviderSettings;
  onChange: (value: ModelProviderSettings) => void;
}) {
  const { t } = useTranslation();
  return (
    <div className="settings-form-grid">
      <Field label={t("settings.providers.displayName")}>
        <input
          value={provider.label}
          onChange={(event) => onChange({ ...provider, label: event.target.value })}
        />
      </Field>
      <Field label={t("settings.providers.baseUrl")}>
        <input
          value={provider.baseUrl}
          placeholder="http://127.0.0.1:44449"
          onChange={(event) => onChange({ ...provider, baseUrl: event.target.value })}
        />
      </Field>
      <Field label={t("settings.providers.model")}>
        <input
          value={provider.model}
          placeholder={t("settings.providers.modelPlaceholder")}
          onChange={(event) => onChange({ ...provider, model: event.target.value })}
        />
      </Field>
      <Field label={t("settings.providers.modelsPath")}>
        <input
          value={provider.modelsPath}
          placeholder="/v1/agents/models?runtime=agent"
          onChange={(event) => onChange({ ...provider, modelsPath: event.target.value })}
        />
      </Field>
      <Field label={t("settings.providers.sessionsPath")}>
        <input
          value={provider.sessionsPath}
          placeholder="/v1/agents/sessions"
          onChange={(event) => onChange({ ...provider, sessionsPath: event.target.value })}
        />
      </Field>
      <Field label={t("settings.providers.authentication")}>
        <select
          value={provider.authentication}
          onChange={(event) =>
            onChange({ ...provider, authentication: event.target.value as "none" | "api-key" })
          }
        >
          <option value="api-key">{t("settings.providers.apiKey")}</option>
          <option value="none">{t("settings.providers.none")}</option>
        </select>
      </Field>
    </div>
  );
}

function AsrFields({
  provider,
  onChange,
}: {
  provider: CloudAsrProviderSettings;
  onChange: (value: ModelProviderSettings) => void;
}) {
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

function TtsFields({
  provider,
  onChange,
}: {
  provider: CloudTtsProviderSettings;
  onChange: (value: ModelProviderSettings) => void;
}) {
  const { t } = useTranslation();
  return (
    <div className="settings-form-grid">
      <CommonCloudFields provider={provider} onChange={onChange} />
      {provider.model === "voicevox-core" ? null : (
        <Field label={t("settings.providers.voice")}>
          <input
            value={provider.voice}
            placeholder={t("settings.providers.voicePlaceholder")}
            onChange={(event) => onChange({ ...provider, voice: event.target.value })}
          />
        </Field>
      )}
      <TtsVoiceControls
        provider={provider}
        onChange={(next) => onChange(next)}
      />
      <Field label={t("settings.providers.audioFormat")}>
        <select
          value={provider.responseFormat ?? "wav"}
          onChange={(event) =>
            onChange({ ...provider, responseFormat: event.target.value as "wav" | "pcm" })
          }
        >
          <option value="wav">WAV</option>
          <option value="pcm">PCM · 24 kHz · mono · s16le</option>
        </select>
      </Field>
    </div>
  );
}
