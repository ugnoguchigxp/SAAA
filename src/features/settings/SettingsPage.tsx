import { type ReactNode, useEffect, useMemo, useState } from "react";
import { findSettingsDocument, isCodexAgentSettings, isModelProvidersSettings, isRoutingSettings, isSecuritySettings, isSituationSettings, isVoiceSettings, type CodexAgentSettings, type DynamicLanProviderSettings, type LarmProviderSettings, type LarmRuntimeStatus, type ModelProviderSettings, type ModelProvidersSettings, type OpenAiCompatibleProviderSettings, type RoutingSettings, type SecuritySettings, type SettingsDocument, type SettingsNamespace, type SituationSettings, type SituationSnapshot, type VoiceSettings, type VoiceProfileSnapshot } from "../../lib/contracts";
import { backupDatabase, exportDiagnostics, getSituationSnapshot, resolveNetworkAsr, saveSettingsDocuments, testModelProvider } from "../../lib/runtime";
import { enumerateAudioInputDevices, microphoneErrorMessage } from "../../lib/microphone";
import { DYNAMIC_LAN_MAX_REQUEST_TIMEOUT_MS } from "../../lib/schemas";
import { VoiceProfileCard } from "./VoiceProfileCard";
type SettingsTab = "general" | "providers" | "routing" | "voice" | "situation" | "security";
type SettingsDraft = {
  providers: ModelProvidersSettings;
  codex: CodexAgentSettings;
  routing: RoutingSettings;
  voice: VoiceSettings;
  security: SecuritySettings;
  situation: SituationSettings;
};
const tabs: Array<{ id: SettingsTab; label: string; detail: string }> = [
  { id: "general", label: "General", detail: "Runtime overview" },
  { id: "providers", label: "LLM Providers", detail: "Endpoints and models" },
  { id: "routing", label: "Task Routing", detail: "Primary and fallback" },
  { id: "voice", label: "Voice", detail: "STT, TTS and devices" },
  { id: "situation", label: "Situation", detail: "Shadow observation controls" },
  { id: "security", label: "Privacy & Security", detail: "Local-first controls" },
];
const DYNAMIC_LAN_PROVIDER_ID = "lan-llm-dynamic";
const DEFAULT_DYNAMIC_LAN_HOST = "localhost";
const DEFAULT_AGENT_NAME = "SAAA";
const DEFAULT_USER_NAME = "";
const defaultDraft: SettingsDraft = {
  providers: {
    providers: [
      {
        kind: "dynamic-lan",
        id: DYNAMIC_LAN_PROVIDER_ID,
        enabled: false,
        label: "Dynamic LAN LLM",
        location: "local",
        host: DEFAULT_DYNAMIC_LAN_HOST,
      },
    ],
    reasoningEffort: "medium",
    maxOutputTokens: 2048,
  },
  codex: {
    agentName: DEFAULT_AGENT_NAME,
    userName: DEFAULT_USER_NAME,
    enabled: false,
    provider: "codex-sdk",
    model: "",
    runtimeMode: "app-server",
    health: "unchecked",
    sandboxMode: "read-only",
    approvalPolicy: "never",
    networkEnabled: false,
    webSearchEnabled: false,
    workspacePolicy: "select-per-conversation",
  },
  routing: {
    conversationRespond: {
      primaryProviderId: DYNAMIC_LAN_PROVIDER_ID,
      fallbackProviderIds: [],
      timeoutMs: 30000,
    },
    codingAssist: {
      providerId: "codex-sdk",
      timeoutMs: 120000,
      readOnly: true,
      networkEnabled: false,
      webSearchEnabled: false,
    },
  },
  voice: {
    inputDeviceId: "default",
    outputDeviceId: "default",
    captureMode: "push-to-talk",
    sttHost: DEFAULT_DYNAMIC_LAN_HOST,
    sttProviderId: "network-asr",
    sttModel: "qwen3-asr-1.7b",
    ttsProviderId: "system-tts",
    ttsVoice: "default",
    autoSpeak: true,
    cloudFallbackEnabled: false,
  },
  security: {
    credentialStorage: "environment",
    localOnlyWhenSelected: true,
    diagnosticsRedaction: true,
  },
  situation: {
    enabled: false,
    sampleIntervalMs: 2000,
    calendarEnabled: false,
    retentionDays: 7,
    maxLedgerEntries: 10000,
    heartbeatIntervalMs: 300000,
    sensitiveApplicationCategories: true,
  },
};
export function SettingsPage({ documents, larmRuntime, voiceProfile, voiceEnrollmentBlocked, onSaved, onVoiceProfileChanged }: { documents: SettingsDocument[]; larmRuntime: LarmRuntimeStatus; voiceProfile: VoiceProfileSnapshot; voiceEnrollmentBlocked: boolean; onSaved: (documents: SettingsDocument[]) => void; onVoiceProfileChanged: (profile: VoiceProfileSnapshot) => void }) {
  const source = useMemo(() => draftFromDocuments(documents), [documents]);
  const [draft, setDraft] = useState<SettingsDraft>(source);
  const [activeTab, setActiveTab] = useState<SettingsTab>("general");
  const [saveState, setSaveState] = useState<"idle" | "saving" | "saved" | "error">("idle");
  const [saveMessage, setSaveMessage] = useState<string | null>(null);
  useEffect(() => {
    setDraft(source);
    setSaveState("idle");
    setSaveMessage(null);
  }, [source]);
  const dirty = JSON.stringify(draft) !== JSON.stringify(source);
  const activeTabMeta = tabs.find((tab) => tab.id === activeTab) ?? tabs[0];
  async function save() {
    try {
      setSaveState("saving");
      setSaveMessage(null);
      const saved = await saveSettingsDocuments(documentsFromDraft(draft));
      onSaved(saved);
      setSaveState("saved");
      setSaveMessage(`SQLiteへ保存しました · ${new Date().toLocaleTimeString()}`);
    } catch (cause) {
      setSaveState("error");
      setSaveMessage(cause instanceof Error ? cause.message : String(cause));
    }
  }
  return ( <section className="settings-page"> <header className="settings-page-header"> <div> <p className="eyebrow">SETTINGS</p> <h1>Runtime settings</h1> <p>保存済みの設定を編集し、Save Settingsで一括検証・SQLite保存します。</p> </div> <div className="settings-save-status" aria-live="polite"> {dirty && saveState === "idle" && <span className="unsaved">Unsaved changes</span>} {saveMessage && <span className={saveState === "error" ? "save-error" : "save-success"}>{saveMessage}</span>} </div> </header> <div className="settings-screen-layout"> <nav className="settings-menu" aria-label="Settings sections"> {tabs.map((tab) => ( <button className={tab.id === activeTab ? "settings-menu-item active" : "settings-menu-item"} key={tab.id} onClick={() => setActiveTab(tab.id)}> <strong>{tab.label}</strong> <span>{tab.detail}</span> </button> ))} </nav> <div className="settings-content"> <header className="settings-content-header"> <h2>{activeTabMeta.label}</h2> <p>{activeTabMeta.detail}</p> </header> {activeTab === "general" && ( <GeneralSection draft={draft} onAgentNameChange={(agentName) => setDraft((current) => ({ ...current, codex: { ...current.codex, agentName }, })) } onUserNameChange={(userName) => setDraft((current) => ({ ...current, codex: { ...current.codex, userName }, })) } /> )} {activeTab === "providers" && <ProvidersSection providers={draft.providers} larmRuntime={larmRuntime} onChange={(providers) => setDraft((current) => ({ ...current, providers }))} />} {activeTab === "routing" && <RoutingSection routing={draft.routing} providers={draft.providers.providers} localOnlyWhenSelected={draft.security.localOnlyWhenSelected} onChange={(routing) => setDraft((current) => ({ ...current, routing }))} />} {activeTab === "voice" && <VoiceSection voice={draft.voice} profile={voiceProfile} enrollmentBlocked={voiceEnrollmentBlocked} onProfileChanged={onVoiceProfileChanged} onChange={(voice) => setDraft((current) => ({ ...current, voice }))} />} {activeTab === "situation" && <SituationSection situation={draft.situation} onChange={(situation) => setDraft((current) => ({ ...current, situation }))} />} {activeTab === "security" && <SecuritySection security={draft.security} onChange={(security) => setDraft((current) => ({ ...current, security }))} />} </div> </div> <footer className="settings-save-bar"> <p>{dirty ? "変更はまだRuntimeへ反映されていません。" : "保存済みのRuntime設定を表示しています。"}</p> <div> <button className="discard-button" onClick={() => setDraft(source)} disabled={!dirty || saveState === "saving"}> Discard </button> <button className="save-button" onClick={() => void save()} disabled={!dirty || saveState === "saving"}> {saveState === "saving" ? "Saving…" : "Save Settings"} </button> </div> </footer> </section> );
}
function GeneralSection({ draft, onAgentNameChange, onUserNameChange }: { draft: SettingsDraft; onAgentNameChange: (agentName: string) => void; onUserNameChange: (userName: string) => void }) {
  const enabledProviders = draft.providers.providers.filter((provider) => provider.enabled).length;
  return ( <div className="settings-stack"> <section className="settings-card"> <h3>Conversation identity</h3> <div className="settings-form-grid"> <Field label="Agent name"> <input value={draft.codex.agentName} maxLength={80} placeholder={DEFAULT_AGENT_NAME} onChange={(event) => onAgentNameChange(event.target.value)} /> </Field> <Field label="User name"> <input value={draft.codex.userName} maxLength={80} placeholder="未設定（名前で呼ばない）" onChange={(event) => onUserNameChange(event.target.value)} /> </Field> </div> <p className="settings-help">保存した名前を会話のSystem Contextへ反映します。User nameが未設定の場合、エージェントは名前を推測せず、名前で呼びかけません。</p> </section> <section className="settings-card"> <h3>Runtime state</h3> <div className="settings-summary-grid"> <Metric label="LLM providers" value={`${enabledProviders} enabled`} /> <Metric label="Conversation route" value={draft.routing.conversationRespond.primaryProviderId} /> <Metric label="Voice input" value={draft.voice.captureMode} /> <Metric label="Situation" value={draft.situation.enabled ? "shadow monitoring" : "paused"} /> </div> </section> <section className="settings-card"> <h3>Persistence</h3> <p>設定、Conversation、Message、boundedなSituation履歴はSAAA所有のSQLiteに保存されます。API keyはSQLiteに複製しません。</p> </section> </div> );
}
function SituationSection({ situation, onChange }: { situation: SituationSettings; onChange: (value: SituationSettings) => void }) {
  const [snapshot, setSnapshot] = useState<SituationSnapshot | null>(null);
  useEffect(() => {
    let active = true;
    void getSituationSnapshot()
      .then((value) => {
        if (active) setSnapshot(value);
      })
      .catch(() => undefined);
    return () => {
      active = false;
    };
  }, []);
  const calendarHealth = snapshot?.signals.calendar.health ?? (situation.calendarEnabled ? "checking" : "disabled");
  return ( <div className="settings-stack"> <section className="settings-card"> <div className="card-title-row"> <div> <h3>Situation Shadow Mode</h3> <p className="settings-help">Hard Signalから候補と介入案を記録します。Model、TTS、通知、外部操作は自動実行しません。</p> </div> <label className="toggle"> <input type="checkbox" checked={situation.enabled} onChange={(event) => onChange({ ...situation, enabled: event.target.checked })} /> <span /> </label> </div> <div className="locked-policy">Shadow only · Execution NONE · Presentation SILENT</div> </section> <section className="settings-card"> <h3>Signals</h3> <label className="check-row"> <input type="checkbox" checked={situation.calendarEnabled} onChange={(event) => onChange({ ...situation, calendarEnabled: event.target.checked })} /> Calendarのcoarse busy signalを有効化する </label> <div className="settings-summary-grid"> <Metric label="Sampling" value={`${situation.sampleIntervalMs} ms`} /> <Metric label="Calendar health" value={calendarHealth} /> <Metric label="Raw window title" value="never collected" /> <Metric label="Audio content" value="never stored" /> </div> <p className="settings-help">Calendar adapterが未対応またはpermission拒否でも、Foreground categoryとSAAA自身のlifecycleだけでdegraded動作します。</p> </section> <section className="settings-card"> <h3>Retention</h3> <div className="settings-form-grid"> <Field label="History retention (days)"> <input type="number" min="1" max="30" value={situation.retentionDays} onChange={(event) => onChange({ ...situation, retentionDays: Math.max(1, Math.min(30, Number(event.target.value) || 7)), }) } /> </Field> <Field label="Maximum entries"> <input value={situation.maxLedgerEntries} disabled /> </Field> </div> <p className="settings-help">状態遷移、decision変更、5分heartbeatだけを保存します。samplingごとのraw signalは保存しません。</p> </section> </div> );
}
function ProvidersSection({ providers, larmRuntime, onChange }: { providers: ModelProvidersSettings; larmRuntime: LarmRuntimeStatus; onChange: (value: ModelProvidersSettings) => void }) {
  const [tests, setTests] = useState<Record<string, { state: "testing" | "success" | "error"; message: string }>>({});
  function replace(index: number, next: ModelProviderSettings) {
    onChange({
      ...providers,
      providers: providers.providers.map((provider, providerIndex) => (providerIndex === index ? next : provider)),
    });
  }
  function remove(index: number) {
    onChange({
      ...providers,
      providers: providers.providers.filter((_, providerIndex) => providerIndex !== index),
    });
  }
  function addOpenAiCompatible() {
    const number = providers.providers.length + 1;
    const provider: OpenAiCompatibleProviderSettings = {
      kind: "openai-compatible",
      id: `provider-${Date.now()}`,
      enabled: false,
      label: `Provider ${number}`,
      location: "local",
      endpoint: "",
      model: "",
      credentialStatus: "not-configured",
    };
    onChange({ ...providers, providers: [...providers.providers, provider] });
  }
  function addLarm() {
    const provider: LarmProviderSettings = {
      kind: "larm",
      id: `larm-${Date.now()}`,
      enabled: false,
      label: "LARM",
      location: "local",
      baseUrl: "http://127.0.0.1:9810",
      tokenEnv: "LARM_API_TOKEN",
      allocationTtlSeconds: 300,
      allocationStartupTimeoutSeconds: 300,
      allowFallbackByDefault: false,
      deploymentPolicy: "existing-only",
    };
    onChange({ ...providers, providers: [...providers.providers, provider] });
  }
  function addDynamicLan() {
    const provider: DynamicLanProviderSettings = {
      kind: "dynamic-lan",
      id: `dynamic-lan-${Date.now()}`,
      enabled: false,
      label: "Dynamic LAN LLM",
      location: "local",
      host: DEFAULT_DYNAMIC_LAN_HOST,
    };
    onChange({ ...providers, providers: [...providers.providers, provider] });
  }
  async function test(provider: ModelProviderSettings) {
    setTests((current) => ({
      ...current,
      [provider.id]: { state: "testing", message: "Connecting…" },
    }));
    try {
      const result = await testModelProvider(provider);
      setTests((current) => ({
        ...current,
        [provider.id]: {
          state: result.ok ? "success" : "error",
          message: `${result.message} · ${result.latencyMs} ms`,
        },
      }));
    } catch (cause) {
      setTests((current) => ({
        ...current,
        [provider.id]: {
          state: "error",
          message: cause instanceof Error ? cause.message : String(cause),
        },
      }));
    }
  }
  const hasLarm = providers.providers.some((provider) => provider.kind === "larm");
  const hasDynamicLan = providers.providers.some((provider) => provider.kind === "dynamic-lan");
  return ( <div className="settings-stack"> <p className="settings-help">Dynamic LAN Providerはhostだけを保存し、モデル・Gateway URL・短期credentialを接続APIから動的に解決します。credential値はSQLiteへ保存しません。</p> <section className="settings-card"> <h3>Generation</h3> <div className="settings-form-grid"> <Field label="Reasoning effort"> <select value={providers.reasoningEffort} onChange={(event) => onChange({ ...providers, reasoningEffort: event.target.value as ModelProvidersSettings["reasoningEffort"], }) } > <option value="low">Low</option> <option value="medium">Medium (recommended)</option> <option value="xhigh">Extra high</option> </select> </Field> <Field label="Maximum output tokens"> <input type="number" min="256" max="8192" step="256" value={providers.maxOutputTokens} onChange={(event) => onChange({ ...providers, maxOutputTokens: Math.max(256, Math.min(8192, Number(event.target.value) || 2048)), }) } /> </Field> </div> <p className="settings-help">通常会話で使用する思考量と最大出力量です。Providerを切り替えた場合も同じ値を使用します。`length` による打ち切りは成功扱いにしません。</p> </section> {providers.providers.map((provider, index) => ( <section className="settings-card provider-card" key={provider.id}> <div className="card-title-row"> <div> <h3>{provider.label || `Provider ${index + 1}`}</h3> <p className="muted"> ID: {provider.id} · {provider.kind} </p> </div> <label className="toggle"> <input type="checkbox" checked={provider.enabled} onChange={(event) => replace(index, { ...provider, enabled: event.target.checked })} /> <span /> </label> </div> {provider.kind === "openai-compatible" ? ( <> <div className="settings-form-grid"> <Field label="Display name"> <input value={provider.label} onChange={(event) => replace(index, { ...provider, label: event.target.value })} /> </Field> <Field label="Location"> <select value={provider.location} onChange={(event) => replace(index, { ...provider, location: event.target.value as OpenAiCompatibleProviderSettings["location"], }) } > <option value="local">Local</option> <option value="cloud">Cloud</option> </select> </Field> <Field label="Endpoint"> <input value={provider.endpoint} placeholder="http://localhost:11434/v1" onChange={(event) => replace(index, { ...provider, endpoint: event.target.value, }) } /> </Field> <Field label="Model"> <input value={provider.model} placeholder="model name" onChange={(event) => replace(index, { ...provider, model: event.target.value })} /> </Field> </div> {tests[provider.id] && <p className={`provider-test-result ${tests[provider.id].state}`}>{tests[provider.id].message}</p>} <div className="provider-card-footer"> <span>Credential: environment</span> <div> <button className="text-button" type="button" onClick={() => void test(provider)} disabled={!provider.endpoint || !provider.model || tests[provider.id]?.state === "testing"}> Test connection </button> <button className="text-button danger" type="button" onClick={() => remove(index)} disabled={providers.providers.length === 1}> Remove provider </button> </div> </div> </> ) : provider.kind === "dynamic-lan" ? ( <> <div className="settings-form-grid"> <Field label="Display name"> <input value={provider.label} onChange={(event) => replace(index, { ...provider, label: event.target.value })} /> </Field> <Field label="LLM Provider host IP address"> <input value={provider.host} placeholder="10.0.0.42" aria-describedby={`${provider.id}-host-help`} onChange={(event) => replace(index, { ...provider, host: event.target.value })} /> </Field> <Field label="Configuration API"> <input value={`http://${provider.host || "<host>"}:9810`} disabled /> </Field> <Field label="Credential source"> <input value="LARM_API_TOKEN" disabled /> </Field> </div> <p className="settings-help" id={`${provider.id}-host-help`}> プライベートIPアドレス、.local名、またはLAN内ホスト名だけを入力します。URL、ポート、モデル名は入力不要です。 </p> <div className="locked-policy">Profile: deep-reasoning-35b · Audience: saaa-desktop · Endpoint, model, semantic health and short-lived key are resolved over REST</div> {tests[provider.id] && <p className={`provider-test-result ${tests[provider.id].state}`}>{tests[provider.id].message}</p>} <div className="provider-card-footer"> <span>保存対象はhostのみ</span> <div> <button className="text-button" type="button" onClick={() => void test(provider)} disabled={!provider.host || tests[provider.id]?.state === "testing"}> Resolve &amp; test </button> <button className="text-button danger" type="button" onClick={() => remove(index)} disabled={providers.providers.length === 1}> Remove provider </button> </div> </div> </> ) : ( <> <div className="settings-form-grid"> <Field label="Display name"> <input value={provider.label} onChange={(event) => replace(index, { ...provider, label: event.target.value })} /> </Field> <Field label="Location"> <input value="Local" disabled /> </Field> <Field label="Base URL"> <input value={provider.baseUrl} onChange={(event) => replace(index, { ...provider, baseUrl: event.target.value, }) } /> </Field> <Field label="Credential source"> <input value={provider.tokenEnv} disabled /> </Field> <Field label="Allocation TTL (seconds)"> <input type="number" min="60" max="3600" value={provider.allocationTtlSeconds} onChange={(event) => replace(index, { ...provider, allocationTtlSeconds: Math.max(60, Math.min(3600, Number(event.target.value) || 300)), }) } /> </Field> <Field label="Startup timeout (seconds)"> <input type="number" min="1" max="300" value={provider.allocationStartupTimeoutSeconds} onChange={(event) => replace(index, { ...provider, allocationStartupTimeoutSeconds: Math.max(1, Math.min(300, Number(event.target.value) || 300)), }) } /> </Field> </div> <div className="locked-policy">Fallback disabled · Existing deployments only · Runtime selection owned by LARM</div> {tests[provider.id] && <p className={`provider-test-result ${tests[provider.id].state}`}>{tests[provider.id].message}</p>} <div className="provider-card-footer"> <span> Feature flag: {larmRuntime.state} · contract {larmRuntime.contractCommit}. {larmRuntime.message} </span> <div> <button className="text-button" type="button" onClick={() => void test(provider)} disabled={larmRuntime.state !== "ready" || !provider.baseUrl || tests[provider.id]?.state === "testing"}> Test health &amp; ready </button> <button className="text-button danger" type="button" onClick={() => remove(index)} disabled={providers.providers.length === 1}> Remove provider </button> </div> </div> </> )} </section> ))} <div className="provider-card-footer"> <button className="add-provider-button" type="button" onClick={addOpenAiCompatible}> ＋ Add provider </button> <div> <button className="add-provider-button" type="button" onClick={addDynamicLan} disabled={hasDynamicLan}> ＋ Add dynamic LAN </button> <button className="add-provider-button" type="button" onClick={addLarm} disabled={hasLarm}> ＋ Add LARM </button> </div> </div> </div> );
}
function RoutingSection({ routing, providers, localOnlyWhenSelected, onChange }: { routing: RoutingSettings; providers: ModelProviderSettings[]; localOnlyWhenSelected: boolean; onChange: (value: RoutingSettings) => void }) {
  const selectable = providers.filter((provider) => provider.enabled);
  const fallbackIds = routing.conversationRespond.fallbackProviderIds;
  const primary = providers.find((provider) => provider.id === routing.conversationRespond.primaryProviderId);
  const timeoutMaximum = primary?.kind === "dynamic-lan" ? DYNAMIC_LAN_MAX_REQUEST_TIMEOUT_MS : 300_000;
  const effectiveFallbackIds = fallbackIds.filter((id) => !(localOnlyWhenSelected && primary?.location === "local" && providers.find((provider) => provider.id === id)?.location === "cloud"));
  const blockedFallbacks = fallbackIds.filter((id) => !effectiveFallbackIds.includes(id));
  return ( <div className="settings-stack"> <section className="settings-card"> <h3>conversation.respond</h3> <p className="settings-help">通常ChatとVoiceの確定文字起こしに使うModel Routeです。</p> <div className="settings-form-grid"> <Field label="Primary provider"> <select value={routing.conversationRespond.primaryProviderId} onChange={(event) => onChange({ ...routing, conversationRespond: { ...routing.conversationRespond, primaryProviderId: event.target.value, }, }) } > {providers.map((provider) => ( <option key={provider.id} value={provider.id}> {provider.label || provider.id} {provider.enabled ? "" : " (disabled)"} </option> ))} </select> </Field> <Field label="Timeout (ms)"> <input type="number" min="1000" max={timeoutMaximum} value={routing.conversationRespond.timeoutMs} onChange={(event) => onChange({ ...routing, conversationRespond: { ...routing.conversationRespond, timeoutMs: clampTimeout(event.target.value, timeoutMaximum), }, }) } /> </Field> </div> <div className="fallback-list"> <strong>Fallback providers</strong> {selectable .filter((provider) => provider.id !== routing.conversationRespond.primaryProviderId) .map((provider) => ( <label className="check-row" key={provider.id}> <input type="checkbox" checked={fallbackIds.includes(provider.id)} onChange={(event) => onChange({ ...routing, conversationRespond: { ...routing.conversationRespond, fallbackProviderIds: event.target.checked ? [...fallbackIds, provider.id] : fallbackIds.filter((id) => id !== provider.id), }, }) } /> {provider.label || provider.id} </label> ))} {selectable.length < 2 && <p className="muted">Fallbackを使うには、もう一つ有効なProviderを登録してください。</p>} {blockedFallbacks.length > 0 && <p className="provider-test-result error">Local-only policy blocks Cloud fallback: {blockedFallbacks.join(", ")}</p>} </div> <div className="effective-route"> <span>Effective route</span> <strong>{[routing.conversationRespond.primaryProviderId, ...effectiveFallbackIds].join(" → ")}</strong> </div> </section> </div> );
}
function VoiceSection({ voice, profile, enrollmentBlocked, onProfileChanged, onChange }: { voice: VoiceSettings; profile: VoiceProfileSnapshot; enrollmentBlocked: boolean; onProfileChanged: (profile: VoiceProfileSnapshot) => void; onChange: (value: VoiceSettings) => void }) {
  const [devices, setDevices] = useState<MediaDeviceInfo[]>([]);
  const [deviceError, setDeviceError] = useState<string | null>(null);
  const [asrState, setAsrState] = useState<{
    state: "idle" | "resolving" | "success" | "error";
    message: string;
    endpoint: string | null;
  }>({ state: "idle", message: "", endpoint: null });
  const sttHost = voice.sttHost.trim();
  useEffect(() => {
    let active = true;
    void enumerateAudioInputDevices()
      .then((available) => {
        if (active) setDevices(available);
      })
      .catch((cause) => {
        if (active) setDeviceError(microphoneErrorMessage(cause));
      });
    return () => {
      active = false;
    };
  }, []);
  useEffect(() => {
    if (!sttHost) {
      setAsrState({
        state: "error",
        message: "ASR hostを設定してください。",
        endpoint: null,
      });
      return;
    }
    let active = true;
    setAsrState({
      state: "resolving",
      message: "ASR設定を問い合わせています…",
      endpoint: null,
    });
    const timer = window.setTimeout(
      () =>
        void resolveNetworkAsr(sttHost)
          .then((resolution) => {
            if (!active) return;
            if (resolution.model !== voice.sttModel)
              onChange({
                ...voice,
                sttProviderId: resolution.providerId,
                sttModel: resolution.model,
              });
            setAsrState({
              state: "success",
              message: `Resolved ${resolution.model}`,
              endpoint: resolution.endpoint,
            });
          })
          .catch((cause) => {
            if (active)
              setAsrState({
                state: "error",
                message: cause instanceof Error ? cause.message : String(cause),
                endpoint: null,
              });
          }),
      350,
    );
    return () => {
      active = false;
      window.clearTimeout(timer);
    };
    // Resolution is keyed by the independent ASR host. Voice changes are applied from the response.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [sttHost]);
  async function retryAsrResolution() {
    if (!sttHost) return;
    setAsrState({
      state: "resolving",
      message: "ASR設定を問い合わせています…",
      endpoint: null,
    });
    try {
      const resolution = await resolveNetworkAsr(sttHost);
      onChange({
        ...voice,
        sttProviderId: resolution.providerId,
        sttModel: resolution.model,
      });
      setAsrState({
        state: "success",
        message: `Resolved ${resolution.model}`,
        endpoint: resolution.endpoint,
      });
    } catch (cause) {
      setAsrState({
        state: "error",
        message: cause instanceof Error ? cause.message : String(cause),
        endpoint: null,
      });
    }
  }
  const currentDeviceMissing = voice.inputDeviceId !== "default" && !devices.some((device) => device.deviceId === voice.inputDeviceId);
  return ( <div className="settings-stack"> <section className="settings-card"> <h3>Capture</h3> <div className="settings-form-grid"> <Field label="Capture mode"> <select value={voice.captureMode} disabled> <option value="push-to-talk">Push-to-talk</option> </select> </Field> <Field label="Input device"> <select value={voice.inputDeviceId} onChange={(event) => onChange({ ...voice, inputDeviceId: event.target.value })}> <option value="default">System default</option> {currentDeviceMissing && <option value={voice.inputDeviceId}>{voice.inputDeviceId} (unavailable)</option>} {devices.map((device, index) => ( <option key={device.deviceId} value={device.deviceId}> {device.label || `Microphone ${index + 1}`} </option> ))} </select> </Field> <Field label="ASR host"> <input value={voice.sttHost} placeholder="localhost or 10.0.0.42" onChange={(event) => onChange({ ...voice, sttHost: event.target.value })} /> </Field> <Field label="ASR endpoint"> <input value={asrState.endpoint ?? (sttHost ? `http://${sttHost}:8081` : "Not configured")} disabled /> </Field> <Field label="STT provider"> <input value={voice.sttProviderId} disabled /> </Field> <Field label="STT model"> <input value={voice.sttModel} disabled /> </Field> </div> {deviceError && <p className="provider-test-result error">Microphone devices unavailable: {deviceError}</p>} <p className={`provider-test-result ${asrState.state === "success" ? "success" : asrState.state === "error" ? "error" : "testing"}`} aria-live="polite"> {asrState.message} </p> <div className="provider-card-footer"> <span>ASR hostはLLMの接続先から独立しています。endpointとmodelはASR APIから検証し、成功時は実行時キャッシュを更新します。</span> <button className="text-button" type="button" onClick={() => void retryAsrResolution()} disabled={!sttHost || asrState.state === "resolving"}> Resolve again </button> </div> </section> <VoiceProfileCard voice={voice} profile={profile} blocked={enrollmentBlocked} onChanged={onProfileChanged} /> <section className="settings-card"> <h3>Speech output</h3> <div className="settings-form-grid"> <Field label="Output device"> <input value="System default (fixed)" disabled /> </Field> <Field label="TTS provider"> <input value={voice.ttsProviderId} disabled /> </Field> <Field label="Voice"> <input value={voice.ttsVoice} onChange={(event) => onChange({ ...voice, ttsVoice: event.target.value })} /> </Field> <Field label="Auto speak"> <select value={voice.autoSpeak ? "on" : "off"} onChange={(event) => onChange({ ...voice, autoSpeak: event.target.value === "on" })}> <option value="on">On</option> <option value="off">Off</option> </select> </Field> </div> <div className="locked-policy">URLと絵文字は読み上げから除外します。Cloud fallback: disabled.</div> </section> </div> );
}
function SecuritySection({ security, onChange }: { security: SecuritySettings; onChange: (value: SecuritySettings) => void }) {
  const [artifactState, setArtifactState] = useState<"idle" | "working" | "success" | "error">("idle");
  const [artifactMessage, setArtifactMessage] = useState<string | null>(null);
  async function createArtifact(kind: "diagnostics" | "backup") {
    try {
      setArtifactState("working");
      setArtifactMessage(null);
      const result = kind === "diagnostics" ? await exportDiagnostics() : await backupDatabase();
      setArtifactState("success");
      setArtifactMessage(`${kind === "diagnostics" ? "Diagnostics" : "Database backup"} created: ${result.path}`);
    } catch (cause) {
      setArtifactState("error");
      setArtifactMessage(cause instanceof Error ? cause.message : String(cause));
    }
  }
  return ( <div className="settings-stack"> <section className="settings-card"> <h3>Credential handling</h3> <p className="settings-help">Model API keyは環境変数からだけ読み込み、SQLiteやDiagnosticsへ保存しません。</p> <Field label="Credential store"> <input value={security.credentialStorage} disabled /> </Field> </section> <section className="settings-card"> <h3>Privacy defaults</h3> <label className="check-row"> <input type="checkbox" checked={security.localOnlyWhenSelected} onChange={(event) => onChange({ ...security, localOnlyWhenSelected: event.target.checked, }) } /> Local routeを選択している場合はCloudへ自動fallbackしない </label> <label className="check-row"> <input type="checkbox" checked={security.diagnosticsRedaction} onChange={(event) => onChange({ ...security, diagnosticsRedaction: event.target.checked, }) } disabled /> DiagnosticsとProvider activityからsecretをredactする（固定） </label> </section> <section className="settings-card"> <h3>Recovery & diagnostics</h3> <p className="settings-help">Diagnosticsには会話本文、ローカルpath、credentialを含めません。Backupは整合性のあるSQLite snapshotですが、登録音声fileとKeychain keyは含まないため、声profile単体では復元できません。</p> <div className="artifact-actions"> <button className="secondary-button" type="button" onClick={() => void createArtifact("diagnostics")} disabled={artifactState === "working"}> Export diagnostics </button> <button className="secondary-button" type="button" onClick={() => void createArtifact("backup")} disabled={artifactState === "working"}> Backup database </button> </div> {artifactMessage && ( <p className={`provider-test-result ${artifactState === "success" ? "success" : "error"}`} aria-live="polite"> {artifactMessage} </p> )} </section> </div> );
}
function Field({ label, children }: { label: string; children: ReactNode }) {
  return ( <label className="settings-field"> <span>{label}</span> {children} </label> );
}
function Metric({ label, value }: { label: string; value: string }) {
  return ( <div> <span>{label}</span> <strong>{value}</strong> </div> );
}
function clampTimeout(value: string, maximum: number): number {
  return Math.max(1000, Math.min(maximum, Number(value) || 30000));
}
function draftFromDocuments(documents: SettingsDocument[]): SettingsDraft {
  const model = findSettingsDocument(documents, "providers.model", "default");
  const codex = findSettingsDocument(documents, "providers.agent", "codex-sdk");
  const routing = findSettingsDocument(documents, "routing.tasks", "default");
  const voice = findSettingsDocument(documents, "voice.runtime", "default");
  const security = findSettingsDocument(documents, "security.runtime", "default");
  const situation = findSettingsDocument(documents, "situation.runtime", "default");
  return {
    providers: model && isModelProvidersSettings(model.valueJson) ? model.valueJson : defaultDraft.providers,
    codex: codex && isCodexAgentSettings(codex.valueJson) ? codex.valueJson : defaultDraft.codex,
    routing: routing && isRoutingSettings(routing.valueJson) ? routing.valueJson : defaultDraft.routing,
    voice: voice && isVoiceSettings(voice.valueJson) ? voice.valueJson : defaultDraft.voice,
    security: security && isSecuritySettings(security.valueJson) ? security.valueJson : defaultDraft.security,
    situation: situation && isSituationSettings(situation.valueJson) ? situation.valueJson : defaultDraft.situation,
  };
}
function documentsFromDraft(draft: SettingsDraft): Array<Omit<SettingsDocument, "updatedAt">> {
  return [
    document("providers.model", "default", draft.providers),
    document("providers.agent", "codex-sdk", {
      ...draft.codex,
      agentName: draft.codex.agentName.trim(),
      userName: draft.codex.userName.trim(),
    }),
    document("routing.tasks", "default", draft.routing),
    document("voice.runtime", "default", draft.voice),
    document("security.runtime", "default", draft.security),
    document("situation.runtime", "default", draft.situation),
  ];
}
function document(namespace: SettingsNamespace, key: "default" | "codex-sdk", valueJson: Record<string, unknown>): Omit<SettingsDocument, "updatedAt"> {
  return { namespace, key, schemaVersion: 10, valueJson };
}
