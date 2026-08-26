import { type ReactNode, useEffect, useMemo, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import {
  findSettingsDocument,
  isCodexAgentSettings,
  isModelProvidersSettings,
  isRoutingSettings,
  isSecuritySettings,
  isSituationSettings,
  isVoiceSettings,
  type CodexAgentSettings,
  type CodexModelOption,
  type CodexRuntimeStatus,
  type ModelProviderSettings,
  type ModelProvidersSettings,
  type RoutingSettings,
  type SecuritySettings,
  type SettingsDocument,
  type SettingsNamespace,
  type SituationSettings,
  type SituationSnapshot,
  type VoiceSettings,
} from "../../lib/contracts";
import { backupDatabase, exportDiagnostics, getCodexStatus, getSituationSnapshot, listCodexModels, saveSettingsDocuments, testModelProvider } from "../../lib/runtime";

type SettingsTab = "general" | "providers" | "routing" | "voice" | "codex" | "situation" | "security";

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
  { id: "codex", label: "Codex SDK", detail: "Read-only agent route" },
  { id: "situation", label: "Situation", detail: "Shadow observation controls" },
  { id: "security", label: "Privacy & Security", detail: "Local-first controls" },
];

const defaultDraft: SettingsDraft = {
  providers: {
    providers: [{
      id: "local-openai-compatible",
      enabled: false,
      label: "Local OpenAI-compatible",
      location: "local",
      endpoint: "",
      model: "",
      credentialStatus: "not-configured",
    }],
  },
  codex: {
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
      primaryProviderId: "local-openai-compatible",
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
    sttProviderId: "local-whisper",
    sttModel: "",
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

export function SettingsPage({
  documents,
  onSaved,
}: {
  documents: SettingsDocument[];
  onSaved: (documents: SettingsDocument[]) => void;
}) {
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

  return (
    <section className="settings-page">
      <header className="settings-page-header">
        <div>
          <p className="eyebrow">SETTINGS</p>
          <h1>Runtime settings</h1>
          <p>保存済みの設定を編集し、Save Settingsで一括検証・SQLite保存します。</p>
        </div>
        <div className="settings-save-status" aria-live="polite">
          {dirty && saveState === "idle" && <span className="unsaved">Unsaved changes</span>}
          {saveMessage && <span className={saveState === "error" ? "save-error" : "save-success"}>{saveMessage}</span>}
        </div>
      </header>

      <div className="settings-screen-layout">
        <nav className="settings-menu" aria-label="Settings sections">
          {tabs.map((tab) => (
            <button
              className={tab.id === activeTab ? "settings-menu-item active" : "settings-menu-item"}
              key={tab.id}
              onClick={() => setActiveTab(tab.id)}
            >
              <strong>{tab.label}</strong><span>{tab.detail}</span>
            </button>
          ))}
        </nav>

        <div className="settings-content">
          <header className="settings-content-header"><h2>{activeTabMeta.label}</h2><p>{activeTabMeta.detail}</p></header>
          {activeTab === "general" && <GeneralSection draft={draft} />}
          {activeTab === "providers" && <ProvidersSection providers={draft.providers} onChange={(providers) => setDraft((current) => ({ ...current, providers }))} />}
          {activeTab === "routing" && <RoutingSection routing={draft.routing} providers={draft.providers.providers} localOnlyWhenSelected={draft.security.localOnlyWhenSelected} onChange={(routing) => setDraft((current) => ({ ...current, routing }))} />}
          {activeTab === "voice" && <VoiceSection voice={draft.voice} onChange={(voice) => setDraft((current) => ({ ...current, voice }))} />}
          {activeTab === "codex" && <CodexSection codex={draft.codex} onChange={(codex) => setDraft((current) => ({ ...current, codex }))} />}
          {activeTab === "situation" && <SituationSection situation={draft.situation} onChange={(situation) => setDraft((current) => ({ ...current, situation }))} />}
          {activeTab === "security" && <SecuritySection security={draft.security} onChange={(security) => setDraft((current) => ({ ...current, security }))} />}
        </div>
      </div>

      <footer className="settings-save-bar">
        <p>{dirty ? "変更はまだRuntimeへ反映されていません。" : "保存済みのRuntime設定を表示しています。"}</p>
        <div>
          <button className="discard-button" onClick={() => setDraft(source)} disabled={!dirty || saveState === "saving"}>Discard</button>
          <button className="save-button" onClick={() => void save()} disabled={!dirty || saveState === "saving"}>{saveState === "saving" ? "Saving…" : "Save Settings"}</button>
        </div>
      </footer>
    </section>
  );
}

function GeneralSection({ draft }: { draft: SettingsDraft }) {
  const enabledProviders = draft.providers.providers.filter((provider) => provider.enabled).length;
  return <div className="settings-stack"><section className="settings-card"><h3>Runtime state</h3><div className="settings-summary-grid"><Metric label="LLM providers" value={`${enabledProviders} enabled`} /><Metric label="Conversation route" value={draft.routing.conversationRespond.primaryProviderId} /><Metric label="Voice input" value={draft.voice.captureMode} /><Metric label="Codex route" value={draft.codex.enabled ? "enabled · read-only" : "disabled"} /><Metric label="Situation" value={draft.situation.enabled ? "shadow monitoring" : "paused"} /></div></section><section className="settings-card"><h3>Persistence</h3><p>設定、Conversation、Message、boundedなSituation履歴はSAAA所有のSQLiteに保存されます。API keyやCodex認証本文はSQLiteに複製しません。</p></section></div>;
}

function SituationSection({ situation, onChange }: { situation: SituationSettings; onChange: (value: SituationSettings) => void }) {
  const [snapshot, setSnapshot] = useState<SituationSnapshot | null>(null);
  useEffect(() => {
    let active = true;
    void getSituationSnapshot().then((value) => { if (active) setSnapshot(value); }).catch(() => undefined);
    return () => { active = false; };
  }, []);
  const calendarHealth = snapshot?.signals.calendar.health ?? (situation.calendarEnabled ? "checking" : "disabled");
  return <div className="settings-stack"><section className="settings-card"><div className="card-title-row"><div><h3>Situation Shadow Mode</h3><p className="settings-help">Hard Signalから候補と介入案を記録します。Model、TTS、通知、外部操作は自動実行しません。</p></div><label className="toggle"><input type="checkbox" checked={situation.enabled} onChange={(event) => onChange({ ...situation, enabled: event.target.checked })} /><span /></label></div><div className="locked-policy">Shadow only · Execution NONE · Presentation SILENT</div></section><section className="settings-card"><h3>Signals</h3><label className="check-row"><input type="checkbox" checked={situation.calendarEnabled} onChange={(event) => onChange({ ...situation, calendarEnabled: event.target.checked })} />Calendarのcoarse busy signalを有効化する</label><div className="settings-summary-grid"><Metric label="Sampling" value={`${situation.sampleIntervalMs} ms`} /><Metric label="Calendar health" value={calendarHealth} /><Metric label="Raw window title" value="never collected" /><Metric label="Audio content" value="never stored" /></div><p className="settings-help">Calendar adapterが未対応またはpermission拒否でも、Foreground categoryとSAAA自身のlifecycleだけでdegraded動作します。</p></section><section className="settings-card"><h3>Retention</h3><div className="settings-form-grid"><Field label="History retention (days)"><input type="number" min="1" max="30" value={situation.retentionDays} onChange={(event) => onChange({ ...situation, retentionDays: Math.max(1, Math.min(30, Number(event.target.value) || 7)) })} /></Field><Field label="Maximum entries"><input value={situation.maxLedgerEntries} disabled /></Field></div><p className="settings-help">状態遷移、decision変更、5分heartbeatだけを保存します。samplingごとのraw signalは保存しません。</p></section></div>;
}

function ProvidersSection({ providers, onChange }: { providers: ModelProvidersSettings; onChange: (value: ModelProvidersSettings) => void }) {
  const [tests, setTests] = useState<Record<string, { state: "testing" | "success" | "error"; message: string }>>({});
  function update(index: number, next: Partial<ModelProviderSettings>) {
    onChange({ providers: providers.providers.map((provider, providerIndex) => providerIndex === index ? { ...provider, ...next } : provider) });
  }
  function remove(index: number) {
    onChange({ providers: providers.providers.filter((_, providerIndex) => providerIndex !== index) });
  }
  function add() {
    const number = providers.providers.length + 1;
    onChange({ providers: [...providers.providers, { id: `provider-${Date.now()}`, enabled: false, label: `Provider ${number}`, location: "local", endpoint: "", model: "", credentialStatus: "not-configured" }] });
  }
  async function test(provider: ModelProviderSettings) {
    setTests((current) => ({ ...current, [provider.id]: { state: "testing", message: "Connecting…" } }));
    try {
      const result = await testModelProvider(provider);
      setTests((current) => ({ ...current, [provider.id]: { state: result.ok ? "success" : "error", message: `${result.message} · ${result.latencyMs} ms` } }));
    } catch (cause) {
      setTests((current) => ({ ...current, [provider.id]: { state: "error", message: cause instanceof Error ? cause.message : String(cause) } }));
    }
  }
  return <div className="settings-stack"><p className="settings-help">複数のOpenAI-compatible endpointを登録できます。API keyはSQLiteへ保存せず、`SAAA_PROVIDER_&lt;ID&gt;_API_KEY`または`OPENAI_API_KEY`から読み込みます。</p>{providers.providers.map((provider, index) => <section className="settings-card provider-card" key={provider.id}><div className="card-title-row"><div><h3>{provider.label || `Provider ${index + 1}`}</h3><p className="muted">ID: {provider.id}</p></div><label className="toggle"><input type="checkbox" checked={provider.enabled} onChange={(event) => update(index, { enabled: event.target.checked })} /><span /></label></div><div className="settings-form-grid"><Field label="Display name"><input value={provider.label} onChange={(event) => update(index, { label: event.target.value })} /></Field><Field label="Location"><select value={provider.location} onChange={(event) => update(index, { location: event.target.value as ModelProviderSettings["location"] })}><option value="local">Local</option><option value="cloud">Cloud</option></select></Field><Field label="Endpoint"><input value={provider.endpoint} placeholder="http://localhost:11434/v1" onChange={(event) => update(index, { endpoint: event.target.value })} /></Field><Field label="Model"><input value={provider.model} placeholder="model name" onChange={(event) => update(index, { model: event.target.value })} /></Field></div>{tests[provider.id] && <p className={`provider-test-result ${tests[provider.id].state}`}>{tests[provider.id].message}</p>}<div className="provider-card-footer"><span>Credential: environment</span><div><button className="text-button" type="button" onClick={() => void test(provider)} disabled={!provider.endpoint || !provider.model || tests[provider.id]?.state === "testing"}>Test connection</button><button className="text-button danger" type="button" onClick={() => remove(index)} disabled={providers.providers.length === 1}>Remove provider</button></div></div></section>)}<button className="add-provider-button" onClick={add}>＋ Add provider</button></div>;
}

function RoutingSection({ routing, providers, localOnlyWhenSelected, onChange }: { routing: RoutingSettings; providers: ModelProviderSettings[]; localOnlyWhenSelected: boolean; onChange: (value: RoutingSettings) => void }) {
  const selectable = providers.filter((provider) => provider.enabled);
  const fallbackIds = routing.conversationRespond.fallbackProviderIds;
  const primary = providers.find((provider) => provider.id === routing.conversationRespond.primaryProviderId);
  const effectiveFallbackIds = fallbackIds.filter((id) => !(localOnlyWhenSelected && primary?.location === "local" && providers.find((provider) => provider.id === id)?.location === "cloud"));
  const blockedFallbacks = fallbackIds.filter((id) => !effectiveFallbackIds.includes(id));
  return <div className="settings-stack"><section className="settings-card"><h3>conversation.respond</h3><p className="settings-help">通常ChatとVoiceの確定文字起こしに使うModel Routeです。</p><div className="settings-form-grid"><Field label="Primary provider"><select value={routing.conversationRespond.primaryProviderId} onChange={(event) => onChange({ ...routing, conversationRespond: { ...routing.conversationRespond, primaryProviderId: event.target.value } })}>{providers.map((provider) => <option key={provider.id} value={provider.id}>{provider.label || provider.id}{provider.enabled ? "" : " (disabled)"}</option>)}</select></Field><Field label="Timeout (ms)"><input type="number" min="1000" max="120000" value={routing.conversationRespond.timeoutMs} onChange={(event) => onChange({ ...routing, conversationRespond: { ...routing.conversationRespond, timeoutMs: clampTimeout(event.target.value) } })} /></Field></div><div className="fallback-list"><strong>Fallback providers</strong>{selectable.filter((provider) => provider.id !== routing.conversationRespond.primaryProviderId).map((provider) => <label className="check-row" key={provider.id}><input type="checkbox" checked={fallbackIds.includes(provider.id)} onChange={(event) => onChange({ ...routing, conversationRespond: { ...routing.conversationRespond, fallbackProviderIds: event.target.checked ? [...fallbackIds, provider.id] : fallbackIds.filter((id) => id !== provider.id) } })} />{provider.label || provider.id}</label>)}{selectable.length < 2 && <p className="muted">Fallbackを使うには、もう一つ有効なProviderを登録してください。</p>}{blockedFallbacks.length > 0 && <p className="provider-test-result error">Local-only policy blocks Cloud fallback: {blockedFallbacks.join(", ")}</p>}</div><div className="effective-route"><span>Effective route</span><strong>{[routing.conversationRespond.primaryProviderId, ...effectiveFallbackIds].join(" → ")}</strong></div></section><section className="settings-card"><h3>coding.assist</h3><p className="settings-help">Codex SDK専用Routeです。ユーザーがCoding modeを明示選択した場合だけ実行します。</p><div className="settings-form-grid"><Field label="Provider"><input value="codex-sdk" disabled /></Field><Field label="Timeout (ms)"><input type="number" min="1000" max="300000" value={routing.codingAssist.timeoutMs} onChange={(event) => onChange({ ...routing, codingAssist: { ...routing.codingAssist, timeoutMs: clampTimeout(event.target.value) } })} /></Field></div><div className="locked-policy">Read-only · Approval never · Network disabled · Web Search disabled</div></section></div>;
}

function VoiceSection({ voice, onChange }: { voice: VoiceSettings; onChange: (value: VoiceSettings) => void }) {
  const [devices, setDevices] = useState<MediaDeviceInfo[]>([]);
  const [deviceError, setDeviceError] = useState<string | null>(null);
  useEffect(() => {
    let active = true;
    void navigator.mediaDevices?.enumerateDevices()
      .then((available) => { if (active) setDevices(available.filter((device) => device.kind === "audioinput")); })
      .catch((cause) => { if (active) setDeviceError(cause instanceof Error ? cause.message : String(cause)); });
    return () => { active = false; };
  }, []);
  async function chooseModel() {
    try {
      const selected = await open({ multiple: false, directory: false, title: "Choose a local whisper.cpp model" });
      if (typeof selected === "string") onChange({ ...voice, sttModel: selected });
    } catch (cause) {
      setDeviceError(cause instanceof Error ? cause.message : String(cause));
    }
  }
  const currentDeviceMissing = voice.inputDeviceId !== "default" && !devices.some((device) => device.deviceId === voice.inputDeviceId);
  return <div className="settings-stack"><section className="settings-card"><h3>Capture</h3><div className="settings-form-grid"><Field label="Capture mode"><select value={voice.captureMode} disabled><option value="push-to-talk">Push-to-talk</option></select></Field><Field label="Input device"><select value={voice.inputDeviceId} onChange={(event) => onChange({ ...voice, inputDeviceId: event.target.value })}><option value="default">System default</option>{currentDeviceMissing && <option value={voice.inputDeviceId}>{voice.inputDeviceId} (unavailable)</option>}{devices.map((device, index) => <option key={device.deviceId} value={device.deviceId}>{device.label || `Microphone ${index + 1}`}</option>)}</select></Field><Field label="STT provider"><input value={voice.sttProviderId} disabled /></Field><Field label="STT model"><div className="inline-picker"><input value={voice.sttModel} onChange={(event) => onChange({ ...voice, sttModel: event.target.value })} placeholder="Absolute path to a local whisper model" /><button className="secondary-button" type="button" onClick={() => void chooseModel()}>Choose…</button></div></Field></div>{deviceError && <p className="provider-test-result error">Microphone devices unavailable: {deviceError}. Grant microphone access and reopen Settings.</p>}</section><section className="settings-card"><h3>Speech output</h3><div className="settings-form-grid"><Field label="Output device"><input value="System default" disabled /></Field><Field label="TTS provider"><input value={voice.ttsProviderId} disabled /></Field><Field label="Voice"><input value={voice.ttsVoice} onChange={(event) => onChange({ ...voice, ttsVoice: event.target.value })} /></Field><Field label="Auto speak"><select value={voice.autoSpeak ? "on" : "off"} onChange={(event) => onChange({ ...voice, autoSpeak: event.target.value === "on" })}><option value="on">On</option><option value="off">Off</option></select></Field></div><div className="locked-policy">Cloud fallback: disabled. Audio stays on this device; missing whisper/TTS affects Voice only.</div></section></div>;
}

function CodexSection({ codex, onChange }: { codex: CodexAgentSettings; onChange: (value: CodexAgentSettings) => void }) {
  const [models, setModels] = useState<CodexModelOption[]>([]);
  const [modelState, setModelState] = useState<"loading" | "ready" | "error">("loading");
  const [modelError, setModelError] = useState<string | null>(null);
  const [runtimeStatus, setRuntimeStatus] = useState<CodexRuntimeStatus | null>(null);

  async function loadModels() {
    setModelState("loading");
    setModelError(null);
    try {
      setModels(await listCodexModels());
      setModelState("ready");
    } catch (cause) {
      setModelState("error");
      setModelError(cause instanceof Error ? cause.message : String(cause));
    }
  }

  async function loadRuntimeStatus() {
    setRuntimeStatus(await getCodexStatus());
  }

  async function refreshCodex() {
    await Promise.allSettled([loadModels(), loadRuntimeStatus()]);
  }

  useEffect(() => {
    let active = true;
    void getCodexStatus()
      .then((status) => { if (active) setRuntimeStatus(status); })
      .catch((cause) => {
        if (active) setRuntimeStatus({ installed: false, authenticated: false, runtime: "unavailable", accountType: null, message: cause instanceof Error ? cause.message : String(cause) });
      });
    void listCodexModels()
      .then((availableModels) => {
        if (!active) return;
        setModels(availableModels);
        setModelState("ready");
      })
      .catch((cause) => {
        if (!active) return;
        setModelState("error");
        setModelError(cause instanceof Error ? cause.message : String(cause));
      });
    return () => { active = false; };
  }, []);

  const defaultModel = models.find((model) => model.isDefault);
  const selectedModelMissing = Boolean(codex.model) && !models.some((model) => model.model === codex.model);

  return <div className="settings-stack"><section className="settings-card"><div className="card-title-row"><div><h3>Codex SDK agent</h3><p className="settings-help">`coding.assist`だけに接続するAgent Providerです。</p></div><label className="toggle"><input type="checkbox" checked={codex.enabled} onChange={(event) => onChange({ ...codex, enabled: event.target.checked })} /><span /></label></div><div className="settings-form-grid"><Field label="Model"><select value={codex.model} onChange={(event) => onChange({ ...codex, model: event.target.value })} disabled={modelState === "loading" && models.length === 0}><option value="">Codex default{defaultModel ? ` (${defaultModel.displayName})` : ""}</option>{selectedModelMissing && <option value={codex.model}>{codex.model} (saved)</option>}{models.map((model) => <option key={model.id} value={model.model}>{model.displayName}{model.isDefault ? " — default" : ""}</option>)}</select></Field><Field label="Runtime"><input value={runtimeStatus?.runtime ?? codex.runtimeMode.replace(/-/g, " ")} disabled /></Field><Field label="Installed"><input value={runtimeStatus ? (runtimeStatus.installed ? "yes" : "no") : "checking"} disabled /></Field><Field label="Authentication"><input value={runtimeStatus ? (runtimeStatus.authenticated ? runtimeStatus.accountType ?? "authenticated" : "not authenticated") : "checking"} disabled /></Field><Field label="Workspace"><input value="Choose per conversation" disabled /></Field></div><div className={`codex-model-status ${runtimeStatus?.authenticated ? "ready" : modelState}`} aria-live="polite"><span>{runtimeStatus?.message ?? "Codex runtimeを確認しています…"} · {modelState === "loading" && "モデル一覧を取得中"}{modelState === "ready" && `${models.length} models available`}{modelState === "error" && (modelError || "モデル一覧を取得できませんでした。")}</span><button className="text-button" type="button" onClick={() => void refreshCodex()} disabled={modelState === "loading"}>Refresh</button></div>{models.length > 0 && <div className="codex-model-list">{models.map((model) => <button className={codex.model === model.model || (!codex.model && model.isDefault) ? "codex-model-option selected" : "codex-model-option"} type="button" key={model.id} onClick={() => onChange({ ...codex, model: model.model })}><span><strong>{model.displayName}</strong>{model.isDefault && <small>Default</small>}</span><code>{model.model}</code><p>{model.description}</p><small>Reasoning: {model.supportedReasoningEfforts.map((effort) => effort.reasoningEffort).join(" · ") || "not reported"}</small></button>)}</div>}</section><section className="settings-card"><h3>Safety policy</h3><div className="policy-grid"><Policy label="Sandbox" value={codex.sandboxMode} /><Policy label="Approvals" value={codex.approvalPolicy} /><Policy label="Network" value="disabled" /><Policy label="Web Search" value="disabled" /></div><p className="settings-help">これらはMVPの固定安全条件です。設定画面から緩和できません。</p></section></div>;
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
  return <div className="settings-stack"><section className="settings-card"><h3>Credential handling</h3><p className="settings-help">Model API keyは環境変数からだけ読み込み、SQLiteやDiagnosticsへ保存しません。</p><Field label="Credential store"><input value={security.credentialStorage} disabled /></Field></section><section className="settings-card"><h3>Privacy defaults</h3><label className="check-row"><input type="checkbox" checked={security.localOnlyWhenSelected} onChange={(event) => onChange({ ...security, localOnlyWhenSelected: event.target.checked })} />Local routeを選択している場合はCloudへ自動fallbackしない</label><label className="check-row"><input type="checkbox" checked={security.diagnosticsRedaction} onChange={(event) => onChange({ ...security, diagnosticsRedaction: event.target.checked })} disabled />DiagnosticsとProvider activityからsecretをredactする（固定）</label></section><section className="settings-card"><h3>Recovery & diagnostics</h3><p className="settings-help">Diagnosticsには会話本文、workspace path、thread ID、credentialを含めません。Backupは整合性のあるSQLite snapshotです。</p><div className="artifact-actions"><button className="secondary-button" type="button" onClick={() => void createArtifact("diagnostics")} disabled={artifactState === "working"}>Export diagnostics</button><button className="secondary-button" type="button" onClick={() => void createArtifact("backup")} disabled={artifactState === "working"}>Backup database</button></div>{artifactMessage && <p className={`provider-test-result ${artifactState === "success" ? "success" : "error"}`} aria-live="polite">{artifactMessage}</p>}</section></div>;
}

function Field({ label, children }: { label: string; children: ReactNode }) { return <label className="settings-field"><span>{label}</span>{children}</label>; }
function Metric({ label, value }: { label: string; value: string }) { return <div><span>{label}</span><strong>{value}</strong></div>; }
function Policy({ label, value }: { label: string; value: string }) { return <div><span>{label}</span><strong>{value}</strong></div>; }
function clampTimeout(value: string): number { return Math.max(1000, Math.min(300000, Number(value) || 30000)); }

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
    document("providers.agent", "codex-sdk", draft.codex),
    document("routing.tasks", "default", draft.routing),
    document("voice.runtime", "default", draft.voice),
    document("security.runtime", "default", draft.security),
    document("situation.runtime", "default", draft.situation),
  ];
}

function document(namespace: SettingsNamespace, key: "default" | "codex-sdk", valueJson: Record<string, unknown>): Omit<SettingsDocument, "updatedAt"> {
  return { namespace, key, schemaVersion: 6, valueJson };
}
