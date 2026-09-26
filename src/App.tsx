import { Suspense, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import "./App.css";
import { getAppSnapshot, reportFrontendReady } from "./lib/runtime";
import { applySnapshotLanguage } from "./lib/appLanguage";
import { toMessage } from "./lib/appHelpers";
import type { AppSnapshot } from "./lib/contracts";
import { SettingsPage } from "./appPages";
import { DiagnosisPage } from "./features/diagnosis/DiagnosisPage";
import { MemoryPage } from "./features/memory/MemoryPage";
import { WorkPage } from "./features/work/WorkPage";
import { RecordsPage } from "./features/records/RecordsPage";
import { AuditLogPage } from "./features/audit/AuditLogPage";
import { ArtifactWorkspaceProvider } from "./features/chat/artifacts/ArtifactDrawer";
import { DesignSystemProvider } from "./design-system";
import "./design-system/styles.css";
import { AppShell } from "./shell/AppShell";
import type { AppRoute } from "./shell/appRoute";

function App() {
  const { t } = useTranslation();
  const [snapshot, setSnapshot] = useState<AppSnapshot | null>(null);
  const [route, setRoute] = useState<AppRoute>("conversation");
  const [recordTargetId, setRecordTargetId] = useState<string | null>(null);
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
    return () => { active = false; };
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
  return (
    <main className="app-shell">
      <Suspense fallback={<main className="boot-screen">{t("app.booting")}</main>}>
        <ArtifactWorkspaceProvider>
          <AppShell route={route} onRouteChange={setRoute}>
            {route === "settings" ? (
              <SettingsPage
                documents={snapshot.settings}
                voiceProfile={snapshot.voiceProfile}
                voiceEnrollmentBlocked={false}
                voiceListeningEnabled={false}
                voiceListeningBusy={false}
                voiceAvailability="stopped"
                voiceError={null}
                onToggleVoiceListening={() => undefined}
                onSaved={(settings) => {
                  setSnapshot((current) => current && ({ ...current, settings }));
                  void refreshSnapshot();
                }}
                onVoiceProfileChanged={(voiceProfile) =>
                  setSnapshot((current) => current && ({ ...current, voiceProfile }))
                }
              />
            ) : route === "memory" ? (
              <MemoryPage
                onOpenRecord={(id) => { setRecordTargetId(id); setRoute("records"); }}
                onCorrect={() => setError("会話Runtimeの再構築中は記憶の訂正依頼を受け付けられません。")}
              />
            ) : route === "work" ? (
              <WorkPage conversationId={primaryConversationId ?? undefined} onOpenSettings={() => setRoute("settings")} />
            ) : route === "audit" ? (
              <AuditLogPage />
            ) : route === "diagnosis" ? (
              <DiagnosisPage />
            ) : (
              <>
                {route === "conversation" && <p role="status">回答Runtimeは再構築中です。過去の会話は引き続き閲覧できます。</p>}
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
  return <DesignSystemProvider><App /></DesignSystemProvider>;
}
