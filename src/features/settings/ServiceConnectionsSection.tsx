import { useState } from "react";
import type {
  HarnessResolution,
  ModelProviderSettings,
  ModelProvidersSettings,
  RoutingSettings,
} from "../../lib/contracts";
import { resolveServiceHarness } from "../../lib/runtime";
import { legacyDynamicLanHost } from "../../lib/providerRuntime";
import { Field } from "./SettingsFields";

type Capability = "llm" | "asr" | "tts";

export function ServiceConnectionsSection({
  providers,
  routing,
  onProvidersChange,
  onRoutingChange,
}: {
  providers: ModelProvidersSettings;
  routing: RoutingSettings;
  onProvidersChange: (value: ModelProvidersSettings) => void;
  onRoutingChange: (value: RoutingSettings) => void;
}) {
  const [resolution, setResolution] = useState<HarnessResolution | null>(null);
  const [resolveState, setResolveState] = useState<"idle" | "resolving" | "error">("idle");
  const [resolveMessage, setResolveMessage] = useState<string | null>(null);

  const candidates = {
    llm: providers.providers.filter(isLlmProvider),
    asr: providers.providers.filter((provider) => provider.kind === "cloud-asr"),
    tts: providers.providers.filter((provider) => provider.kind === "cloud-tts" || provider.kind === "system-tts"),
  };

  function changeHarnessAddress(address: string) {
    const host = legacyDynamicLanHost(address);
    onProvidersChange({
      ...providers,
      harness: { address },
      providers: providers.providers.map((provider) =>
        provider.kind === "dynamic-lan" && host
          ? { ...provider, enabled: true, host }
          : provider,
      ),
    });
    setResolution(null);
    setResolveState("idle");
    setResolveMessage(null);
  }

  async function resolveHarness() {
    setResolveState("resolving");
    setResolveMessage(null);
    try {
      const next = await resolveServiceHarness(providers.harness.address);
      setResolution(next);
      setResolveState("idle");
      setResolveMessage(next.state === "ready" ? "LLM・ASR・TTSを解決しました。" : "一部のサービスだけを解決しました。");
    } catch (cause) {
      setResolution(null);
      setResolveState("error");
      setResolveMessage(cause instanceof Error ? cause.message : String(cause));
    }
  }

  function setSource(capability: Capability, source: "harness" | "provider") {
    const providerId = source === "provider"
      ? candidates[capability].find((provider) => provider.enabled)?.id ?? null
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
      onRoutingChange({ ...routing, voiceTranscribe: { ...routing.voiceTranscribe, source, providerId } });
    } else {
      onRoutingChange({ ...routing, voiceSpeak: { ...routing.voiceSpeak, source, providerId } });
    }
  }

  function setProvider(capability: Capability, providerId: string) {
    if (capability === "llm") {
      onRoutingChange({
        ...routing,
        conversationRespond: { ...routing.conversationRespond, primaryProviderId: providerId },
      });
    } else if (capability === "asr") {
      onRoutingChange({ ...routing, voiceTranscribe: { ...routing.voiceTranscribe, providerId } });
    } else {
      onRoutingChange({ ...routing, voiceSpeak: { ...routing.voiceSpeak, providerId } });
    }
  }

  return (
    <div className="settings-stack">
      <section className="settings-card">
        <div className="card-title-row">
          <div>
            <p className="eyebrow">PRIMARY CONNECTION</p>
            <h3>LLM Provider harness address</h3>
            <p className="settings-help">一つのアドレスからLLM・ASR・TTSを個別に解決します。</p>
          </div>
          <span className={`provider-test-result ${resolution?.state === "ready" ? "success" : resolveState === "error" ? "error" : ""}`}>
            {resolveState === "resolving" ? "Resolving…" : resolution?.state ?? "unchecked"}
          </span>
        </div>
        <div className="settings-form-grid">
          <Field label="Harness address">
            <input
              value={providers.harness.address}
              placeholder="http://provider.local:9810"
              onChange={(event) => changeHarnessAddress(event.target.value)}
            />
          </Field>
          <Field label="Reasoning effort (LLM)">
            <select
              value={providers.reasoningEffort}
              onChange={(event) => onProvidersChange({
                ...providers,
                reasoningEffort: event.target.value as ModelProvidersSettings["reasoningEffort"],
              })}
            >
              <option value="low">Low</option>
              <option value="medium">Medium (recommended)</option>
              <option value="xhigh">Extra high</option>
            </select>
          </Field>
        </div>
        <div className="provider-card-footer">
          <span>{resolveMessage ?? "接続確認後、各サービスの解決状態を表示します。"}</span>
          <button
            className="text-button"
            type="button"
            disabled={!providers.harness.address || resolveState === "resolving"}
            onClick={() => void resolveHarness()}
          >
            Resolve services
          </button>
        </div>
      </section>

      <section className="settings-card">
        <h3>Service sources</h3>
        <p className="settings-help">LLM・ASR・TTSはそれぞれHarnessまたは個別Providerを選べます。暗黙の切り替えは行いません。</p>
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
  const status = resolution?.services.find((service) => service.capability === capability);
  const selected = candidates.find((provider) => provider.id === providerId);
  return (
    <div className="provider-card">
      <div className="card-title-row">
        <div>
          <strong>{label}</strong>
          <p className="muted">
            {source === "harness"
              ? status?.model ?? status?.voice ?? status?.message ?? "Harnessで保存後に解決"
              : selected?.label ?? "個別Providerを登録してください"}
          </p>
        </div>
        <span className={`provider-test-result ${source === "harness" && status?.state === "ready" ? "success" : ""}`}>
          {source === "harness" ? status?.state ?? "unchecked" : selected?.enabled ? "configured" : "missing"}
        </span>
      </div>
      <div className="settings-form-grid">
        <Field label="Source">
          <select value={source} onChange={(event) => onSourceChange(capability, event.target.value as "harness" | "provider")}>
            <option value="harness">Provider Harness</option>
            <option value="provider">Individual Provider</option>
          </select>
        </Field>
        <Field label="Provider">
          <select
            value={providerId ?? ""}
            disabled={source === "harness"}
            onChange={(event) => onProviderChange(capability, event.target.value)}
          >
            <option value="">Select provider</option>
            {candidates.map((provider) => (
              <option key={provider.id} value={provider.id} disabled={!provider.enabled}>
                {provider.label}{provider.enabled ? "" : " (disabled)"}
              </option>
            ))}
          </select>
        </Field>
      </div>
    </div>
  );
}

function isLlmProvider(provider: ModelProviderSettings): boolean {
  return provider.kind === "openai-compatible" || provider.kind === "larm";
}
