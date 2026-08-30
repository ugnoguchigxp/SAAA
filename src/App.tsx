import { useEffect, useMemo, useRef, useState, type Dispatch, type SetStateAction } from "react";
import "./App.css";
import { AppIcon } from "./components/AppIcon";
import { ChatPage } from "./features/chat/ChatPage";
import { useConversationTurn } from "./features/chat/useConversationTurn";
import { MeetingPage } from "./features/meeting/MeetingPage";
import { SettingsPage } from "./features/settings/SettingsPage";
import { SituationPage } from "./features/situation/SituationPage";
import { usePushToTalk } from "./features/voice/usePushToTalk";
import { isMeetingBlocking, toMessage } from "./lib/appHelpers";
import {
  findSettingsDocument,
  isVoiceSettings,
  type AppSnapshot,
  type MeetingState,
} from "./lib/contracts";
import { resolveModelProviderStatus } from "./lib/conversationRouting";
import { initialConversationSession, type ConversationSession, type PendingConversationPrompt, type SubmitPromptOptions } from "./lib/conversationSession";
import { getAppSnapshot, reportFrontendReady, reportOwnedSignal } from "./lib/runtime";

const initialSnapshot: AppSnapshot = { settings: [], conversations: [], primaryConversationId: "", effectiveRoute: { providerId: null, label: "モデル未選択", location: null, state: "unchecked", fallbackUsed: false, reasonCode: "snapshot-loading", updatedAt: null }, larmRuntime: { state: "disabled", message: "LARM runtime state is loading.", contractCommit: "unknown" }, voiceProfile: { status: "empty", filterEnabled: false, runtimeAvailable: false, runtimeMessage: "Loading local speaker verification…", sampleCount: 0, targetSampleCount: 5, totalDurationMs: 0, minimumDurationMs: 20_000, threshold: 0.55, samples: [] } };
type Surface = "chat" | "meeting" | "situation" | "settings";

function App() {
  const [snapshot, setSnapshot] = useState<AppSnapshot>(initialSnapshot);
  const [surface, setSurface] = useState<Surface>("chat");
  const [selectedConversationId, setSelectedConversationId] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [meetingState, setMeetingState] = useState<MeetingState>("idle");
  const pendingVoicePromptsRef = useRef<PendingConversationPrompt[]>([]);
  const conversationSessionRef = useRef<ConversationSession>(initialConversationSession);
  const submitPromptRef = useRef<(prompt: string, options?: SubmitPromptOptions) => Promise<void>>(async () => {});
  const stopSpeechRef = useRef<() => Promise<void>>(async () => {});
  const setRuntimeActivityRef = useRef<Dispatch<SetStateAction<string[]>>>(() => {});

  const selectedConversation = snapshot.conversations.find(
    (conversation) => conversation.id === selectedConversationId,
  );
  const modelProviderStatus = useMemo(() => resolveModelProviderStatus(snapshot), [snapshot]);
  const voiceSettings = useMemo(() => {
    const document = findSettingsDocument(snapshot.settings, "voice.runtime", "default");
    return document && isVoiceSettings(document.valueJson) ? document.valueJson : null;
  }, [snapshot.settings]);
  const meetingActive = isMeetingBlocking(meetingState);

  const voice = usePushToTalk({
    selectedConversationId,
    voiceSettings,
    meetingState,
    conversationSessionRef,
    pendingVoicePromptsRef,
    setError,
    setRuntimeActivity: (value) => setRuntimeActivityRef.current(value),
    stopSpeech: () => stopSpeechRef.current(),
    submitPrompt: (prompt, options) => submitPromptRef.current(prompt, options),
  });
  const turn = useConversationTurn({
    selectedConversationId,
    voiceSettings,
    meetingState,
    isVoiceBusy: voice.isBusy,
    pendingVoicePromptsRef,
    conversationSessionRef,
    suspendVoiceForSpeech: voice.suspendVoiceForSpeech,
    resumeVoiceAfterSpeech: voice.resumeVoiceAfterSpeech,
    setSnapshot,
    setError,
  });
  submitPromptRef.current = turn.submitPrompt;
  stopSpeechRef.current = turn.stopSpeech;
  setRuntimeActivityRef.current = turn.setRuntimeActivity;
  const { voiceBusy, voiceState } = voice;
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
        if (voiceState !== "idle") void voice.toggleVoiceCapture();
        if (activeTtsRunId) void stopSpeech();
      }
    };
    window.addEventListener("keydown", handleShortcut);
    return () => window.removeEventListener("keydown", handleShortcut);
  }, [activeRunId, activeTtsRunId, surface, voiceBusy, voiceState]);
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
      const nextSnapshot = await getAppSnapshot();
      const primaryConversation = nextSnapshot.conversations.find(
        (conversation) => conversation.id === nextSnapshot.primaryConversationId,
      );
      if (!primaryConversation) throw new Error("Primary conversation is unavailable.");
      setSnapshot(nextSnapshot);
      setSelectedConversationId(primaryConversation.id);
    } catch (cause) { setError(toMessage(cause)); } finally { setLoading(false); }
  }

  async function refreshSnapshot() {
    try {
      const next = await getAppSnapshot();
      setSnapshot(next);
    } catch (cause) {
      setError(toMessage(cause));
    }
  }

  function canChangeConversation(): boolean {
    if (conversationSessionRef.current.runId) {
      setError("実行中の処理を停止してからSurfaceを切り替えてください。");
      return false;
    }
    if (voice.isBusy()) {
      setError("音声入力を停止してからSurfaceを切り替えてください。");
      return false;
    }
    return true;
  }

  function openChatSurface() {
    if (!canChangeConversation()) return;
    const primaryConversation = snapshot.conversations.find(
      (conversation) => conversation.id === snapshot.primaryConversationId,
    );
    if (!primaryConversation) {
      setError("Primary conversation is unavailable.");
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

  if (loading) return <main className="boot-screen">SAAA Runtime を起動しています…</main>;

  return <main className="app-shell">
    <aside className="sidebar">
      <div className="brand"><span className="brand-mark">S</span><strong>SAAA</strong></div>
      <nav className="primary-nav" aria-label="Primary navigation">
        <button className={surface === "chat" ? "primary-nav-item active" : "primary-nav-item"} onClick={openChatSurface}><AppIcon name="chat" />会話</button>
        <button className={surface === "meeting" ? "primary-nav-item active" : "primary-nav-item"} onClick={() => void openMeetingSurface()}><AppIcon name="calendar" />ミーティング {meetingActive && <span className="meeting-active-indicator">進行中</span>}</button>
        <button className={surface === "situation" ? "primary-nav-item active" : "primary-nav-item"} onClick={() => openAuxiliarySurface("situation")}><AppIcon name="situation" />状況</button>
      </nav>
      <button className={surface === "settings" ? "sidebar-settings active" : "sidebar-settings"} onClick={() => openAuxiliarySurface("settings")}><AppIcon name="settings" />設定</button>
    </aside>

    <div className="meeting-surface-host" hidden={surface !== "meeting"}><MeetingPage voiceSettings={voiceSettings} chatVoiceBusy={voiceBusy || Boolean(activeRunId) || Boolean(activeTtsRunId)} onStateChanged={setMeetingState} /></div>
    {surface === "settings" ? <SettingsPage documents={snapshot.settings} larmRuntime={snapshot.larmRuntime} voiceProfile={snapshot.voiceProfile} voiceEnrollmentBlocked={voiceBusy || meetingActive || Boolean(activeTtsRunId)} onSaved={(settings) => { setSnapshot((current) => ({ ...current, settings })); void refreshSnapshot(); }} onVoiceProfileChanged={(voiceProfile) => setSnapshot((current) => ({ ...current, voiceProfile }))} /> : surface === "situation" ? <SituationPage onSettingsChanged={refreshSnapshot} /> : surface === "chat" ? <ChatPage messages={turn.messages} streamingText={turn.streamingText} interimTranscript={voice.interimTranscript} voiceState={voiceState} runtimeActivity={turn.runtimeActivity} composer={composer} onComposerChange={turn.setComposer} onSubmit={(event) => void turn.handleSubmit(event)} onToggleVoice={() => void voice.toggleVoiceCapture()} voiceStarting={voice.voiceStarting} meetingActive={meetingActive} activeRunId={activeRunId} filterEnabled={snapshot.voiceProfile.filterEnabled} modelProviderStatus={modelProviderStatus} onOpenSettings={() => openAuxiliarySurface("settings")} onOpenMeeting={() => void openMeetingSurface()} onOpenSituation={() => openAuxiliarySurface("situation")} onStopRun={() => void turn.stopActiveRun()} onStopSpeech={() => void stopSpeech()} onRetry={() => void turn.retryFailedAction()} selectedConversation={selectedConversation} voiceBusy={voiceBusy} activeTtsRunId={activeTtsRunId} error={error} lastPrompt={turn.lastPrompt} retryKind={turn.retryKind} /> : null}
  </main>;
}

export default App;
