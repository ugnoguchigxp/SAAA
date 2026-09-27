import { PersonalStateSection } from "./PersonalStateSection";
import { CodingSettingsSection } from "../coding/CodingSettingsSection";
import { SecuritySection } from "./SecuritySection";
import { useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import type { SettingsDocument, VoiceProfileSnapshot } from "../../lib/contracts";
import { setDisplayLanguagePreference } from "../../i18n";
import { localizeUiMessage } from "../../i18n/presentation";
import { saveSettingsDocuments, setTargetSpeakerFilterEnabled } from "../../lib/runtime";
import { deleteProviderApiKey } from "../../lib/providerRuntime";
import { IndividualProvidersSection } from "./IndividualProvidersSection";
import { ServiceConnectionsSection } from "./ServiceConnectionsSection";
import {
  credentialCleanupProviderIds,
  documentsFromDraft,
  draftFromDocuments,
  reconcileSavedDraft,
  settingsPageHasChanges,
  type SettingsDraft,
} from "./settingsDraft";
import { defaultSettingsDraft } from "./settingsDefaults";
import { VoiceSettingsSection } from "./VoiceSettingsSection";
import { SettingsGeneralSection } from "./SettingsGeneralSection";
import { RoleRoutingSection } from "./RoleRoutingSection";
import { ScheduleSection } from "./ScheduleSection";
import type { VoiceCaptureState as AmbientVoiceAvailability } from "../../lib/voiceSession";

type SettingsTab =
  | "general"
  | "connection"
  | "providers"
  | "coding"
  | "routing"
  | "voice"
  | "security"
  | "schedule";
type SaveNotice =
  | { kind: "saved"; cleanupFailures: number; savedAt: number }
  | { kind: "error"; message: string };

export function SettingsPage({
  documents,
  voiceProfile,
  voiceEnrollmentBlocked,
  voiceListeningEnabled,
  voiceListeningBusy,
  voiceAvailability,
  voiceError,
  onSaved,
  onVoiceProfileChanged,
  onToggleVoiceListening,
}: {
  documents: SettingsDocument[];
  voiceProfile: VoiceProfileSnapshot;
  voiceEnrollmentBlocked: boolean;
  voiceListeningEnabled: boolean;
  voiceListeningBusy: boolean;
  voiceAvailability: AmbientVoiceAvailability;
  voiceError: string | null;
  onSaved: (documents: SettingsDocument[]) => void;
  onVoiceProfileChanged: (profile: VoiceProfileSnapshot) => void;
  onToggleVoiceListening: (enabled: boolean) => void;
}) {
  const { t, i18n } = useTranslation();
  const tabs: Array<{ id: SettingsTab; label: string; detail: string }> = [
    {
      id: "general",
      label: t("settings.tabs.general.label"),
      detail: t("settings.tabs.general.detail"),
    },
    {
      id: "connection",
      label: t("settings.tabs.connection.label"),
      detail: t("settings.tabs.connection.detail"),
    },
    {
      id: "providers",
      label: t("settings.tabs.providers.label"),
      detail: t("settings.tabs.providers.detail"),
    },
    { id: "coding", label: "実装方法", detail: "Pi / Codex SDK" },
    { id: "routing", label: "Role routing", detail: "モデルの役割と学習" },
    {
      id: "schedule",
      label: t("settings.tabs.schedule.label"),
      detail: t("settings.tabs.schedule.detail"),
    },
    { id: "voice", label: t("settings.tabs.voice.label"), detail: t("settings.tabs.voice.detail") },
    {
      id: "security",
      label: t("settings.tabs.security.label"),
      detail: t("settings.tabs.security.detail"),
    },
  ];
  const source = useMemo(() => draftFromDocuments(documents, defaultSettingsDraft), [documents]);
  const [draft, setDraft] = useState<SettingsDraft>(source);
  const [profileFilterDraft, setProfileFilterDraft] = useState(voiceProfile.filterEnabled);
  const draftRef = useRef(draft);
  draftRef.current = draft;
  const previousSourceRef = useRef(source);
  const previousProfileFilterRef = useRef(voiceProfile.filterEnabled);
  const saveGeneration = useRef(0);
  const draftEditGeneration = useRef(0);
  const [activeTab, setActiveTab] = useState<SettingsTab>("general");
  const [connectionSettingsValid, setConnectionSettingsValid] = useState(true);
  const [saveState, setSaveState] = useState<"idle" | "saving" | "saved" | "error">("idle");
  const [saveMessage, setSaveMessage] = useState<SaveNotice | null>(null);
  useEffect(() => {
    setDraft((current) =>
      JSON.stringify(current) === JSON.stringify(previousSourceRef.current)
        ? source
        : {
            ...current,
            voice: { ...current.voice, listeningEnabled: source.voice.listeningEnabled },
          },
    );
    previousSourceRef.current = source;
    void setDisplayLanguagePreference(source.regional.language);
  }, [source]);
  useEffect(() => {
    setProfileFilterDraft((current) =>
      current === previousProfileFilterRef.current ? voiceProfile.filterEnabled : current,
    );
    previousProfileFilterRef.current = voiceProfile.filterEnabled;
  }, [voiceProfile.filterEnabled]);
  const dirty = settingsPageHasChanges(
    draft,
    source,
    profileFilterDraft,
    voiceProfile.filterEnabled,
  );
  const persistedProviderIds = useMemo(
    () => new Set(source.providers.providers.map((provider) => provider.id)),
    [source],
  );
  const activeTabMeta = tabs.find((tab) => tab.id === activeTab) ?? tabs[0];

  function changeDraft(update: SettingsDraft | ((current: SettingsDraft) => SettingsDraft)) {
    draftEditGeneration.current += 1;
    setDraft(update);
    setSaveMessage(null);
    setSaveState((current) => (current === "saving" ? current : "idle"));
  }

  function changeProfileFilter(enabled: boolean) {
    draftEditGeneration.current += 1;
    setProfileFilterDraft(enabled);
    setSaveMessage(null);
    setSaveState((current) => (current === "saving" ? current : "idle"));
  }

  async function save() {
    const generation = ++saveGeneration.current;
    const submitted = draftRef.current;
    const submittedFingerprint = JSON.stringify(submitted);
    const submittedSettingsDirty = submittedFingerprint !== JSON.stringify(source);
    const submittedProfileFilter = profileFilterDraft;
    const submittedProfileFilterDirty = submittedProfileFilter !== voiceProfile.filterEnabled;
    const submittedEditGeneration = draftEditGeneration.current;
    setSaveState("saving");
    setSaveMessage(null);
    try {
      let cleanupFailures = 0;
      if (submittedSettingsDirty) {
        const credentialCleanup = credentialCleanupProviderIds(source, submitted);
        const saved = await saveSettingsDocuments(documentsFromDraft(submitted));
        if (generation !== saveGeneration.current) return;
        setDraft((current) => reconcileSavedDraft(current, submittedFingerprint, saved));
        onSaved(saved);
        const cleanupResults = await Promise.allSettled(
          credentialCleanup.map((providerId) => deleteProviderApiKey(providerId)),
        );
        cleanupFailures = cleanupResults.filter((result) => result.status === "rejected").length;
      }
      let savedProfileFilter = voiceProfile.filterEnabled;
      if (submittedProfileFilterDirty) {
        const savedProfile = await setTargetSpeakerFilterEnabled(submittedProfileFilter);
        if (generation !== saveGeneration.current) return;
        savedProfileFilter = savedProfile.filterEnabled;
        onVoiceProfileChanged(savedProfile);
      }
      if (generation !== saveGeneration.current) return;
      const unchanged = draftEditGeneration.current === submittedEditGeneration;
      if (unchanged) setProfileFilterDraft(savedProfileFilter);
      setSaveState(unchanged ? "saved" : "idle");
      setSaveMessage(unchanged ? { kind: "saved", cleanupFailures, savedAt: Date.now() } : null);
    } catch (cause) {
      if (generation !== saveGeneration.current) return;
      setSaveState("error");
      setSaveMessage({
        kind: "error",
        message: cause instanceof Error ? cause.message : String(cause),
      });
    }
  }

  function discard() {
    setDraft(source);
    setProfileFilterDraft(voiceProfile.filterEnabled);
    setSaveState("idle");
    setSaveMessage(null);
    void setDisplayLanguagePreference(source.regional.language);
  }

  return (
    <section className="settings-page">
      <header className="settings-page-header">
        <h1>{t("settings.title")}</h1>
        <div className="settings-save-status" aria-live="polite">
          {dirty && saveState === "idle" && (
            <span className="unsaved">{t("settings.unsaved")}</span>
          )}
          {saveMessage && (
            <span className={saveMessage.kind === "error" ? "save-error" : "save-success"}>
              {saveMessage.kind === "error"
                ? localizeUiMessage(t, saveMessage.message, "settings")
                : saveMessage.cleanupFailures > 0
                  ? t("settings.savedWithCleanupFailure", { count: saveMessage.cleanupFailures })
                  : t("settings.savedAt", {
                      time: new Date(saveMessage.savedAt).toLocaleTimeString(
                        i18n.resolvedLanguage,
                        draft.regional.timeZone === "system"
                          ? undefined
                          : { timeZone: draft.regional.timeZone },
                      ),
                    })}
            </span>
          )}
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
            </button>
          ))}
        </nav>
        <div className="settings-content">
          <header className="settings-content-header">
            <h2>{activeTabMeta.label}</h2>
          </header>
          {activeTab === "general" && (
            <SettingsGeneralSection draft={draft} onChange={changeDraft} />
          )}
          {activeTab === "general" && <PersonalStateSection />}
          {activeTab === "connection" && (
            <ServiceConnectionsSection
              providers={draft.providers}
              routing={draft.routing}
              onProvidersChange={(providers) =>
                changeDraft((current) => ({ ...current, providers }))
              }
              onRoutingChange={(routing) => changeDraft((current) => ({ ...current, routing }))}
              onValidityChange={setConnectionSettingsValid}
            />
          )}
          {activeTab === "providers" && (
            <IndividualProvidersSection
              settings={draft.providers}
              codex={draft.codex}
              persistedProviderIds={persistedProviderIds}
              primaryProviderId={draft.routing.conversationRespond.primaryProviderId}
              onChange={(providers) => changeDraft((current) => ({ ...current, providers }))}
              onCodexChange={(codex) => changeDraft((current) => ({ ...current, codex }))}
            />
          )}
          {activeTab === "coding" && <CodingSettingsSection />}
          {activeTab === "routing" && (
            <RoleRoutingSection
              settings={draft.roleRouting}
              providers={draft.providers}
              codex={draft.codex}
              onChange={(roleRouting) => changeDraft((current) => ({ ...current, roleRouting }))}
            />
          )}
          {activeTab === "schedule" && <ScheduleSection />}
          {activeTab === "voice" && (
            <VoiceSettingsSection
              voice={{ ...draft.voice, listeningEnabled: voiceListeningEnabled }}
              profile={voiceProfile}
              profileFilterEnabled={profileFilterDraft}
              enrollmentBlocked={voiceEnrollmentBlocked}
              listeningBusy={voiceListeningBusy}
              availability={voiceAvailability}
              listeningError={voiceError}
              onToggleListening={onToggleVoiceListening}
              onProfileChanged={onVoiceProfileChanged}
              onProfileFilterChange={changeProfileFilter}
              onChange={(voice) =>
                changeDraft((current) => ({
                  ...current,
                  voice: { ...voice, listeningEnabled: current.voice.listeningEnabled },
                }))
              }
            />
          )}
          {activeTab === "security" && (
            <SecuritySection
              security={draft.security}
              onChange={(security) => changeDraft((current) => ({ ...current, security }))}
            />
          )}
        </div>
      </div>
      {activeTab !== "coding" && <footer className="settings-save-bar">
        <p>{dirty ? t("settings.pendingRuntime") : t("settings.showingSaved")}</p>
        <div>
          <button
            className="discard-button"
            onClick={discard}
            disabled={!dirty || saveState === "saving"}
          >
            {t("settings.discard")}
          </button>
          <button
            className="save-button"
            onClick={() => void save()}
            disabled={!dirty || !connectionSettingsValid || saveState === "saving"}
          >
            {saveState === "saving" ? t("settings.saving") : t("settings.saveSettings")}
          </button>
        </div>
      </footer>}
    </section>
  );
}
