import { PersonalStateSection } from "./PersonalStateSection";
import { CodingSettingsSection } from "../coding/CodingSettingsSection";
import { SecuritySection } from "./SecuritySection";
import { useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import type { SettingsDocument, VoiceProfileSnapshot } from "../../lib/contracts";
import { setDisplayLanguagePreference } from "../../i18n";
import { localizeUiMessage } from "../../i18n/presentation";
import { saveSettingsDocuments } from "../../lib/runtime";
import { deleteProviderApiKey } from "../../lib/providerRuntime";
import { IndividualProvidersSection } from "./IndividualProvidersSection";
import { ServiceConnectionsSection } from "./ServiceConnectionsSection";
import {
  credentialCleanupProviderIds,
  documentsFromDraft,
  draftFromDocuments,
  reconcileSavedDraft,
  type SettingsDraft,
} from "./settingsDraft";
import { defaultSettingsDraft } from "./settingsDefaults";
import { VoiceSettingsSection } from "./VoiceSettingsSection";
import { SettingsGeneralSection } from "./SettingsGeneralSection";
import { RoleRoutingSection } from "./RoleRoutingSection";
import { ScheduleSection } from "./ScheduleSection";
import type { AmbientVoiceAvailability } from "../voice/useAmbientVoiceSession";

type SettingsTab =
  | "general"
  | "connection"
  | "providers"
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
  onClose,
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
  onClose: () => void;
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
  const draftRef = useRef(draft);
  draftRef.current = draft;
  const previousSourceRef = useRef(source);
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
  const dirty = JSON.stringify(draft) !== JSON.stringify(source);
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

  async function save() {
    const generation = ++saveGeneration.current;
    const submitted = draftRef.current;
    const submittedFingerprint = JSON.stringify(submitted);
    const submittedEditGeneration = draftEditGeneration.current;
    setSaveState("saving");
    setSaveMessage(null);
    try {
      const credentialCleanup = credentialCleanupProviderIds(source, submitted);
      const saved = await saveSettingsDocuments(documentsFromDraft(submitted));
      if (generation !== saveGeneration.current) return;
      setDraft((current) => reconcileSavedDraft(current, submittedFingerprint, saved));
      onSaved(saved);
      const cleanupResults = await Promise.allSettled(
        credentialCleanup.map((providerId) => deleteProviderApiKey(providerId)),
      );
      const cleanupFailures = cleanupResults.filter(
        (result) => result.status === "rejected",
      ).length;
      if (generation !== saveGeneration.current) return;
      const unchanged = draftEditGeneration.current === submittedEditGeneration;
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
    setSaveState("idle");
    setSaveMessage(null);
    void setDisplayLanguagePreference(source.regional.language);
  }

  return (
    <section className="settings-page">
      <header className="settings-page-header">
        <div>
          <button type="button" className="text-button" onClick={onClose}>
            {t("common.back")}
          </button>
          <p className="eyebrow">{t("settings.eyebrow")}</p>
          <h1>{t("settings.title")}</h1>
          <p>{t("settings.description")}</p>
        </div>
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
              <span>{tab.detail}</span>
            </button>
          ))}
        </nav>
        <div className="settings-content">
          <header className="settings-content-header">
            <h2>{activeTabMeta.label}</h2>
            <p>{activeTabMeta.detail}</p>
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
          {activeTab === "providers" && <CodingSettingsSection />}
          {activeTab === "providers" && (
            <IndividualProvidersSection
              settings={draft.providers}
              persistedProviderIds={persistedProviderIds}
              onChange={(providers) => changeDraft((current) => ({ ...current, providers }))}
            />
          )}
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
              enrollmentBlocked={voiceEnrollmentBlocked}
              listeningBusy={voiceListeningBusy}
              availability={voiceAvailability}
              listeningError={voiceError}
              onToggleListening={onToggleVoiceListening}
              onProfileChanged={onVoiceProfileChanged}
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
      <footer className="settings-save-bar">
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
      </footer>
    </section>
  );
}
