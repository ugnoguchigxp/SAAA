import { useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import type {
  LarmRuntimeStatus,
  RegionalPreferencesSettings,
  SecuritySettings,
  SettingsDocument,
  SituationSettings,
  SituationSnapshot,
  VoiceProfileSnapshot,
} from "../../lib/contracts";
import { setDisplayLanguagePreference } from "../../i18n";
import { localizeStatus, localizeUiMessage } from "../../i18n/presentation";
import { availableTimeZones, CURRENCY_CODES, systemTimeZone } from "../../lib/regionalPreferences";
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
type SaveNotice =
  | { kind: "saved"; cleanupFailures: number; savedAt: number }
  | { kind: "error"; message: string };

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
  const { t, i18n } = useTranslation();
  const tabs: Array<{ id: SettingsTab; label: string; detail: string }> = [
    { id: "general", label: t("settings.tabs.general.label"), detail: t("settings.tabs.general.detail") },
    { id: "connection", label: t("settings.tabs.connection.label"), detail: t("settings.tabs.connection.detail") },
    { id: "providers", label: t("settings.tabs.providers.label"), detail: t("settings.tabs.providers.detail") },
    { id: "voice", label: t("settings.tabs.voice.label"), detail: t("settings.tabs.voice.detail") },
    { id: "situation", label: t("settings.tabs.situation.label"), detail: t("settings.tabs.situation.detail") },
    { id: "security", label: t("settings.tabs.security.label"), detail: t("settings.tabs.security.detail") },
  ];
  const source = useMemo(() => draftFromDocuments(documents, defaultSettingsDraft), [documents]);
  const [draft, setDraft] = useState<SettingsDraft>(source);
  const [activeTab, setActiveTab] = useState<SettingsTab>("general");
  const [saveState, setSaveState] = useState<"idle" | "saving" | "saved" | "error">("idle");
  const [saveMessage, setSaveMessage] = useState<SaveNotice | null>(null);
  useEffect(() => {
    setDraft(source);
    void setDisplayLanguagePreference(source.regional.language);
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
      setSaveMessage({ kind: "saved", cleanupFailures, savedAt: Date.now() });
    } catch (cause) {
      setSaveState("error");
      setSaveMessage({ kind: "error", message: cause instanceof Error ? cause.message : String(cause) });
    }
  }

  function discard() {
    setDraft(source);
    void setDisplayLanguagePreference(source.regional.language);
  }

  return (
    <section className="settings-page">
      <header className="settings-page-header">
        <div>
          <p className="eyebrow">{t("settings.eyebrow")}</p>
          <h1>{t("settings.title")}</h1>
          <p>{t("settings.description")}</p>
        </div>
        <div className="settings-save-status" aria-live="polite">
          {dirty && saveState === "idle" && <span className="unsaved">{t("settings.unsaved")}</span>}
          {saveMessage && <span className={saveMessage.kind === "error" ? "save-error" : "save-success"}>{saveMessage.kind === "error" ? localizeUiMessage(t, saveMessage.message, "settings") : saveMessage.cleanupFailures > 0 ? t("settings.savedWithCleanupFailure", { count: saveMessage.cleanupFailures }) : t("settings.savedAt", { time: new Date(saveMessage.savedAt).toLocaleTimeString(i18n.resolvedLanguage, draft.regional.timeZone === "system" ? undefined : { timeZone: draft.regional.timeZone }) })}</span>}
        </div>
      </header>
      <div className="settings-screen-layout">
        <nav className="settings-menu" aria-label={t("settings.sectionsLabel")}>
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
        <p>{dirty ? t("settings.pendingRuntime") : t("settings.showingSaved")}</p>
        <div>
          <button className="discard-button" onClick={discard} disabled={!dirty || saveState === "saving"}>{t("settings.discard")}</button>
          <button className="save-button" onClick={() => void save()} disabled={!dirty || saveState === "saving"}>{saveState === "saving" ? t("settings.saving") : t("settings.saveSettings")}</button>
        </div>
      </footer>
    </section>
  );
}

function GeneralSection({ draft, onChange }: { draft: SettingsDraft; onChange: (draft: SettingsDraft) => void }) {
  const { t, i18n } = useTranslation();
  const timeZones = useMemo(availableTimeZones, []);
  const localTimeZone = systemTimeZone();
  const currencyNames = useMemo(
    () => new Intl.DisplayNames([i18n.resolvedLanguage ?? "en"], { type: "currency" }),
    [i18n.resolvedLanguage],
  );
  const enabledProviders = draft.providers.providers.filter((provider) => provider.enabled && provider.kind !== "dynamic-lan").length;
  function changeRegional<K extends keyof RegionalPreferencesSettings>(key: K, value: RegionalPreferencesSettings[K]) {
    onChange({ ...draft, regional: { ...draft.regional, [key]: value } });
  }
  return (
    <div className="settings-stack">
      <section className="settings-card">
        <h3>{t("settings.general.regionalPreferences")}</h3>
        <p>{t("settings.general.regionalPreferencesDescription")}</p>
        <div className="settings-form-grid">
          <Field label={t("settings.general.displayLanguage")}>
            <select
              value={draft.regional.language}
              onChange={(event) => {
                const language = event.currentTarget.value as RegionalPreferencesSettings["language"];
                changeRegional("language", language);
                void setDisplayLanguagePreference(language);
              }}
            >
              <option value="system">{t("settings.general.systemLanguage")}</option>
              <option value="ja">{t("common.japanese")}</option>
              <option value="en">{t("common.english")}</option>
            </select>
          </Field>
          <Field label={t("settings.general.timeZone")}>
            <select value={draft.regional.timeZone} onChange={(event) => changeRegional("timeZone", event.currentTarget.value)}>
              <option value="system">{t("settings.general.systemTimeZone", { timeZone: localTimeZone })}</option>
              {timeZones.map((timeZone) => <option key={timeZone} value={timeZone}>{timeZone}</option>)}
            </select>
          </Field>
          <Field label={t("settings.general.lengthUnit")}>
            <select value={draft.regional.lengthUnit} onChange={(event) => changeRegional("lengthUnit", event.currentTarget.value as RegionalPreferencesSettings["lengthUnit"])}>
              <option value="metric">{t("settings.general.metric")}</option>
              <option value="imperial">{t("settings.general.imperial")}</option>
            </select>
          </Field>
          <Field label={t("settings.general.weightUnit")}>
            <select value={draft.regional.weightUnit} onChange={(event) => changeRegional("weightUnit", event.currentTarget.value as RegionalPreferencesSettings["weightUnit"])}>
              <option value="kilogram">{t("settings.general.kilogram")}</option>
              <option value="pound">{t("settings.general.pound")}</option>
            </select>
          </Field>
          <Field label={t("settings.general.currency")}>
            <select value={draft.regional.currency} onChange={(event) => changeRegional("currency", event.currentTarget.value as RegionalPreferencesSettings["currency"])}>
              {CURRENCY_CODES.map((currency) => <option key={currency} value={currency}>{currency} — {currencyNames.of(currency) ?? currency}</option>)}
            </select>
          </Field>
        </div>
      </section>
      <section className="settings-card">
        <h3>{t("settings.general.identity")}</h3>
        <div className="settings-form-grid">
          <Field label={t("settings.general.agentName")}><input value={draft.codex.agentName} maxLength={80} placeholder={DEFAULT_AGENT_NAME} onChange={(event) => onChange({ ...draft, codex: { ...draft.codex, agentName: event.target.value } })} /></Field>
          <Field label={t("settings.general.userName")}><input value={draft.codex.userName} maxLength={80} placeholder={t("settings.general.userNamePlaceholder")} onChange={(event) => onChange({ ...draft, codex: { ...draft.codex, userName: event.target.value } })} /></Field>
        </div>
      </section>
      <section className="settings-card">
        <h3>{t("settings.general.runtimeState")}</h3>
        <div className="settings-summary-grid">
          <Metric label={t("settings.general.harness")} value={draft.providers.harness.address || t("common.notConfigured")} />
          <Metric label={t("settings.general.individualProviders")} value={t("settings.general.enabledCount", { count: enabledProviders })} />
          <Metric label={t("settings.general.listening")} value={draft.voice.listeningEnabled ? t("settings.general.alwaysOn") : t("common.paused")} />
          <Metric label={t("settings.general.situation")} value={draft.situation.enabled ? t("settings.general.shadowMonitoring") : t("common.paused")} />
        </div>
      </section>
    </div>
  );
}

function SituationSection({ situation, onChange }: { situation: SituationSettings; onChange: (value: SituationSettings) => void }) {
  const { t } = useTranslation();
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
          <div><h3>{t("settings.situation.title")}</h3><p className="settings-help">{t("settings.situation.description")}</p></div>
          <label className="toggle"><input type="checkbox" checked={situation.enabled} onChange={(event) => onChange({ ...situation, enabled: event.target.checked })} /><span /></label>
        </div>
        <div className="settings-summary-grid">
          <Metric label={t("settings.situation.sampling")} value={`${situation.sampleIntervalMs} ms`} />
          <Metric label={t("settings.situation.calendar")} value={snapshot ? localizeStatus(t, snapshot.signals.calendar.health) : t("common.disabled")} />
          <Metric label={t("settings.situation.retention")} value={t("settings.situation.days", { count: situation.retentionDays })} />
          <Metric label={t("settings.situation.rawAudio")} value={t("settings.situation.neverStored")} />
        </div>
      </section>
    </div>
  );
}

function SecuritySection({ security, onChange }: { security: SecuritySettings; onChange: (value: SecuritySettings) => void }) {
  const { t } = useTranslation();
  const [message, setMessage] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  async function run(action: typeof backupDatabase | typeof exportDiagnostics) {
    try {
      const result = await action();
      setMessage(result.path);
      setError(null);
    } catch (cause) {
      setMessage(null);
      setError(cause instanceof Error ? cause.message : String(cause));
    }
  }
  return (
    <div className="settings-stack">
      <section className="settings-card">
        <h3>{t("settings.security.credentials")}</h3>
        <p>{t("settings.security.credentialsDescription")}</p>
        <div className="locked-policy">{t("settings.security.storagePolicy")}</div>
      </section>
      <section className="settings-card">
        <h3>{t("settings.security.runtimePolicy")}</h3>
        <label className="check-row"><input type="checkbox" checked={security.localOnlyWhenSelected} onChange={(event) => onChange({ ...security, localOnlyWhenSelected: event.target.checked })} />{t("settings.security.noCloudFallback")}</label>
        <label className="check-row"><input type="checkbox" checked={security.diagnosticsRedaction} disabled />{t("settings.security.diagnosticsRedaction")}</label>
      </section>
      <section className="settings-card">
        <h3>{t("settings.security.dataOperations")}</h3>
        <div className="provider-card-footer">
          <span>{error ? localizeUiMessage(t, error, "settings") : message ?? t("settings.security.noApiKeysInExports")}</span>
          <div>
            <button className="text-button" type="button" onClick={() => void run(exportDiagnostics)}>{t("settings.security.exportDiagnostics")}</button>
            <button className="text-button" type="button" onClick={() => void run(backupDatabase)}>{t("settings.security.backupDatabase")}</button>
          </div>
        </div>
      </section>
    </div>
  );
}
