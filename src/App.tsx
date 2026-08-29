import { useEffect, useMemo, useRef, useState, type Dispatch, type SetStateAction } from "react";
import "./App.css";
import { AppIcon } from "./components/AppIcon";
import { ChatPage } from "./features/chat/ChatPage";
import { useConversationTurn, type StreamingSpeechSession } from "./features/chat/useConversationTurn";
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
import { getAppSnapshot, reportFrontendReady, reportOwnedSignal } from "./lib/runtime";

const initialSnapshot: AppSnapshot = { settings: [], conversations: [], primaryConversationId: "", larmRuntime: { state: "disabled", message: "LARM runtime state is loading.", contractCommit: "unknown" }, voiceProfile: { status: "empty", filterEnabled: false, runtimeAvailable: false, runtimeMessage: "Loading local speaker verification…", sampleCount: 0, targetSampleCount: 5, totalDurationMs: 0, minimumDurationMs: 20_000, threshold: 0.55, samples: [] } };
type Surface = "chat" | "meeting" | "situation" | "settings";

function App() {
  const [snapshot, setSnapshot] = useState<AppSnapshot>(initialSnapshot);
  const [surface, setSurface] = useState<Surface>("chat");
  const [selectedConversationId, setSelectedConversationId] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [meetingState, setMeetingState] = useState<MeetingState>("idle");
  const pendingVoicePromptsRef = useRef<string[]>([]);
  const activeRunIdRef = useRef<string | null>(null);
  const activeTtsRunIdRef = useRef<string | null>(null);
  const streamingSpeechSessionRef = useRef<StreamingSpeechSession | null>(null);
  const submitPromptRef = useRef<(prompt: string, allowVoiceBusy?: boolean) => Promise<void>>(async () => {});
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
    filterEnabled: snapshot.voiceProfile.filterEnabled,
    meetingState,
    activeRunIdRef,
    activeTtsRunIdRef,
    streamingSpeechSessionRef,
    pendingVoicePromptsRef,
    setError,
    setRuntimeActivity: (value) => setRuntimeActivityRef.current(value),
    stopSpeech: () => stopSpeechRef.current(),
    submitPrompt: (prompt, allowVoiceBusy) => submitPromptRef.current(prompt, allowVoiceBusy),
  });
  const turn = useConversationTurn({
    selectedConversationId,
    voiceSettings,
    meetingState,
    voiceActionRef: voice.voiceActionRef,
    voiceState: voice.voiceState,
    pendingVoicePromptsRef,
    activeRunIdRef,
    activeTtsRunIdRef,
    streamingSpeechSessionRef,
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
    const next = await getAppSnapshot();
    setSnapshot(next);
  }

  function canChangeConversation(): boolean {
    if (activeRunIdRef.current) {
      setError("実行中の処理を停止してからSurfaceを切り替えてください。");
      return false;
    }
    if (voice.voiceActionRef.current || voiceState !== "idle") {
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
    if (activeTtsRunIdRef.current || streamingSpeechSessionRef.current) await stopSpeech();
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
    {surface === "settings" ? <SettingsPage documents={snapshot.settings} larmRuntime={snapshot.larmRuntime} voiceProfile={snapshot.voiceProfile} voiceEnrollmentBlocked={voiceBusy || meetingActive || Boolean(activeTtsRunId)} onSaved={(settings) => setSnapshot((current) => ({ ...current, settings }))} onVoiceProfileChanged={(voiceProfile) => setSnapshot((current) => ({ ...current, voiceProfile }))} /> : surface === "situation" ? <SituationPage onSettingsChanged={refreshSnapshot} /> : surface === "chat" ? <ChatPage messages={turn.messages} streamingText={turn.streamingText} interimTranscript={voice.interimTranscript} voiceState={voiceState} runtimeActivity={turn.runtimeActivity} composer={composer} onComposerChange={turn.setComposer} onSubmit={(event) => void turn.handleSubmit(event)} onToggleVoice={() => void voice.toggleVoiceCapture()} voiceStarting={voice.voiceStarting} meetingActive={meetingActive} activeRunId={activeRunId} filterEnabled={snapshot.voiceProfile.filterEnabled} modelProviderStatus={modelProviderStatus} onOpenSettings={() => openAuxiliarySurface("settings")} onOpenMeeting={() => void openMeetingSurface()} onOpenSituation={() => openAuxiliarySurface("situation")} onStopRun={() => void turn.stopActiveRun()} onStopSpeech={() => void stopSpeech()} onRetry={() => { if (turn.lastPrompt) void turn.submitPrompt(turn.lastPrompt); }} selectedConversation={selectedConversation} voiceBusy={voiceBusy} activeTtsRunId={activeTtsRunId} error={error} lastPrompt={turn.lastPrompt} /> : null}
  </main>;
}

export default App;
