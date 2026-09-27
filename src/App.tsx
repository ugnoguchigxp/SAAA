import { Suspense, useEffect, useState, useSyncExternalStore } from "react";
import { useTranslation } from "react-i18next";
import "./App.css";
import {
  conversationAsrSnapshot,
  subscribeConversationAsr,
  startConversationAsr,
  stopConversationAsr,
} from "./lib/conversationAsrCapture";
import { getAppSnapshot, reportFrontendReady, setVoiceListeningEnabled } from "./lib/runtime";
import { applySnapshotLanguage } from "./lib/appLanguage";
import { toMessage } from "./lib/appHelpers";
import { findSettingsDocument, type AppSnapshot } from "./lib/contracts";
import { SettingsPage } from "./appPages";
import { DiagnosisPage } from "./features/diagnosis/DiagnosisPage";
import { MemoryPage } from "./features/memory/MemoryPage";
import { WorkPage } from "./features/work/WorkPage";
import { RecordsPage } from "./features/records/RecordsPage";
import { AuditLogPage } from "./features/audit/AuditLogPage";
import { ArtifactWorkspaceProvider } from "./features/chat/artifacts/ArtifactDrawer";
import { ConversationCheckPage } from "./features/chat/ConversationCheckPage";
import { DEFAULT_AGENT_NAME } from "./features/settings/settingsDefaults";
import { ProviderUnitTestPage } from "./features/providerUnitTest/ProviderUnitTestPage";
import { DesignSystemProvider } from "./design-system";
import "./design-system/styles.css";
import { AppShell } from "./shell/AppShell";
import type { AppRoute } from "./shell/appRoute";

function App() {
  const { t } = useTranslation();
  const [snapshot, setSnapshot] = useState<AppSnapshot | null>(null);
  const [route, setRoute] = useState<AppRoute>("conversation");
  const [recordTargetId, setRecordTargetId] = useState<string | null>(null);
  const audio = useSyncExternalStore(subscribeConversationAsr, conversationAsrSnapshot);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let active = true;
    void getAppSnapshot()
      .then((next) => {
        if (!active) return;
        applySnapshotLanguage(next);
        setSnapshot(next);
        void reportFrontendReady().catch((cause) => setError(toMessage(cause)));
      })
      .catch((cause) => active && setError(toMessage(cause)));
    return () => {
      active = false;
    };
  }, []);

  async function refreshSnapshot() {
    try {
      const next = await getAppSnapshot();
      applySnapshotLanguage(next);
      setSnapshot(next);
      setError(null);
    } catch (cause) {
      setError(toMessage(cause));
    }
  }

  if (!snapshot) {
    return <main className="boot-screen">{error ?? t("app.booting")}</main>;
  }
  const primaryConversationId = snapshot.primaryConversationId || null;
  const voiceSettings = findSettingsDocument(
    snapshot.settings,
    "voice.runtime",
    "default",
  )?.valueJson;
  const configuredAgentName = findSettingsDocument(
    snapshot.settings,
    "providers.agent",
    "codex-sdk",
  )?.valueJson.agentName;
  const agentName =
    typeof configuredAgentName === "string" && configuredAgentName.trim()
      ? configuredAgentName.trim()
      : DEFAULT_AGENT_NAME;
  async function toggleListening(enabled: boolean) {
    try {
      if (!enabled) await stopConversationAsr();
      const document = await setVoiceListeningEnabled(enabled);
      setSnapshot(
        (current) =>
          current && {
            ...current,
            settings: current.settings.map((entry) =>
              entry.namespace === document.namespace && entry.key === document.key
                ? document
                : entry,
            ),
          },
      );
      if (enabled)
        await startConversationAsr(
          typeof voiceSettings?.inputDeviceId === "string"
            ? voiceSettings.inputDeviceId
            : "default",
          voiceSettings?.aecEnabled !== false,
          voiceSettings?.vadSensitivity === "high" || voiceSettings?.vadSensitivity === "low"
            ? voiceSettings.vadSensitivity
            : "medium",
          typeof voiceSettings?.silenceTimeoutMs === "number"
            ? voiceSettings.silenceTimeoutMs
            : 1500,
        );
    } catch (cause) {
      setError(toMessage(cause));
    }
  }
  return (
    <main className="app-shell">
      <Suspense fallback={<main className="boot-screen">{t("app.booting")}</main>}>
        <ArtifactWorkspaceProvider>
          <AppShell route={route} onRouteChange={setRoute}>
            <div className="conversation-page-host" hidden={route !== "conversation"}>
              <ConversationCheckPage
                conversationId={primaryConversationId ?? ""}
                agentName={agentName}
                providerLabel={snapshot.effectiveRoute.label}
                inputDeviceId={
                  typeof voiceSettings?.inputDeviceId === "string"
                    ? voiceSettings.inputDeviceId
                    : "default"
                }
                echoCancellation={voiceSettings?.aecEnabled !== false}
                listeningEnabled={voiceSettings?.listeningEnabled === true}
                vadSensitivity={
                  voiceSettings?.vadSensitivity === "high" ||
                  voiceSettings?.vadSensitivity === "low"
                    ? voiceSettings.vadSensitivity
                    : "medium"
                }
                silenceTimeoutMs={
                  typeof voiceSettings?.silenceTimeoutMs === "number"
                    ? voiceSettings.silenceTimeoutMs
                    : 1_500
                }
                onToggleListening={toggleListening}
                onOpenSettings={() => setRoute("settings")}
              />
            </div>
            {route === "settings" ? (
              <SettingsPage
                documents={snapshot.settings}
                voiceProfile={snapshot.voiceProfile}
                voiceEnrollmentBlocked={audio.phase !== "idle"}
                voiceListeningEnabled={audio.phase === "recording" || audio.phase === "starting"}
                voiceListeningBusy={audio.phase === "starting" || audio.phase === "stopping"}
                voiceAvailability={audio.phase === "recording" ? "listening" : "stopped"}
                voiceError={audio.error}
                onToggleVoiceListening={(enabled) => void toggleListening(enabled)}
                onSaved={(settings) => {
                  setSnapshot((current) => current && { ...current, settings });
                  void refreshSnapshot();
                }}
                onVoiceProfileChanged={(voiceProfile) =>
                  setSnapshot((current) => current && { ...current, voiceProfile })
                }
              />
            ) : route === "conversation" ? null : route === "memory" ? (
              <MemoryPage
                onOpenRecord={(id) => {
                  setRecordTargetId(id);
                  setRoute("records");
                }}
                onCorrect={() =>
                  setError("会話Runtimeの再構築中は記憶の訂正依頼を受け付けられません。")
                }
              />
            ) : route === "work" ? (
              <WorkPage
                conversationId={primaryConversationId ?? undefined}
                onOpenSettings={() => setRoute("settings")}
              />
            ) : route === "audit" ? (
              <AuditLogPage />
            ) : route === "diagnosis" ? (
              <DiagnosisPage />
            ) : route === "unitTest" ? (
              <ProviderUnitTestPage
                inputDeviceId={
                  typeof voiceSettings?.inputDeviceId === "string"
                    ? voiceSettings.inputDeviceId
                    : "default"
                }
                echoCancellation={voiceSettings?.aecEnabled !== false}
                onOpenSettings={() => setRoute("settings")}
              />
            ) : (
              <>
                <RecordsPage
                  conversations={snapshot.conversations}
                  initialConversationId={primaryConversationId}
                  targetMessageId={recordTargetId}
                  onTargetHandled={() => setRecordTargetId(null)}
                />
              </>
            )}
          </AppShell>
        </ArtifactWorkspaceProvider>
      </Suspense>
      {error && <p role="alert">{error}</p>}
    </main>
  );
}

export default function DesignSystemApp() {
  return (
    <DesignSystemProvider>
      <App />
    </DesignSystemProvider>
  );
}
