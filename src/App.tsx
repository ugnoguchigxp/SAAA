import { FormEvent, useEffect, useMemo, useRef, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import "./App.css";
import { SettingsPage } from "./features/settings/SettingsPage";
import { SituationPage } from "./features/situation/SituationPage";
import { MeetingPage } from "./features/meeting/MeetingPage";
import {
  findSettingsDocument,
  isCodexAgentSettings,
  isModelProvidersSettings,
  isVoiceSettings,
  type AppSnapshot,
  type CodexRuntimeStatus,
  type ConversationMessage,
  type MeetingState,
  type RuntimeEvent,
  type SettingsDocument,
  type TaskMode,
} from "./lib/contracts";
import {
  cancelRun,
  createConversation,
  getCodexStatus,
  getAppSnapshot,
  listMessages,
  reportFrontendReady,
  speakText,
  startTurn,
  stopTts,
  transcribeAudio,
  reportOwnedSignal,
} from "./lib/runtime";

const initialSnapshot: AppSnapshot = { settings: [], conversations: [] };
type Surface = "chat" | "meeting" | "situation" | "settings";

function App() {
  const [snapshot, setSnapshot] = useState<AppSnapshot>(initialSnapshot);
  const [surface, setSurface] = useState<Surface>("chat");
  const [selectedConversationId, setSelectedConversationId] = useState<string | null>(null);
  const [messages, setMessages] = useState<ConversationMessage[]>([]);
  const [composer, setComposer] = useState("");
  const [taskMode, setTaskMode] = useState<TaskMode>("conversation");
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [activeRunId, setActiveRunId] = useState<string | null>(null);
  const [activeRunMode, setActiveRunMode] = useState<TaskMode | null>(null);
  const [streamingText, setStreamingText] = useState("");
  const [runtimeActivity, setRuntimeActivity] = useState<string[]>([]);
  const [lastPrompt, setLastPrompt] = useState<string | null>(null);
  const [workspacePath, setWorkspacePath] = useState("");
  const [codexStatus, setCodexStatus] = useState<CodexRuntimeStatus | null>(null);
  const [voiceState, setVoiceState] = useState<"idle" | "recording" | "transcribing">("idle");
  const [interimTranscript, setInterimTranscript] = useState("");
  const [activeVoiceRunId, setActiveVoiceRunId] = useState<string | null>(null);
  const [activeTtsRunId, setActiveTtsRunId] = useState<string | null>(null);
  const [meetingState, setMeetingState] = useState<MeetingState>("idle");
  const recorderRef = useRef<MediaRecorder | null>(null);
  const recorderChunksRef = useRef<Blob[]>([]);
  const recorderStreamRef = useRef<MediaStream | null>(null);
  const selectedConversationIdRef = useRef<string | null>(null);
  const messagesRequestRef = useRef(0);
  const activeRunIdRef = useRef<string | null>(null);
  const activeTtsRunIdRef = useRef<string | null>(null);
  const meetingStateRef = useRef<MeetingState>("idle");
  selectedConversationIdRef.current = selectedConversationId;
  meetingStateRef.current = meetingState;

  const selectedConversation = snapshot.conversations.find(
    (conversation) => conversation.id === selectedConversationId,
  );
  const modelProviderLabel = useMemo(() => {
    const document = findSettingsDocument(snapshot.settings, "providers.model", "default");
    if (!document || !isModelProvidersSettings(document.valueJson)) return "No provider configured";
    const primaryId = findPrimaryRoute(snapshot.settings);
    return document.valueJson.providers.find((provider) => provider.id === primaryId)?.label ?? primaryId;
  }, [snapshot.settings]);
  const voiceSettings = useMemo(() => {
    const document = findSettingsDocument(snapshot.settings, "voice.runtime", "default");
    return document && isVoiceSettings(document.valueJson) ? document.valueJson : null;
  }, [snapshot.settings]);
  const codexSettings = useMemo(() => {
    const document = findSettingsDocument(snapshot.settings, "providers.agent", "codex-sdk");
    return document && isCodexAgentSettings(document.valueJson) ? document.valueJson : null;
  }, [snapshot.settings]);
  const meetingActive = isMeetingBlocking(meetingState);

  useEffect(() => { void reportFrontendReady(); void initialize(); }, []);
  useEffect(() => { void getCodexStatus().then(setCodexStatus).catch((cause) => setCodexStatus({ installed: false, authenticated: false, runtime: "unavailable", accountType: null, message: toMessage(cause) })); }, []);
  useEffect(() => {
    if (!selectedConversationId) { setMessages([]); return; }
    setMessages([]);
    setStreamingText("");
    setRuntimeActivity([]);
    void loadMessages(selectedConversationId);
  }, [selectedConversationId]);
  useEffect(() => {
    const handleShortcut = (event: KeyboardEvent) => {
      const command = event.metaKey || event.ctrlKey;
      if (command && event.key.toLowerCase() === "n") {
        event.preventDefault();
        if (!activeRunId) void handleNewConversation();
      } else if (command && event.key === ",") {
        event.preventDefault();
        setSurface("settings");
      } else if (event.key === "Escape") {
        if (activeRunId) void stopActiveRun();
        if (voiceState !== "idle") void toggleVoiceCapture();
        if (activeTtsRunId) void stopSpeech();
      }
    };
    window.addEventListener("keydown", handleShortcut);
    return () => window.removeEventListener("keydown", handleShortcut);
  }, [activeRunId, activeTtsRunId, taskMode, voiceState]);
  useEffect(() => () => {
    if (recorderRef.current) recorderRef.current.onstop = null;
    recorderStreamRef.current?.getTracks().forEach((track) => track.stop());
  }, []);
  useEffect(() => {
    const input = {
      conversationState: activeRunId ? (activeRunMode === "coding" ? "agent-running" : "model-running") : composer.trim() ? "user-input" : "idle",
      microphoneState: meetingState === "active" ? "saaa-capturing" : voiceState === "recording" ? "saaa-capturing" : voiceState === "transcribing" ? "saaa-transcribing" : "inactive",
      audioState: activeTtsRunId ? "saaa-speaking" : "silent",
    } as const;
    void reportOwnedSignal(input).catch(() => undefined);
    if (input.conversationState === "idle" && input.microphoneState === "inactive" && input.audioState === "silent") return;
    const heartbeat = window.setInterval(() => { void reportOwnedSignal(input).catch(() => undefined); }, 2_000);
    return () => window.clearInterval(heartbeat);
  }, [activeRunId, activeRunMode, activeTtsRunId, composer, meetingState, voiceState]);

  async function initialize() {
    try {
      setLoading(true);
      const nextSnapshot = await getAppSnapshot();
      if (nextSnapshot.conversations.length > 0) {
        setSnapshot(nextSnapshot);
        setSelectedConversationId(nextSnapshot.conversations[0].id);
        setTaskMode(nextSnapshot.conversations[0].taskMode);
        return;
      }
      const conversation = await createConversation("conversation");
      setSnapshot({ ...nextSnapshot, conversations: [conversation] });
      setSelectedConversationId(conversation.id);
    } catch (cause) { setError(toMessage(cause)); } finally { setLoading(false); }
  }

  async function loadMessages(conversationId: string) {
    const request = ++messagesRequestRef.current;
    try {
      const nextMessages = await listMessages(conversationId);
      if (request === messagesRequestRef.current && selectedConversationIdRef.current === conversationId) {
        setMessages(nextMessages);
      }
    } catch (cause) {
      if (request === messagesRequestRef.current && selectedConversationIdRef.current === conversationId) {
        setError(toMessage(cause));
      }
    }
  }

  async function refreshSnapshot() {
    const next = await getAppSnapshot();
    setSnapshot(next);
  }

  async function handleNewConversation(mode: TaskMode = taskMode) {
    try {
      setError(null);
      const conversation = await createConversation(mode);
      setSnapshot((current) => ({ ...current, conversations: [conversation, ...current.conversations] }));
      setTaskMode(mode);
      setSelectedConversationId(conversation.id);
      setSurface("chat");
    } catch (cause) { setError(toMessage(cause)); }
  }

  async function handleSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    await submitPrompt(composer);
  }

  async function chooseWorkspace() {
    try {
      const selected = await open({ directory: true, multiple: false, title: "Choose a read-only Codex workspace" });
      if (typeof selected === "string") setWorkspacePath(selected);
    } catch (cause) {
      setError(`Workspace selection failed: ${toMessage(cause)}`);
    }
  }

  async function submitPrompt(prompt: string) {
    if (!selectedConversationId || !prompt.trim() || activeRunIdRef.current) return;
    const conversationId = selectedConversationId;
    const runMode = taskMode;
    const content = prompt.trim();
    const runId = `run_${crypto.randomUUID()}`;
    try {
      setError(null);
      setLastPrompt(content);
      activeRunIdRef.current = runId;
      setActiveRunId(runId);
      setActiveRunMode(runMode);
      setStreamingText("");
      setRuntimeActivity([]);
      setMessages((current) => [...current, { id: `pending_${runId}`, conversationId, role: "user", content, createdAt: String(Date.now()) }]);
      setComposer("");
      setSnapshot((current) => updateConversationTimestamp(current, conversationId, content));
      await startTurn(
        { runId, conversationId, content, workspacePath: workspacePath.trim() || null },
        (event) => handleRuntimeEvent(event, conversationId),
      );
    } catch (cause) {
      setError((current) => current ?? toMessage(cause));
    } finally {
      if (activeRunIdRef.current === runId) {
        activeRunIdRef.current = null;
        setActiveRunId(null);
        setActiveRunMode(null);
      }
      if (selectedConversationIdRef.current === conversationId) {
        setStreamingText("");
        await loadMessages(conversationId);
      }
    }
  }

  function handleRuntimeEvent(event: RuntimeEvent, conversationId: string) {
    if (
      selectedConversationIdRef.current !== conversationId ||
      activeRunIdRef.current !== event.runId
    ) return;
    switch (event.type) {
      case "started":
        setRuntimeActivity((current) => [...current, `${event.route} → ${event.providerId}`].slice(-8));
        break;
      case "delta":
        setStreamingText((current) => current + event.text);
        break;
      case "activity":
        setRuntimeActivity((current) => [...current, `${event.kind}: ${event.summary}`].slice(-8));
        break;
      case "providerFailed":
        setRuntimeActivity((current) => [...current, `${event.providerId} failed: ${event.reason}`].slice(-8));
        setStreamingText("");
        break;
      case "messageCompleted":
        setMessages((current) => [...current.filter((message) => !message.id.startsWith("streaming_")), event.message]);
        setStreamingText("");
        if (voiceSettings?.autoSpeak && !meetingActive) void startSpeech(event.message.content, conversationId);
        break;
      case "cancelled":
        setRuntimeActivity((current) => [...current, "Generation cancelled"].slice(-8));
        break;
      case "failed":
        setError(`${event.message} ${event.recovery}`);
        break;
    }
  }

  async function stopActiveRun() {
    const runId = activeRunIdRef.current;
    if (!runId) return;
    try { await cancelRun(runId); } catch (cause) { setError(toMessage(cause)); }
  }

  async function toggleVoiceCapture() {
    if (voiceState === "recording") {
      recorderRef.current?.stop();
      return;
    }
    if (voiceState === "transcribing" && activeVoiceRunId) {
      await cancelRun(activeVoiceRunId);
      return;
    }
    if (isMeetingBlocking(meetingStateRef.current)) {
      setError("Chat voice capture is disabled while a meeting is active or paused.");
      return;
    }
    if (!selectedConversationId || !voiceSettings) {
      setError("Voice settings are unavailable.");
      return;
    }
    if (!voiceSettings.sttModel.trim()) {
      setError("Select an absolute local whisper model path in Settings → Voice.");
      return;
    }
    try {
      setError(null);
      const audio = voiceSettings.inputDeviceId === "default"
        ? true
        : { deviceId: { exact: voiceSettings.inputDeviceId } };
      const stream = await navigator.mediaDevices.getUserMedia({ audio });
      const recorder = new MediaRecorder(stream);
      recorderChunksRef.current = [];
      recorderStreamRef.current = stream;
      recorder.ondataavailable = (event) => { if (event.data.size > 0) recorderChunksRef.current.push(event.data); };
      recorder.onstop = () => { void finishVoiceCapture(); };
      recorderRef.current = recorder;
      recorder.start(250);
      setInterimTranscript("");
      setVoiceState("recording");
    } catch (cause) {
      setError(`Microphone unavailable: ${toMessage(cause)} Check the device and microphone permission, then retry.`);
      setVoiceState("idle");
    }
  }

  async function finishVoiceCapture() {
    const stream = recorderStreamRef.current;
    stream?.getTracks().forEach((track) => track.stop());
    recorderStreamRef.current = null;
    recorderRef.current = null;
    if (!selectedConversationId || !voiceSettings) { setVoiceState("idle"); return; }
    const runId = `voice_${crypto.randomUUID()}`;
    setActiveVoiceRunId(runId);
    setVoiceState("transcribing");
    try {
      const blob = new Blob(recorderChunksRef.current);
      const context = new AudioContext();
      let buffer: AudioBuffer;
      try {
        buffer = await context.decodeAudioData(await blob.arrayBuffer());
      } finally {
        await context.close().catch(() => undefined);
      }
      const samples = mixAudioChannels(buffer);
      const transcript = await transcribeAudio({
        runId,
        conversationId: selectedConversationId,
        samples: Array.from(samples),
        sampleRate: buffer.sampleRate,
        modelPath: voiceSettings.sttModel,
      }, (event) => {
        if (event.runId !== runId) return;
        if (event.type === "transcriptDelta") setInterimTranscript((current) => `${current} ${event.text}`.trim());
        if (event.type === "transcriptFinal") setInterimTranscript(event.text);
        if (event.type === "failed") setError(`${event.message} ${event.recovery}`);
      });
      setInterimTranscript(transcript);
      await submitPrompt(transcript);
    } catch (cause) {
      setError((current) => current ?? toMessage(cause));
    } finally {
      setVoiceState("idle");
      setActiveVoiceRunId(null);
      recorderChunksRef.current = [];
    }
  }

  async function startSpeech(text: string, conversationId = selectedConversationId) {
    if (!conversationId || !voiceSettings || activeTtsRunIdRef.current || isMeetingBlocking(meetingStateRef.current)) return;
    const runId = `speech_${crypto.randomUUID()}`;
    activeTtsRunIdRef.current = runId;
    setActiveTtsRunId(runId);
    try {
      await speakText({ runId, conversationId, text, voice: voiceSettings.ttsVoice });
    } catch (cause) {
      setError(`Speech playback failed: ${toMessage(cause)}`);
    } finally {
      if (activeTtsRunIdRef.current === runId) {
        activeTtsRunIdRef.current = null;
        setActiveTtsRunId(null);
      }
    }
  }

  async function stopSpeech() {
    const runId = activeTtsRunIdRef.current;
    if (!runId) return;
    try { await stopTts(runId); } catch (cause) { setError(toMessage(cause)); }
  }

  if (loading) return <main className="boot-screen">SAAA Runtime を起動しています…</main>;

  return <main className="app-shell">
    <aside className="sidebar">
      <div className="brand"><span className="brand-mark">S</span><div><strong>SAAA</strong><small>Ambient agent runtime</small></div></div>
      <nav className="primary-nav" aria-label="Primary navigation">
        <button className={surface === "chat" ? "primary-nav-item active" : "primary-nav-item"} onClick={() => setSurface("chat")}>◌ Conversations</button>
        <button className={surface === "meeting" ? "primary-nav-item active" : "primary-nav-item"} onClick={() => setSurface("meeting")}>● Meeting {meetingActive && <span className="meeting-active-indicator">Active</span>}</button>
        <button className={surface === "situation" ? "primary-nav-item active" : "primary-nav-item"} onClick={() => setSurface("situation")}>◎ Situation</button>
        <button className={surface === "settings" ? "primary-nav-item active" : "primary-nav-item"} onClick={() => setSurface("settings")}>⚙ Settings</button>
      </nav>
      {surface === "chat" && <><button className="new-chat" onClick={() => void handleNewConversation()}><span>＋</span> New conversation</button><nav className="conversation-list" aria-label="Conversations">{snapshot.conversations.map((conversation) => <button className={conversation.id === selectedConversationId ? "conversation active" : "conversation"} key={conversation.id} onClick={() => { setSelectedConversationId(conversation.id); setTaskMode(conversation.taskMode); }}><span>{conversation.taskMode === "coding" ? "⌘" : "◌"}</span><span>{conversation.title ?? (conversation.taskMode === "coding" ? "Coding thread" : "New conversation")}</span></button>)}</nav></>}
      <div className="sidebar-status"><span className="status-dot" />SQLite local state</div>
    </aside>

    <div className="meeting-surface-host" hidden={surface !== "meeting"}><MeetingPage voiceSettings={voiceSettings} chatVoiceBusy={voiceState !== "idle"} onStateChanged={setMeetingState} /></div>
    {surface === "settings" ? <SettingsPage documents={snapshot.settings} onSaved={(settings) => setSnapshot((current) => ({ ...current, settings }))} /> : surface === "situation" ? <SituationPage onSettingsChanged={refreshSnapshot} /> : surface === "chat" ? <section className="chat-panel">
      <header className="topbar"><div><p className="eyebrow">{taskMode === "coding" ? "READ-ONLY AGENT" : "CONVERSATION"}</p><h1>{taskMode === "coding" ? "Coding assist" : "Local voice chat"}</h1></div><button className="secondary-button" onClick={() => setSurface("settings")}>Settings</button></header>
      <div className="route-banner"><span className="route-label">Effective route</span><strong>{taskMode === "coding" ? "coding.assist → codex-sdk" : `conversation.respond → ${modelProviderLabel}`}</strong><span className={taskMode === "coding" ? `badge ${codexStatus?.authenticated && codexSettings?.enabled ? "safe" : "warning"}` : "badge local"}>{taskMode === "coding" ? (codexStatus?.authenticated && codexSettings?.enabled ? "ready · read-only" : "unavailable") : "configured route"}</span></div>
      <div className="message-area" aria-live="polite">{messages.length === 0 && !streamingText ? <div className="empty-state"><p className="eyebrow">MVP 0 RUNTIME</p><h2>保存済みRouteで会話を実行します。</h2><p>Settingsで有効なProviderを登録してからメッセージを送信してください。</p></div> : messages.map((message) => <article className={`message ${message.role}`} key={message.id}><span className="message-role">{message.role === "user" ? "You" : message.role}</span><p>{message.content}</p></article>)}{streamingText && <article className="message assistant streaming"><span className="message-role">assistant · streaming</span><p>{streamingText}</p></article>}{interimTranscript && voiceState !== "idle" && <article className="message transcript streaming"><span className="message-role">transcript · {voiceState}</span><p>{interimTranscript}</p></article>}{runtimeActivity.length > 0 && <details className="activity-panel"><summary>Runtime activity</summary>{runtimeActivity.map((activity, index) => <p key={`${index}-${activity}`}>{activity}</p>)}</details>}</div>
      <form className="composer" onSubmit={handleSubmit}><div className="mode-switch" role="group" aria-label="Task mode"><button className={taskMode === "conversation" ? "selected" : ""} type="button" onClick={() => taskMode !== "conversation" && void handleNewConversation("conversation")}>Chat</button><button className={taskMode === "coding" ? "selected" : ""} type="button" onClick={() => taskMode !== "coding" && void handleNewConversation("coding")}>Coding</button></div>{taskMode === "coding" && <div className="workspace-picker"><input className="workspace-input" aria-label="Codex workspace" value={workspacePath} onChange={(event) => setWorkspacePath(event.currentTarget.value)} placeholder="Read-only workspace path" /><button className="secondary-button" type="button" onClick={() => void chooseWorkspace()}>Choose…</button></div>}<textarea aria-label="Message" onChange={(event) => setComposer(event.currentTarget.value)} onKeyDown={(event) => { if ((event.metaKey || event.ctrlKey) && event.key === "Enter") event.currentTarget.form?.requestSubmit(); }} placeholder={taskMode === "coding" ? "Ask Codex to inspect or explain…" : "Message SAAA…"} value={composer} disabled={Boolean(activeRunId)} /><div className="composer-actions"><button className={voiceState === "recording" ? "voice-button recording" : "voice-button"} type="button" onClick={() => void toggleVoiceCapture()} disabled={Boolean(activeRunId) || taskMode === "coding" || meetingActive}>{voiceState === "recording" ? "■ Stop recording" : voiceState === "transcribing" ? "■ Stop transcription" : "◉ Voice"}</button><div className="composer-primary-actions">{activeTtsRunId && <button className="stop-button" type="button" onClick={() => void stopSpeech()}>Stop speech</button>}{error && lastPrompt && !activeRunId && <button className="secondary-button" type="button" onClick={() => void submitPrompt(lastPrompt)}>Retry</button>}{activeRunId ? <button className="stop-button" type="button" onClick={() => void stopActiveRun()}>Stop</button> : <button className="send-button" type="submit" disabled={!composer.trim() || !selectedConversation || voiceState !== "idle" || (taskMode === "coding" && (!workspacePath.trim() || !codexSettings?.enabled || !codexStatus?.authenticated))}>Send ↑</button>}</div></div>{taskMode === "coding" && (!codexSettings?.enabled || !codexStatus?.authenticated) && <p className="composer-hint">{codexStatus?.message ?? "Codex runtimeを確認しています…"} Settings → Codex SDKで有効化してください。</p>}</form>
      {error && <p className="error-banner" role="alert">{error}</p>}
    </section> : null}
  </main>;
}

function findPrimaryRoute(documents: SettingsDocument[]): string {
  const routing = findSettingsDocument(documents, "routing.tasks", "default");
  if (routing && typeof routing.valueJson.conversationRespond === "object" && routing.valueJson.conversationRespond !== null) {
    const value = routing.valueJson.conversationRespond as Record<string, unknown>;
    if (typeof value.primaryProviderId === "string") return value.primaryProviderId;
  }
  return "local-openai-compatible";
}

function updateConversationTimestamp(snapshot: AppSnapshot, conversationId: string, title: string): AppSnapshot {
  const conversation = snapshot.conversations.find((item) => item.id === conversationId);
  if (!conversation) return snapshot;
  return { ...snapshot, conversations: [{ ...conversation, title: conversation.title ?? title.slice(0, 60), updatedAt: "pending" }, ...snapshot.conversations.filter((item) => item.id !== conversationId)] };
}

function toMessage(cause: unknown): string { return cause instanceof Error ? cause.message : String(cause); }

function isMeetingBlocking(state: MeetingState): boolean {
  return state === "active" || state === "paused" || state === "stopping";
}

function mixAudioChannels(buffer: AudioBuffer): Float32Array {
  const mixed = new Float32Array(buffer.length);
  for (let channel = 0; channel < buffer.numberOfChannels; channel += 1) {
    const data = buffer.getChannelData(channel);
    for (let index = 0; index < data.length; index += 1) mixed[index] += data[index] / buffer.numberOfChannels;
  }
  return mixed;
}

export default App;
