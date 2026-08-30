import { useEffect, useMemo, useState } from "react";
import type {
  LarmRuntimeStatus,
  SecuritySettings,
  SettingsDocument,
  SituationSettings,
  SituationSnapshot,
  VoiceProfileSnapshot,
} from "../../lib/contracts";
import {
  backupDatabase,
  exportDiagnostics,
  getSituationSnapshot,
  saveSettingsDocuments,
} from "../../lib/runtime";
import { deleteProviderApiKey } from "../../lib/providerRuntime";
import { IndividualProvidersSection } from "./IndividualProvidersSection";
import { ServiceConnectionsSection } from "./ServiceConnectionsSection";
import { Field, Metric } from "./SettingsFields";
import { defaultSettingsDraft, DEFAULT_AGENT_NAME } from "./settingsDefaults";
import { credentialCleanupProviderIds, documentsFromDraft, draftFromDocuments, type SettingsDraft } from "./settingsDraft";
import { VoiceSettingsSection } from "./VoiceSettingsSection";

type SettingsTab = "general" | "connection" | "providers" | "voice" | "situation" | "security";

const tabs: Array<{ id: SettingsTab; label: string; detail: string }> = [
  { id: "general", label: "General", detail: "Identity and runtime overview" },
  { id: "connection", label: "Service connection", detail: "Harness and service sources" },
  { id: "providers", label: "Individual services", detail: "Cloud LLM, ASR and TTS" },
  { id: "voice", label: "Voice & devices", detail: "Always-on listening and audio" },
  { id: "situation", label: "Situation", detail: "Shadow observation controls" },
  { id: "security", label: "Privacy & Security", detail: "Local-first controls" },
];

export function SettingsPage({
  documents,
  larmRuntime: _larmRuntime,
  voiceProfile,
  voiceEnrollmentBlocked,
  onSaved,
  onVoiceProfileChanged,
}: {
  documents: SettingsDocument[];
  larmRuntime: LarmRuntimeStatus;
  voiceProfile: VoiceProfileSnapshot;
  voiceEnrollmentBlocked: boolean;
  onSaved: (documents: SettingsDocument[]) => void;
  onVoiceProfileChanged: (profile: VoiceProfileSnapshot) => void;
}) {
  const source = useMemo(() => draftFromDocuments(documents, defaultSettingsDraft), [documents]);
  const [draft, setDraft] = useState<SettingsDraft>(source);
  const [activeTab, setActiveTab] = useState<SettingsTab>("connection");
  const [saveState, setSaveState] = useState<"idle" | "saving" | "saved" | "error">("idle");
  const [saveMessage, setSaveMessage] = useState<string | null>(null);
  useEffect(() => {
    setDraft(source);
    setSaveState("idle");
    setSaveMessage(null);
  }, [source]);
  const dirty = JSON.stringify(draft) !== JSON.stringify(source);
  const persistedProviderIds = useMemo(
    () => new Set(source.providers.providers.map((provider) => provider.id)),
    [source],
  );
  const activeTabMeta = tabs.find((tab) => tab.id === activeTab) ?? tabs[0];

  async function save() {
    setSaveState("saving");
    setSaveMessage(null);
    try {
      const credentialCleanup = credentialCleanupProviderIds(source, draft);
      const saved = await saveSettingsDocuments(documentsFromDraft(draft));
      onSaved(saved);
      const cleanupResults = await Promise.allSettled(
        credentialCleanup.map((providerId) => deleteProviderApiKey(providerId)),
      );
      const cleanupFailures = cleanupResults.filter((result) => result.status === "rejected").length;
      setSaveState("saved");
      setSaveMessage(cleanupFailures > 0
        ? `設定を保存しましたが、未使用API key ${cleanupFailures}件をKeychainから削除できませんでした。`
        : `SQLiteへ保存しました · ${new Date().toLocaleTimeString()}`);
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
          <h1>Service &amp; voice settings</h1>
          <p>Harnessを中心に、必要なサービスだけ個別Providerへ切り替えられます。</p>
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
              <strong>{tab.label}</strong>
              <span>{tab.detail}</span>
            </button>
          ))}
        </nav>
        <div className="settings-content">
          <header className="settings-content-header"><h2>{activeTabMeta.label}</h2><p>{activeTabMeta.detail}</p></header>
          {activeTab === "general" && <GeneralSection draft={draft} onChange={setDraft} />}
          {activeTab === "connection" && (
            <ServiceConnectionsSection
              providers={draft.providers}
              routing={draft.routing}
              onProvidersChange={(providers) => setDraft((current) => ({ ...current, providers }))}
              onRoutingChange={(routing) => setDraft((current) => ({ ...current, routing }))}
            />
          )}
          {activeTab === "providers" && (
            <IndividualProvidersSection
              settings={draft.providers}
              persistedProviderIds={persistedProviderIds}
              onChange={(providers) => setDraft((current) => ({ ...current, providers }))}
            />
          )}
          {activeTab === "voice" && (
            <VoiceSettingsSection
              voice={draft.voice}
              profile={voiceProfile}
              enrollmentBlocked={voiceEnrollmentBlocked}
              onProfileChanged={onVoiceProfileChanged}
              onChange={(voice) => setDraft((current) => ({ ...current, voice }))}
            />
          )}
          {activeTab === "situation" && (
            <SituationSection situation={draft.situation} onChange={(situation) => setDraft((current) => ({ ...current, situation }))} />
          )}
          {activeTab === "security" && (
            <SecuritySection security={draft.security} onChange={(security) => setDraft((current) => ({ ...current, security }))} />
          )}
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

function GeneralSection({ draft, onChange }: { draft: SettingsDraft; onChange: (draft: SettingsDraft) => void }) {
  const enabledProviders = draft.providers.providers.filter((provider) => provider.enabled && provider.kind !== "dynamic-lan").length;
  return (
    <div className="settings-stack">
      <section className="settings-card">
        <h3>Conversation identity</h3>
        <div className="settings-form-grid">
          <Field label="Agent name"><input value={draft.codex.agentName} maxLength={80} placeholder={DEFAULT_AGENT_NAME} onChange={(event) => onChange({ ...draft, codex: { ...draft.codex, agentName: event.target.value } })} /></Field>
          <Field label="User name"><input value={draft.codex.userName} maxLength={80} placeholder="未設定（名前で呼ばない）" onChange={(event) => onChange({ ...draft, codex: { ...draft.codex, userName: event.target.value } })} /></Field>
        </div>
      </section>
      <section className="settings-card">
        <h3>Runtime state</h3>
        <div className="settings-summary-grid">
          <Metric label="Harness" value={draft.providers.harness.address || "not configured"} />
          <Metric label="Individual providers" value={`${enabledProviders} enabled`} />
          <Metric label="Listening" value={draft.voice.listeningEnabled ? "always on" : "paused"} />
          <Metric label="Situation" value={draft.situation.enabled ? "shadow monitoring" : "paused"} />
        </div>
      </section>
    </div>
  );
}

function SituationSection({ situation, onChange }: { situation: SituationSettings; onChange: (value: SituationSettings) => void }) {
  const [snapshot, setSnapshot] = useState<SituationSnapshot | null>(null);
  useEffect(() => {
    let active = true;
    void getSituationSnapshot().then((value) => { if (active) setSnapshot(value); }).catch(() => undefined);
    return () => { active = false; };
  }, []);
  return (
    <div className="settings-stack">
      <section className="settings-card">
        <div className="card-title-row">
          <div><h3>Situation Shadow Mode</h3><p className="settings-help">候補だけを記録し、Model・TTS・通知・外部操作は自動実行しません。</p></div>
          <label className="toggle"><input type="checkbox" checked={situation.enabled} onChange={(event) => onChange({ ...situation, enabled: event.target.checked })} /><span /></label>
        </div>
        <div className="settings-summary-grid">
          <Metric label="Sampling" value={`${situation.sampleIntervalMs} ms`} />
          <Metric label="Calendar" value={snapshot?.signals.calendar.health ?? "disabled"} />
          <Metric label="Retention" value={`${situation.retentionDays} days`} />
          <Metric label="Raw audio" value="never stored" />
        </div>
      </section>
    </div>
  );
}

function SecuritySection({ security, onChange }: { security: SecuritySettings; onChange: (value: SecuritySettings) => void }) {
  const [message, setMessage] = useState<string | null>(null);
  async function run(action: typeof backupDatabase | typeof exportDiagnostics) {
    try {
      const result = await action();
      setMessage(result.path);
    } catch (cause) {
      setMessage(cause instanceof Error ? cause.message : String(cause));
    }
  }
  return (
    <div className="settings-stack">
      <section className="settings-card">
        <h3>Credentials</h3>
        <p>API keyはmacOS Keychainへ保存します。値を再表示せず、SQLite・backup・diagnosticsには含めません。</p>
        <div className="locked-policy">Storage: macOS Keychain · Service: com.saaa.provider-api-key</div>
      </section>
      <section className="settings-card">
        <h3>Runtime policy</h3>
        <label className="check-row"><input type="checkbox" checked={security.localOnlyWhenSelected} onChange={(event) => onChange({ ...security, localOnlyWhenSelected: event.target.checked })} />Local primaryからCloud fallbackを暗黙選択しない</label>
        <label className="check-row"><input type="checkbox" checked={security.diagnosticsRedaction} disabled />Diagnostics redaction (always on)</label>
      </section>
      <section className="settings-card">
        <h3>Data operations</h3>
        <div className="provider-card-footer">
          <span>{message ?? "API keyはどちらの出力にも含まれません。"}</span>
          <div>
            <button className="text-button" type="button" onClick={() => void run(exportDiagnostics)}>Export diagnostics</button>
            <button className="text-button" type="button" onClick={() => void run(backupDatabase)}>Backup database</button>
          </div>
        </div>
      </section>
    </div>
  );
}
