import { RoutePolicyFields } from "./RoutePolicyFields";
import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import type {
  HarnessResolution,
  ModelProviderSettings,
  ModelProvidersSettings,
  RoutingSettings,
} from "../../lib/contracts";
import { resolveServiceHarness } from "../../lib/runtime";
import { legacyDynamicLanHost } from "../../lib/providerRuntime";
import { localizeProviderLabel, localizeStatus, localizeUiMessage } from "../../i18n/presentation";
import { ConversationTimeoutField } from "./ConversationTimeoutField";
import { Field } from "./SettingsFields";
import { DEFAULT_LARM_PROFILE } from "./settingsDefaults";

type Capability = "llm" | "asr" | "tts";
type ResolveNotice =
  | { kind: "resolvedAll" | "resolvedPartial" | "agentConnectionReady" }
  | { kind: "error"; message: string };

export function ServiceConnectionsSection({
  providers,
  routing,
  onProvidersChange,
  onRoutingChange,
  onValidityChange,
  resolveHarnessAddress = resolveServiceHarness,
}: {
  providers: ModelProvidersSettings;
  routing: RoutingSettings;
  onProvidersChange: (value: ModelProvidersSettings) => void;
  onRoutingChange: (value: RoutingSettings) => void;
  onValidityChange: (valid: boolean) => void;
  resolveHarnessAddress?: typeof resolveServiceHarness;
}) {
  const { t } = useTranslation();
  const [resolution, setResolution] = useState<HarnessResolution | null>(null);
  const [resolveState, setResolveState] = useState<"idle" | "resolving" | "error">("idle");
  const [resolveMessage, setResolveMessage] = useState<ResolveNotice | null>(null);
  const resolveGeneration = useRef(0);
  const harnessAddressRef = useRef(providers.harness.address);
  harnessAddressRef.current = providers.harness.address;

  useEffect(() => {
    resolveGeneration.current += 1;
    setResolution(null);
    setResolveState("idle");
    setResolveMessage(null);
  }, [providers.harness.address]);

  const candidates = {
    llm: providers.providers.filter(isLlmProvider),
    asr: providers.providers.filter((provider) => provider.kind === "cloud-asr"),
    tts: providers.providers.filter(
      (provider) => provider.kind === "cloud-tts" || provider.kind === "system-tts",
    ),
  };

  function changeHarnessAddress(address: string) {
    resolveGeneration.current += 1;
    const host = legacyDynamicLanHost(address);
    onProvidersChange({
      ...providers,
      harness: { ...providers.harness, address },
      providers: providers.providers.map((provider) =>
        provider.kind === "dynamic-lan" && host ? { ...provider, enabled: true, host } : provider,
      ),
    });
    setResolution(null);
    setResolveState("idle");
    setResolveMessage(null);
  }

  async function resolveHarness() {
    const generation = ++resolveGeneration.current;
    const address = providers.harness.address;
    setResolveState("resolving");
    setResolveMessage(null);
    try {
      const next = await resolveHarnessAddress(address);
      if (generation !== resolveGeneration.current || address !== harnessAddressRef.current) return;
      setResolution(next);
      setResolveState("idle");
      setResolveMessage({
        kind:
          next.revision === "agent-connection.v1"
            ? "agentConnectionReady"
            : next.state === "ready"
              ? "resolvedAll"
              : "resolvedPartial",
      });
    } catch (cause) {
      if (generation !== resolveGeneration.current || address !== harnessAddressRef.current) return;
      setResolution(null);
      setResolveState("error");
      setResolveMessage({
        kind: "error",
        message: cause instanceof Error ? cause.message : String(cause),
      });
    }
  }

  function setSource(capability: Capability, source: "harness" | "provider") {
    const providerId =
      source === "provider"
        ? (candidates[capability].find((provider) => provider.enabled)?.id ?? null)
        : null;
    if (capability === "llm") {
      onRoutingChange({
        ...routing,
        conversationRespond: {
          ...routing.conversationRespond,
          source,
          primaryProviderId: providerId,
          fallbackProviderIds: [],
        },
      });
    } else if (capability === "asr") {
      onRoutingChange({
        ...routing,
        voiceTranscribe: {
          ...routing.voiceTranscribe,
          source,
          providerId,
          fallbackProviderIds: [],
        },
      });
    } else {
      onRoutingChange({
        ...routing,
        voiceSpeak: { ...routing.voiceSpeak, source, providerId, fallbackProviderIds: [] },
      });
    }
  }

  function setProvider(capability: Capability, providerId: string) {
    if (capability === "llm") {
      onRoutingChange({
        ...routing,
        conversationRespond: {
          ...routing.conversationRespond,
          primaryProviderId: providerId,
          fallbackProviderIds: routing.conversationRespond.fallbackProviderIds.filter(
            (id) => id !== providerId,
          ),
        },
      });
    } else if (capability === "asr") {
      onRoutingChange({
        ...routing,
        voiceTranscribe: {
          ...routing.voiceTranscribe,
          providerId,
          fallbackProviderIds: routing.voiceTranscribe.fallbackProviderIds?.filter(
            (id) => id !== providerId,
          ),
        },
      });
    } else {
      onRoutingChange({
        ...routing,
        voiceSpeak: {
          ...routing.voiceSpeak,
          providerId,
          fallbackProviderIds: routing.voiceSpeak.fallbackProviderIds?.filter(
            (id) => id !== providerId,
          ),
        },
      });
    }
  }

  return (
    <div className="settings-stack">
      <section className="settings-card">
        <div className="card-title-row">
          <div>
            <p className="eyebrow">{t("settings.connection.eyebrow")}</p>
            <h3>{t("settings.connection.title")}</h3>
            <p className="settings-help">{t("settings.connection.description")}</p>
          </div>
          <span
            className={`provider-test-result ${resolution?.state === "ready" ? "success" : resolveState === "error" ? "error" : ""}`}
          >
            {resolveState === "resolving"
              ? t("settings.connection.resolving")
              : localizeStatus(t, resolution?.state ?? "unchecked")}
          </span>
        </div>
        <div className="settings-form-grid">
          <Field label={t("settings.connection.harnessAddress")}>
            <input
              value={providers.harness.address}
              placeholder="http://provider.local:9810"
              onChange={(event) => changeHarnessAddress(event.target.value)}
            />
          </Field>
          <Field label={t("settings.compatibility.voiceProfile")}>
            <input
              value={providers.harness.larmProfile ?? DEFAULT_LARM_PROFILE}
              onChange={(e) =>
                onProvidersChange({
                  ...providers,
                  harness: { ...providers.harness, larmProfile: e.target.value },
                })
              }
            />
            <p className="settings-help">{t("settings.connection.larmProfileHint")}</p>
          </Field>
          <Field label={t("settings.providers.voice")}>
            <input
              value={providers.harness.ttsVoice ?? ""}
              placeholder={t("settings.connection.providerDefault")}
              onChange={(e) =>
                onProvidersChange({
                  ...providers,
                  harness: { ...providers.harness, ttsVoice: e.target.value || undefined },
                })
              }
            />
          </Field>
          <Field label={t("settings.connection.reasoningEffort")}>
            <select
              value={providers.reasoningEffort}
              onChange={(event) =>
                onProvidersChange({
                  ...providers,
                  reasoningEffort: event.target.value as ModelProvidersSettings["reasoningEffort"],
                })
              }
            >
              <option value="provider-default">{t("settings.connection.providerDefault")}</option>
              <option value="low">{t("settings.connection.low")}</option>
              <option value="medium">{t("settings.connection.medium")}</option>
              <option value="xhigh">{t("settings.connection.extraHigh")}</option>
            </select>
          </Field>
          <ConversationTimeoutField
            timeoutMs={routing.conversationRespond.timeoutMs}
            legacyDynamicLan={resolution?.revision === "agent-connection.v1"}
            onValidityChange={onValidityChange}
            onChange={(timeoutMs) =>
              onRoutingChange({
                ...routing,
                conversationRespond: { ...routing.conversationRespond, timeoutMs },
              })
            }
          />
        </div>
        <div className="provider-card-footer">
          <span>
            {resolveMessage?.kind === "error"
              ? localizeUiMessage(t, resolveMessage.message, "settings")
              : resolveMessage?.kind === "agentConnectionReady"
                ? t("settings.connection.agentConnectionReady")
                : resolveMessage?.kind === "resolvedAll"
                  ? t("settings.connection.resolvedAll")
                  : resolveMessage?.kind === "resolvedPartial"
                    ? t("settings.connection.resolvedPartial")
                    : t("settings.connection.resolutionHint")}
          </span>
          <button
            className="text-button"
            type="button"
            disabled={!providers.harness.address || resolveState === "resolving"}
            onClick={() => void resolveHarness()}
          >
            {t("settings.connection.resolveServices")}
          </button>
        </div>
      </section>

      <section className="settings-card">
        <h3>{t("settings.connection.sourcesTitle")}</h3>
        <p className="settings-help">{t("settings.connection.sourcesDescription")}</p>
        <div className="settings-stack">
          <SourceRow
            capability="llm"
            label="LLM"
            source={routing.conversationRespond.source}
            providerId={routing.conversationRespond.primaryProviderId}
            candidates={candidates.llm}
            resolution={resolution}
            onSourceChange={setSource}
            onProviderChange={setProvider}
          />
          <RoutePolicyFields
            maxTotalMs={3_600_000}
            value={routing.conversationRespond}
            primary={routing.conversationRespond.primaryProviderId}
            candidates={candidates.llm}
            onChange={(policy) =>
              onRoutingChange({
                ...routing,
                conversationRespond: {
                  ...routing.conversationRespond,
                  ...policy,
                  fallbackProviderIds: policy.fallbackProviderIds ?? [],
                },
              })
            }
          />
          <SourceRow
            capability="asr"
            label="ASR"
            source={routing.voiceTranscribe.source}
            providerId={routing.voiceTranscribe.providerId}
            candidates={candidates.asr}
            resolution={resolution}
            onSourceChange={setSource}
            onProviderChange={setProvider}
          />
          <RoutePolicyFields
            value={routing.voiceTranscribe}
            primary={routing.voiceTranscribe.providerId}
            candidates={candidates.asr}
            onChange={(policy) =>
              onRoutingChange({
                ...routing,
                voiceTranscribe: {
                  ...routing.voiceTranscribe,
                  ...policy,
                  fallbackProviderIds: policy.fallbackProviderIds ?? [],
                },
              })
            }
          />
          <SourceRow
            capability="tts"
            label="TTS"
            source={routing.voiceSpeak.source}
            providerId={routing.voiceSpeak.providerId}
            candidates={candidates.tts}
            resolution={resolution}
            onSourceChange={setSource}
            onProviderChange={setProvider}
          />
          <RoutePolicyFields
            value={routing.voiceSpeak}
            primary={routing.voiceSpeak.providerId}
            candidates={candidates.tts}
            onChange={(policy) =>
              onRoutingChange({
                ...routing,
                voiceSpeak: {
                  ...routing.voiceSpeak,
                  ...policy,
                  fallbackProviderIds: policy.fallbackProviderIds ?? [],
                },
              })
            }
          />
        </div>
      </section>
    </div>
  );
}

function SourceRow({
  capability,
  label,
  source,
  providerId,
  candidates,
  resolution,
  onSourceChange,
  onProviderChange,
}: {
  capability: Capability;
  label: string;
  source: "harness" | "provider";
  providerId: string | null;
  candidates: ModelProviderSettings[];
  resolution: HarnessResolution | null;
  onSourceChange: (capability: Capability, source: "harness" | "provider") => void;
  onProviderChange: (capability: Capability, providerId: string) => void;
}) {
  const { t } = useTranslation();
  const status = resolution?.services.find((service) => service.capability === capability);
  const selected = candidates.find((provider) => provider.id === providerId);
  return (
    <div className="provider-card">
      <div className="card-title-row">
        <div>
          <strong>{label}</strong>
          <p className="muted">
            {source === "harness"
              ? (status?.model ??
                status?.voice ??
                (status
                  ? localizeStatus(t, status.state)
                  : t("settings.connection.resolveAfterSave")))
              : selected
                ? localizeProviderLabel(t, selected.label)
                : t("settings.connection.registerProvider")}
          </p>
        </div>
        <span
          className={`provider-test-result ${source === "harness" && status?.state === "ready" ? "success" : ""}`}
        >
          {source === "harness"
            ? localizeStatus(t, status?.state ?? "unchecked")
            : selected?.enabled
              ? t("common.configured")
              : t("common.missing")}
        </span>
      </div>
      <div className="settings-form-grid">
        <Field label={t("settings.connection.source")}>
          <select
            value={source}
            onChange={(event) =>
              onSourceChange(capability, event.target.value as "harness" | "provider")
            }
          >
            <option value="harness">{t("settings.connection.providerHarness")}</option>
            <option value="provider">{t("settings.connection.individualProvider")}</option>
          </select>
        </Field>
        <Field label={t("settings.connection.provider")}>
          <select
            value={providerId ?? ""}
            disabled={source === "harness"}
            onChange={(event) => onProviderChange(capability, event.target.value)}
          >
            <option value="">{t("settings.connection.selectProvider")}</option>
            {candidates.map((provider) => (
              <option key={provider.id} value={provider.id} disabled={!provider.enabled}>
                {localizeProviderLabel(t, provider.label)}
                {provider.enabled ? "" : t("settings.connection.disabledSuffix")}
              </option>
            ))}
          </select>
        </Field>
      </div>
    </div>
  );
}

function isLlmProvider(provider: ModelProviderSettings): boolean {
  return provider.kind === "openai-compatible" || provider.kind === "agent-session";
}
