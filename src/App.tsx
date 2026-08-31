import { lazy, Suspense, useEffect, useMemo, useRef, useState, type Dispatch, type SetStateAction } from "react";
import { useTranslation } from "react-i18next";
import "./App.css";
import { AppIcon } from "./components/AppIcon";
import { ChatPage } from "./features/chat/ChatPage";
import { useConversationTurn } from "./features/chat/useConversationTurn";
import { MeetingPage } from "./features/meeting/MeetingPage";
import { useAmbientVoiceSession } from "./features/voice/useAmbientVoiceSession";
import { isMeetingBlocking, toMessage } from "./lib/appHelpers";
import {
  findSettingsDocument,
  isRegionalPreferencesSettings,
  isVoiceSettings,
  type AppSnapshot,
  type MeetingState,
} from "./lib/contracts";
import { applySnapshotLanguage } from "./lib/appLanguage";
import { uiMessage } from "./i18n/presentation";
import { resolveModelProviderStatus } from "./lib/conversationRouting";
import type { ConversationRuntimeActivity } from "./lib/conversationActivity";
import { initialConversationSession, type ConversationSession, type PendingConversationPrompt, type SubmitPromptOptions } from "./lib/conversationSession";
import { getAppSnapshot, reportFrontendReady, reportOwnedSignal, setVoiceListeningEnabled } from "./lib/runtime";

const initialSnapshot: AppSnapshot = { settings: [], conversations: [], primaryConversationId: "", effectiveRoute: { providerId: null, label: "Model not selected", location: null, state: "unchecked", fallbackUsed: false, reasonCode: "snapshot-loading", updatedAt: null }, larmRuntime: { state: "disabled", message: "LARM runtime state is loading.", contractCommit: "unknown" }, voiceProfile: { status: "empty", filterEnabled: false, runtimeAvailable: false, runtimeMessage: "Loading local speaker verification…", sampleCount: 0, targetSampleCount: 5, totalDurationMs: 0, minimumDurationMs: 20_000, threshold: 0.55, samples: [] } };
const SettingsPage = lazy(() => import("./features/settings/SettingsPage").then(({ SettingsPage }) => ({ default: SettingsPage })));
const SituationPage = lazy(() => import("./features/situation/SituationPage").then(({ SituationPage }) => ({ default: SituationPage })));
type Surface = "chat" | "meeting" | "situation" | "settings";
type ErrorSlot = "app" | "conversation" | "voice";
type ErrorSlots = Record<ErrorSlot, string | null>;

function App() {
  const { t } = useTranslation();
  const [snapshot, setSnapshot] = useState<AppSnapshot>(initialSnapshot);
  const [surface, setSurface] = useState<Surface>("chat");
  const [selectedConversationId, setSelectedConversationId] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [errors, setErrors] = useState<ErrorSlots>({ app: null, conversation: null, voice: null });
  const [meetingState, setMeetingState] = useState<MeetingState>("idle");
  const pendingVoicePromptsRef = useRef<PendingConversationPrompt[]>([]);
  const conversationSessionRef = useRef<ConversationSession>(initialConversationSession);
  const submitPromptRef = useRef<(prompt: string, options?: SubmitPromptOptions) => Promise<void>>(async () => {});
  const stopSpeechRef = useRef<() => Promise<void>>(async () => {});
  const suspendVoiceForSpeechRef = useRef<(runId: string) => Promise<boolean>>(async () => false);
  const resumeVoiceAfterSpeechRef = useRef<(runId: string) => Promise<void>>(async () => {});
  const setRuntimeActivityRef = useRef<Dispatch<SetStateAction<ConversationRuntimeActivity[]>>>(() => {});
  const selectedConversation = snapshot.conversations.find(
    (conversation) => conversation.id === selectedConversationId,
  );
  const modelProviderStatus = useMemo(() => resolveModelProviderStatus(snapshot), [snapshot]);
  const voiceSettings = useMemo(() => {
    const document = findSettingsDocument(snapshot.settings, "voice.runtime", "default");
    return document && isVoiceSettings(document.valueJson) ? document.valueJson : null;
  }, [snapshot.settings]);
  const regionalTimeZone = useMemo(() => {
    const document = findSettingsDocument(snapshot.settings, "ui.preferences", "default");
    return document && isRegionalPreferencesSettings(document.valueJson) ? document.valueJson.timeZone : "system";
  }, [snapshot.settings]);
  const meetingActive = isMeetingBlocking(meetingState);
  const error = errors.conversation ?? errors.voice ?? errors.app;
  const errorSetter = (slot: ErrorSlot): Dispatch<SetStateAction<string | null>> => (value) => {
    setErrors((current) => ({
      ...current,
      [slot]: typeof value === "function" ? value(current[slot]) : value,
    }));
  };
  const setAppError = errorSetter("app");
  const setConversationError = errorSetter("conversation");
  const setVoiceError = errorSetter("voice");

  const turn = useConversationTurn({
    selectedConversationId,
    voiceSettings,
    meetingState,
    pendingVoicePromptsRef,
    conversationSessionRef,
    suspendVoiceForSpeech: (runId) => suspendVoiceForSpeechRef.current(runId),
    resumeVoiceAfterSpeech: (runId) => resumeVoiceAfterSpeechRef.current(runId),
    setSnapshot,
    setError: setConversationError,
  });
  const voice = useAmbientVoiceSession({
    selectedConversationId,
    voiceSettings,
    voicePolicy: turn.voicePolicy,
    meetingState,
    conversationSessionRef,
    pendingVoicePromptsRef,
    setError: setVoiceError,
    setRuntimeActivity: (value) => setRuntimeActivityRef.current(value),
    stopSpeech: () => stopSpeechRef.current(),
    submitPrompt: (prompt, options) => submitPromptRef.current(prompt, options),
    persistListeningEnabled: async (enabled) => {
      const document = await setVoiceListeningEnabled(enabled);
      setSnapshot((current) => ({
        ...current,
        settings: current.settings.some((item) => item.namespace === document.namespace && item.key === document.key)
          ? current.settings.map((item) => item.namespace === document.namespace && item.key === document.key ? document : item)
          : [...current.settings, document],
      }));
    },
  });
  suspendVoiceForSpeechRef.current = voice.suspendVoiceForSpeech;
  resumeVoiceAfterSpeechRef.current = voice.resumeVoiceAfterSpeech;
  submitPromptRef.current = turn.submitPrompt;
  stopSpeechRef.current = turn.stopSpeech;
  setRuntimeActivityRef.current = turn.setRuntimeActivity;
  const { voiceBusy, voiceProcessing, voiceState } = voice;
  const { activeRunId, activeTtsRunId, composer } = turn;
  const stopSpeech = turn.stopSpeech;

  useEffect(() => { void reportFrontendReady(); void initialize(); }, []);
  useEffect(() => {
    const handleShortcut = (event: KeyboardEvent) => {
      const command = event.metaKey || event.ctrlKey;
      if (command && event.key === ",") {
        event.preventDefault();
        openAuxiliarySurface("settings");
      } else if (event.key === "Escape") {
        if (activeRunId) void turn.stopActiveRun();
        if (voice.listeningEnabled || voiceState !== "idle") void voice.toggleAmbientListening();
        if (activeTtsRunId) void stopSpeech();
      }
    };
    window.addEventListener("keydown", handleShortcut);
    return () => window.removeEventListener("keydown", handleShortcut);
  }, [activeRunId, activeTtsRunId, surface, voice.listeningEnabled, voiceBusy, voiceState]);
  useEffect(() => {
    const input = {
      conversationState: activeRunId ? "model-running" : composer.trim() ? "user-input" : "idle",
      microphoneState: meetingState === "active" ? "saaa-capturing" : voiceState === "recording" ? "saaa-capturing" : voiceState === "transcribing" ? "saaa-transcribing" : "inactive",
      audioState: activeTtsRunId ? "saaa-speaking" : "silent",
    } as const;
    void reportOwnedSignal(input).catch(() => undefined);
    if (input.conversationState === "idle" && input.microphoneState === "inactive" && input.audioState === "silent") return;
    const heartbeat = window.setInterval(() => { void reportOwnedSignal(input).catch(() => undefined); }, 2_000);
    return () => window.clearInterval(heartbeat);
  }, [activeRunId, activeTtsRunId, composer, meetingState, voiceState]);

  async function initialize() {
    try {
      setLoading(true);
      setAppError(null);
      const nextSnapshot = await getAppSnapshot();
      const primaryConversation = nextSnapshot.conversations.find(
        (conversation) => conversation.id === nextSnapshot.primaryConversationId,
      );
      if (!primaryConversation) throw new Error(uiMessage("appPrimaryConversationUnavailable"));
      applySnapshotLanguage(nextSnapshot);
      setSnapshot(nextSnapshot);
      setSelectedConversationId(primaryConversation.id);
    } catch (cause) { setAppError(toMessage(cause)); } finally { setLoading(false); }
  }

  async function refreshSnapshot() {
    try {
      const next = await getAppSnapshot();
      applySnapshotLanguage(next);
      setSnapshot(next);
      setAppError(null);
    } catch (cause) {
      setAppError(toMessage(cause));
    }
  }

  function canChangeConversation(): boolean {
    if (conversationSessionRef.current.runId) {
      setAppError(uiMessage("appSurfaceSwitchBlocked"));
      return false;
    }
    setAppError(null);
    return true;
  }

  function openChatSurface() {
    if (!canChangeConversation()) return;
    const primaryConversation = snapshot.conversations.find(
      (conversation) => conversation.id === snapshot.primaryConversationId,
    );
    if (!primaryConversation) {
      setAppError(uiMessage("appPrimaryConversationUnavailable"));
      return;
    }
    setSelectedConversationId(primaryConversation.id);
    turn.setComposer("");
    setSurface("chat");
  }

  async function openMeetingSurface() {
    if (!canChangeConversation()) return;
    if (conversationSessionRef.current.speechRunId) await stopSpeech();
    setSurface("meeting");
  }

  function openAuxiliarySurface(nextSurface: "settings" | "situation") {
    if (!canChangeConversation()) return;
    setSurface(nextSurface);
  }

  if (loading) return <main className="boot-screen">{t("app.booting")}</main>;
  return <main className="app-shell">
    <aside className="sidebar">
      <div className="brand"><span className="brand-mark">S</span><strong>SAAA</strong></div>
      <nav className="primary-nav" aria-label={t("app.navigationLabel")}>
        <button className={surface === "chat" ? "primary-nav-item active" : "primary-nav-item"} onClick={openChatSurface}><AppIcon name="chat" />{t("app.chat")}</button>
        <button className={surface === "meeting" ? "primary-nav-item active" : "primary-nav-item"} onClick={() => void openMeetingSurface()}><AppIcon name="calendar" />{t("app.meeting")} {meetingActive && <span className="meeting-active-indicator">{t("app.meetingActive")}</span>}</button>
        <button className={surface === "situation" ? "primary-nav-item active" : "primary-nav-item"} onClick={() => openAuxiliarySurface("situation")}><AppIcon name="situation" />{t("app.situation")}</button>
      </nav>
      <button className={surface === "settings" ? "sidebar-settings active" : "sidebar-settings"} onClick={() => openAuxiliarySurface("settings")}><AppIcon name="settings" />{t("app.settings")}</button>
    </aside>

    <div className="meeting-surface-host" hidden={surface !== "meeting"}><MeetingPage voiceSettings={voiceSettings} conversationBusy={voiceProcessing || Boolean(activeRunId) || Boolean(activeTtsRunId)} onBeforeCapture={voice.suspendVoiceForMeeting} onStateChanged={setMeetingState} /></div>
    <Suspense fallback={<main className="boot-screen">{t("app.booting")}</main>}>{surface === "settings" ? <SettingsPage documents={snapshot.settings} larmRuntime={snapshot.larmRuntime} voiceProfile={snapshot.voiceProfile} voiceEnrollmentBlocked={voiceBusy || meetingActive || Boolean(activeTtsRunId)} voiceListeningEnabled={voice.listeningEnabled} voiceListeningBusy={voice.voiceActionInProgress} voiceAvailability={voice.voiceAvailability} voiceError={errors.voice} onToggleVoiceListening={(enabled) => void voice.toggleAmbientListening(enabled)} onSaved={(settings) => { setSnapshot((current) => ({ ...current, settings })); void refreshSnapshot(); }} onVoiceProfileChanged={(voiceProfile) => setSnapshot((current) => ({ ...current, voiceProfile }))} /> : surface === "situation" ? <SituationPage onSettingsChanged={refreshSnapshot} timeZone={regionalTimeZone} /> : surface === "chat" ? <ChatPage messages={turn.messages} hasMoreMessages={turn.hasMoreMessages} loadingOlderMessages={turn.loadingOlderMessages} onLoadOlderMessages={turn.loadOlderMessages} streamingText={turn.streamingText} interimTranscript={voice.interimTranscript} voiceState={voiceState} listeningEnabled={voice.listeningEnabled} runtimeActivity={turn.runtimeActivity} composer={composer} onComposerChange={turn.setComposer} onSubmit={(event) => void turn.handleSubmit(event)} onToggleVoice={() => void voice.toggleAmbientListening()} voiceStarting={voice.voiceStarting} meetingActive={meetingActive} activeRunId={activeRunId} modelProviderStatus={modelProviderStatus} onOpenSettings={() => openAuxiliarySurface("settings")} onOpenMeeting={() => void openMeetingSurface()} onOpenSituation={() => openAuxiliarySurface("situation")} onStopRun={() => void turn.stopActiveRun()} onStopSpeech={() => void stopSpeech()} onRetry={() => void turn.retryFailedAction()} selectedConversation={selectedConversation} activeTtsRunId={activeTtsRunId} error={error} lastPrompt={turn.lastPrompt} retryKind={turn.retryKind} voicePolicy={turn.voicePolicy} voicePolicyUpdating={turn.voicePolicyUpdating} onSetConversationSpeechOutput={(value) => void turn.setConversationSpeechOutput(value)} onSetConversationListeningPace={(value) => void turn.setConversationListeningPace(value)} onResetConversationVoiceOverrides={() => void turn.resetConversationVoiceOverrides()} /> : null}</Suspense>
  </main>;
}

export default App;
