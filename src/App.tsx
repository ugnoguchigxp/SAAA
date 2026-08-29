import { FormEvent, useEffect, useMemo, useRef, useState } from "react";
import "./App.css";
import { SettingsPage } from "./features/settings/SettingsPage";
import { SituationPage } from "./features/situation/SituationPage";
import { MeetingPage } from "./features/meeting/MeetingPage";
import {
  findSettingsDocument,
  isModelProvidersSettings,
  isRoutingSettings,
  isVoiceSettings,
  type AppSnapshot,
  type ConversationMessage,
  type MeetingState,
  type RuntimeEvent,
  type SettingsDocument,
} from "./lib/contracts";
import {
  cancelRun,
  getAppSnapshot,
  listMessages,
  previewAudio,
  reportFrontendReady,
  speakText,
  startTurn,
  stopTts,
  transcribeAudio,
  reportOwnedSignal,
} from "./lib/runtime";
import { ensureMicrophoneAudioContextRunning, MicrophoneCaptureError, requestMicrophoneStream } from "./lib/microphone";
import { DEFAULT_VOICE_SILENCE_TIMEOUT_MS, VoiceActivityDetector } from "./lib/voiceActivity";
import { acquireAudioCapture } from "./lib/audioCaptureCoordinator";
import { StreamingSpeechChunker } from "./lib/streamingSpeech";

const initialSnapshot: AppSnapshot = { settings: [], conversations: [], primaryConversationId: "", larmRuntime: { state: "disabled", message: "LARM runtime state is loading.", contractCommit: "unknown" }, voiceProfile: { status: "empty", filterEnabled: false, runtimeAvailable: false, runtimeMessage: "Loading local speaker verification…", sampleCount: 0, targetSampleCount: 5, totalDurationMs: 0, minimumDurationMs: 20_000, threshold: 0.55, samples: [] } };
type Surface = "chat" | "meeting" | "situation" | "settings";
type QueuedVoiceSegment = { conversationId: string; model: string; samples: number[]; sampleRate: number; ttsActiveAtCapture: boolean };
type StreamingSpeechSession = {
  sourceRunId: string;
  conversationId: string;
  voice: string;
  chunker: StreamingSpeechChunker;
  receivedText: string;
  queue: string[];
  finalized: boolean;
  cancelled: boolean;
  speaking: boolean;
  interruptRunId: string | null;
};

function App() {
  const [snapshot, setSnapshot] = useState<AppSnapshot>(initialSnapshot);
  const [surface, setSurface] = useState<Surface>("chat");
  const [selectedConversationId, setSelectedConversationId] = useState<string | null>(null);
  const [messages, setMessages] = useState<ConversationMessage[]>([]);
  const [composer, setComposer] = useState("");
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [activeRunId, setActiveRunId] = useState<string | null>(null);
  const [streamingText, setStreamingText] = useState("");
  const [runtimeActivity, setRuntimeActivity] = useState<string[]>([]);
  const [lastPrompt, setLastPrompt] = useState<string | null>(null);
  const [voiceStarting, setVoiceStarting] = useState(false);
  const [voiceState, setVoiceState] = useState<"idle" | "recording" | "transcribing">("idle");
  const [interimTranscript, setInterimTranscript] = useState("");
  const [activeTtsRunId, setActiveTtsRunId] = useState<string | null>(null);
  const [meetingState, setMeetingState] = useState<MeetingState>("idle");
  const voiceStreamRef = useRef<MediaStream | null>(null);
  const voiceContextRef = useRef<AudioContext | null>(null);
  const voiceSourceRef = useRef<MediaStreamAudioSourceNode | null>(null);
  const voiceNodeRef = useRef<AudioWorkletNode | null>(null);
  const voiceFramesRef = useRef<Float32Array[]>([]);
  const voiceFrameSamplesRef = useRef(0);
  const voiceFlushResolverRef = useRef<(() => void) | null>(null);
  const voicePreviewStartedRef = useRef(false);
  const voicePreviewRunIdRef = useRef<string | null>(null);
  const voicePreviewPromiseRef = useRef<Promise<void> | null>(null);
  const voiceActionRef = useRef(false);
  const voiceFinalizingRef = useRef(false);
  const activeVoiceRunIdRef = useRef<string | null>(null);
  const voiceCancellationRequestedRef = useRef(false);
  const voiceActivityDetectorRef = useRef<VoiceActivityDetector | null>(null);
  const voiceCaptureLeaseRef = useRef<(() => void) | null>(null);
  const voiceSegmentQueueRef = useRef<QueuedVoiceSegment[]>([]);
  const voiceSegmentProcessingRef = useRef(false);
  const pendingVoicePromptsRef = useRef<string[]>([]);
  const targetSpeakerFilterEnabledRef = useRef(false);
  const selectedConversationIdRef = useRef<string | null>(null);
  const messagesRequestRef = useRef(0);
  const activeRunIdRef = useRef<string | null>(null);
  const activeTtsRunIdRef = useRef<string | null>(null);
  const streamingSpeechSessionRef = useRef<StreamingSpeechSession | null>(null);
  const speechDisabledRunIdsRef = useRef(new Set<string>());
  const meetingStateRef = useRef<MeetingState>("idle");
  selectedConversationIdRef.current = selectedConversationId;
  meetingStateRef.current = meetingState;
  targetSpeakerFilterEnabledRef.current = snapshot.voiceProfile.filterEnabled;

  const selectedConversation = snapshot.conversations.find(
    (conversation) => conversation.id === selectedConversationId,
  );
  const modelProviderStatus = useMemo(() => {
    const document = findSettingsDocument(snapshot.settings, "providers.model", "default");
    if (!document || !isModelProvidersSettings(document.valueJson)) {
      return { ready: false, label: "モデル未選択" };
    }
    const providers = document.valueJson.providers;
    const primaryId = findPrimaryRoute(snapshot.settings);
    const primary = providers.find((provider) => provider.id === primaryId);
    if (!primary) return { ready: false, label: "モデル未選択" };
    if (primary?.kind === "larm" && snapshot.larmRuntime.state !== "ready") {
      const routing = findSettingsDocument(snapshot.settings, "routing.tasks", "default");
      const fallbackIds = routing && isRoutingSettings(routing.valueJson)
        ? routing.valueJson.conversationRespond.fallbackProviderIds
        : [];
      const fallback = fallbackIds
        .map((id) => providers.find((provider) => provider.id === id))
        .find((provider) => provider?.enabled && provider.kind !== "larm");
      if (fallback?.kind === "openai-compatible") {
        return {
          ready: Boolean(fallback.endpoint.trim() && fallback.model.trim()),
          label: fallback.label || fallback.id,
        };
      }
      return { ready: false, label: primary.label || primary.id };
    }
    return {
      ready: primary.kind === "larm"
        ? primary.enabled && snapshot.larmRuntime.state === "ready"
        : primary.enabled && Boolean(primary.endpoint.trim() && primary.model.trim()),
      label: primary.label || primary.id,
    };
  }, [snapshot.larmRuntime.state, snapshot.settings]);
  const voiceSettings = useMemo(() => {
    const document = findSettingsDocument(snapshot.settings, "voice.runtime", "default");
    return document && isVoiceSettings(document.valueJson) ? document.valueJson : null;
  }, [snapshot.settings]);
  const meetingActive = isMeetingBlocking(meetingState);
  const voiceBusy = voiceStarting || voiceState !== "idle";

  useEffect(() => { void reportFrontendReady(); void initialize(); }, []);
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
      if (command && event.key === ",") {
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
  }, [activeRunId, activeTtsRunId, voiceState]);
  useEffect(() => () => {
    const previewRunId = voicePreviewRunIdRef.current;
    if (previewRunId) void cancelRun(previewRunId);
    voiceNodeRef.current?.disconnect();
    voiceSourceRef.current?.disconnect();
    voiceStreamRef.current?.getTracks().forEach((track) => track.stop());
    void voiceContextRef.current?.close();
    voiceCaptureLeaseRef.current?.();
    voiceCaptureLeaseRef.current = null;
    voiceFramesRef.current = [];
    voiceFrameSamplesRef.current = 0;
    for (const segment of voiceSegmentQueueRef.current) segment.samples = [];
    voiceSegmentQueueRef.current = [];
    pendingVoicePromptsRef.current = [];
    const speechSession = streamingSpeechSessionRef.current;
    if (speechSession) {
      speechSession.cancelled = true;
      speechSession.queue = [];
      speechSession.chunker.reset();
      streamingSpeechSessionRef.current = null;
    }
    const ttsRunId = activeTtsRunIdRef.current;
    if (ttsRunId) void stopTts(ttsRunId);
  }, []);
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

  async function handleSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    await submitPrompt(composer);
  }

  async function submitPrompt(prompt: string, allowVoiceBusy = false) {
    if (
      !selectedConversationId
      || !prompt.trim()
      || activeRunIdRef.current
      || (!allowVoiceBusy && (voiceActionRef.current || voiceState !== "idle"))
    ) return;
    const conversationId = selectedConversationId;
    const content = prompt.trim();
    const runId = `run_${crypto.randomUUID()}`;
    const shouldStreamSpeech = Boolean(voiceSettings?.autoSpeak)
      && !isMeetingBlocking(meetingStateRef.current);
    try {
      setError(null);
      setLastPrompt(content);
      activeRunIdRef.current = runId;
      setActiveRunId(runId);
      setStreamingText("");
      setRuntimeActivity([]);
      setMessages((current) => [...current, { id: `pending_${runId}`, conversationId, role: "user", content, createdAt: String(Date.now()) }]);
      setComposer("");
      setSnapshot((current) => updateConversationTimestamp(current, conversationId, content));
      if (shouldStreamSpeech && voiceSettings) {
        await stopSpeech();
        beginStreamingSpeech(runId, conversationId, voiceSettings.ttsVoice);
      }
      await startTurn(
        { runId, conversationId, content, workspacePath: null },
        (event) => handleRuntimeEvent(event, conversationId),
      );
    } catch (cause) {
      setError((current) => current ?? toMessage(cause));
    } finally {
      if (activeRunIdRef.current === runId) {
        activeRunIdRef.current = null;
        setActiveRunId(null);
      }
      if (selectedConversationIdRef.current === conversationId) {
        setStreamingText("");
        await loadMessages(conversationId);
      }
      speechDisabledRunIdsRef.current.delete(runId);
      const nextVoicePrompt = pendingVoicePromptsRef.current.shift();
      if (nextVoicePrompt && selectedConversationIdRef.current === conversationId) {
        if (activeTtsRunIdRef.current) await stopSpeech();
        else if (streamingSpeechSessionRef.current) await stopSpeech();
        await submitPrompt(nextVoicePrompt, true);
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
      case "providerSelected":
        setRuntimeActivity((current) => [...current, `${event.routeId} → ${event.runtimeId} · ${event.selectionReasonCode}${event.fallbackUsed ? " · fallback" : ""}`].slice(-8));
        break;
      case "delta":
        setStreamingText((current) => current + event.text);
        appendStreamingSpeech(event.runId, event.text);
        break;
      case "activity":
        setRuntimeActivity((current) => [...current, `${event.kind}: ${event.summary}`].slice(-8));
        break;
      case "providerFailed":
        setRuntimeActivity((current) => [...current, `${event.providerId} failed: ${event.reason}`].slice(-8));
        setStreamingText("");
        resetStreamingSpeech(event.runId);
        break;
      case "messageCompleted":
        setMessages((current) => [...current.filter((message) => !message.id.startsWith("streaming_")), event.message]);
        setStreamingText("");
        if (voiceSettings?.autoSpeak && !isMeetingBlocking(meetingStateRef.current)) {
          if (streamingSpeechSessionRef.current?.sourceRunId === event.runId) {
            finishStreamingSpeech(event.runId, event.message.content);
          } else if (!speechDisabledRunIdsRef.current.has(event.runId)) {
            void startSpeech(event.message.content, conversationId);
          }
        }
        break;
      case "cancelled":
        setRuntimeActivity((current) => [...current, "Generation cancelled"].slice(-8));
        void stopStreamingSpeech(event.runId);
        break;
      case "failed":
        setError(`${event.message} ${event.recovery}`);
        void stopStreamingSpeech(event.runId);
        break;
    }
  }

  async function stopActiveRun() {
    const runId = activeRunIdRef.current;
    if (!runId) return;
    try { await cancelRun(runId); } catch (cause) { setError(toMessage(cause)); }
  }

  async function toggleVoiceCapture() {
    if (voiceActionRef.current) return;
    voiceActionRef.current = true;
    try {
      if (voiceState === "recording") {
        void finishVoiceCapture(false);
        return;
      }
      const voiceRunId = activeVoiceRunIdRef.current;
      if (voiceState === "transcribing" && voiceRunId) {
        voiceCancellationRequestedRef.current = true;
        try {
          await cancelRun(voiceRunId);
        } catch (cause) {
          voiceCancellationRequestedRef.current = false;
          setError(toMessage(cause));
        }
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
      if (voiceSettings.sttProviderId !== "gnosis-asr" || voiceSettings.sttModel !== "qwen3-asr-1.7b") {
        setError("Voice settings must use the gnosis ASR provider.");
        return;
      }
      try {
        setError(null);
        setVoiceStarting(true);
        voiceCaptureLeaseRef.current = acquireAudioCapture("chat");
        const audio: MediaTrackConstraints = {
          autoGainControl: true,
          echoCancellation: true,
          noiseSuppression: true,
          ...(voiceSettings.inputDeviceId === "default"
            ? {}
            : { deviceId: { exact: voiceSettings.inputDeviceId } }),
        };
        const stream = await requestMicrophoneStream(audio);
        voiceStreamRef.current = stream;
        const context = new AudioContext();
        voiceContextRef.current = context;
        await context.audioWorklet.addModule("/audio/meeting-processor.js");
        const source = context.createMediaStreamSource(stream);
        const node = new AudioWorkletNode(context, "meeting-processor");
        voiceSourceRef.current = source;
        voiceNodeRef.current = node;
        voiceFramesRef.current = [];
        voiceFrameSamplesRef.current = 0;
        voicePreviewStartedRef.current = false;
        voiceActivityDetectorRef.current = new VoiceActivityDetector({ sampleRate: context.sampleRate });
        node.port.onmessage = (event: MessageEvent<Float32Array | { type: "flushed" }>) => {
          if (!(event.data instanceof Float32Array)) {
            if (event.data.type === "flushed") voiceFlushResolverRef.current?.();
            return;
          }
          voiceFramesRef.current.push(event.data);
          voiceFrameSamplesRef.current += event.data.length;
          if (voiceActivityDetectorRef.current?.observe(event.data).shouldFinalize) {
            void finishVoiceCapture(targetSpeakerFilterEnabledRef.current);
            return;
          }
          if (!targetSpeakerFilterEnabledRef.current && !voicePreviewStartedRef.current && voiceFrameSamplesRef.current >= context.sampleRate * 2) {
            startVoicePreview(context.sampleRate);
          }
        };
        source.connect(node);
        node.connect(context.destination);
        await ensureMicrophoneAudioContextRunning(context);
        setInterimTranscript("");
        setVoiceState("recording");
      } catch (cause) {
        await detachVoiceCapture(false);
        setError(cause instanceof MicrophoneCaptureError
          ? cause.message
          : `Voice capture initialization failed: ${toMessage(cause)}`);
        setVoiceState("idle");
      }
    } finally {
      setVoiceStarting(false);
      voiceActionRef.current = false;
    }
  }

  function startVoicePreview(sampleRate: number) {
    if (!selectedConversationId || !voiceSettings || voicePreviewStartedRef.current || targetSpeakerFilterEnabledRef.current) return;
    voicePreviewStartedRef.current = true;
    const runId = `voice_preview_${crypto.randomUUID().replace(/-/g, "")}`;
    const samples = mergePcmFrames(voiceFramesRef.current, voiceFrameSamplesRef.current);
    voicePreviewRunIdRef.current = runId;
    const preview = previewAudio({
      runId,
      conversationId: selectedConversationId,
      samples: Array.from(samples),
      sampleRate,
      model: voiceSettings.sttModel,
    }, (event) => {
      if (event.runId === runId && event.type === "transcriptDelta") {
        setInterimTranscript(event.text);
      }
    }).then(() => undefined).catch(() => undefined).finally(() => {
      if (voicePreviewRunIdRef.current === runId) {
        voicePreviewRunIdRef.current = null;
        voicePreviewPromiseRef.current = null;
      }
    });
    voicePreviewPromiseRef.current = preview;
  }

  async function detachVoiceCapture(flush: boolean) {
    if (flush && voiceNodeRef.current) {
      await new Promise<void>((resolve) => {
        let completed = false;
        const finish = () => {
          if (completed) return;
          completed = true;
          voiceFlushResolverRef.current = null;
          window.clearTimeout(timeout);
          resolve();
        };
        const timeout = window.setTimeout(finish, 250);
        voiceFlushResolverRef.current = finish;
        voiceNodeRef.current?.port.postMessage({ type: "flush" });
      });
    }
    voiceNodeRef.current?.disconnect();
    voiceSourceRef.current?.disconnect();
    voiceStreamRef.current?.getTracks().forEach((track) => track.stop());
    voiceNodeRef.current = null;
    voiceSourceRef.current = null;
    voiceStreamRef.current = null;
    if (voiceContextRef.current) await voiceContextRef.current.close().catch(() => undefined);
    voiceContextRef.current = null;
    voiceActivityDetectorRef.current = null;
    voiceCaptureLeaseRef.current?.();
    voiceCaptureLeaseRef.current = null;
  }

  async function finishVoiceCapture(keepListening: boolean) {
    if (voiceFinalizingRef.current) return;
    voiceFinalizingRef.current = true;
    if (!selectedConversationId || !voiceSettings) {
      try {
        await detachVoiceCapture(false);
      } finally {
        setVoiceState("idle");
        voiceFinalizingRef.current = false;
      }
      return;
    }
    try {
      const sampleRate = voiceContextRef.current?.sampleRate;
      if (!keepListening) {
        voiceActivityDetectorRef.current = null;
        setVoiceState("transcribing");
        await detachVoiceCapture(true);
      }
      if (!sampleRate) throw new Error("Recorded audio is unavailable.");
      if (voiceFrameSamplesRef.current === 0) return;
      const samples = Array.from(mergePcmFrames(voiceFramesRef.current, voiceFrameSamplesRef.current));
      voiceFramesRef.current = [];
      voiceFrameSamplesRef.current = 0;
      voicePreviewStartedRef.current = false;
      if (keepListening && voiceContextRef.current) {
        voiceActivityDetectorRef.current = new VoiceActivityDetector({ sampleRate: voiceContextRef.current.sampleRate });
        setVoiceState("recording");
      }
      const previewRunId = voicePreviewRunIdRef.current;
      if (previewRunId) await cancelRun(previewRunId).catch(() => undefined);
      await voicePreviewPromiseRef.current?.catch(() => undefined);
      enqueueVoiceSegment({
        conversationId: selectedConversationId,
        model: voiceSettings.sttModel,
        samples,
        sampleRate,
        ttsActiveAtCapture: activeTtsRunIdRef.current !== null,
      });
    } catch (cause) {
      setError((current) => current ?? toMessage(cause));
    } finally {
      voiceFinalizingRef.current = false;
      if (!voiceStreamRef.current && !voiceSegmentProcessingRef.current) setVoiceState("idle");
    }
  }

  function enqueueVoiceSegment(segment: QueuedVoiceSegment) {
    if (voiceSegmentQueueRef.current.length >= 2) {
      setError("音声処理が追いつかないため、新しい発話は送信しませんでした。");
      return;
    }
    voiceSegmentQueueRef.current.push(segment);
    void drainVoiceSegments();
  }

  async function drainVoiceSegments() {
    if (voiceSegmentProcessingRef.current) return;
    voiceSegmentProcessingRef.current = true;
    try {
      while (voiceSegmentQueueRef.current.length > 0) {
        const segment = voiceSegmentQueueRef.current.shift();
        if (!segment) break;
        const runId = `voice_${crypto.randomUUID()}`;
        activeVoiceRunIdRef.current = runId;
        voiceCancellationRequestedRef.current = false;
        if (!voiceStreamRef.current) setVoiceState("transcribing");
        try {
          const transcript = await transcribeAudio({
            runId,
            conversationId: segment.conversationId,
            samples: segment.samples,
            sampleRate: segment.sampleRate,
            model: segment.model,
          }, (event) => {
            if (event.runId !== runId) return;
            if (event.type === "transcriptDelta" || event.type === "transcriptFinal") setInterimTranscript(event.text);
          });
          segment.samples = [];
          if (voiceCancellationRequestedRef.current || !transcript.trim()) continue;
          setInterimTranscript(transcript);
          if (activeTtsRunIdRef.current) await stopSpeech();
          else if (streamingSpeechSessionRef.current) await stopSpeech();
          if (activeRunIdRef.current) {
            if (pendingVoicePromptsRef.current.length >= 2) {
              setError("応答待ちの音声クエリーが上限に達したため、新しい発話は送信しませんでした。");
            } else {
              pendingVoicePromptsRef.current.push(transcript);
              setRuntimeActivity((current) => [...current, "Voice query queued until the active response completes"].slice(-8));
            }
          } else {
            void submitPrompt(transcript, true);
          }
        } catch (cause) {
          segment.samples = [];
          const message = toMessage(cause);
          if (!voiceCancellationRequestedRef.current && !(segment.ttsActiveAtCapture && message.startsWith("TARGET_SPEAKER_REJECTED"))) {
            setError(message.startsWith("TARGET_SPEAKER_REJECTED") ? "登録した本人の声として確認できなかったため、文字起こしへ送信しませんでした。" : message);
          }
        } finally {
          if (activeVoiceRunIdRef.current === runId) activeVoiceRunIdRef.current = null;
          voiceCancellationRequestedRef.current = false;
        }
      }
    } finally {
      voiceSegmentProcessingRef.current = false;
      setVoiceState(voiceStreamRef.current ? "recording" : "idle");
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

  function beginStreamingSpeech(sourceRunId: string, conversationId: string, voice: string) {
    speechDisabledRunIdsRef.current.delete(sourceRunId);
    streamingSpeechSessionRef.current = {
      sourceRunId,
      conversationId,
      voice,
      chunker: new StreamingSpeechChunker(),
      receivedText: "",
      queue: [],
      finalized: false,
      cancelled: false,
      speaking: false,
      interruptRunId: null,
    };
  }

  function appendStreamingSpeech(sourceRunId: string, text: string) {
    const session = streamingSpeechSessionRef.current;
    if (!session || session.sourceRunId !== sourceRunId || session.cancelled || session.finalized) return;
    session.receivedText += text;
    session.queue.push(...session.chunker.push(text));
    void pumpStreamingSpeech(session);
  }

  function finishStreamingSpeech(sourceRunId: string, finalText: string) {
    const session = streamingSpeechSessionRef.current;
    if (!session || session.sourceRunId !== sourceRunId || session.cancelled || session.finalized) return;
    if (finalText.startsWith(session.receivedText)) {
      const missingSuffix = finalText.slice(session.receivedText.length);
      if (missingSuffix) session.queue.push(...session.chunker.push(missingSuffix));
    }
    session.queue.push(...session.chunker.finish());
    session.finalized = true;
    void pumpStreamingSpeech(session);
  }

  function resetStreamingSpeech(sourceRunId: string) {
    const session = streamingSpeechSessionRef.current;
    if (!session || session.sourceRunId !== sourceRunId || session.cancelled) return;
    session.receivedText = "";
    session.queue = [];
    session.chunker.reset();
    const ttsRunId = activeTtsRunIdRef.current;
    if (ttsRunId) {
      session.interruptRunId = ttsRunId;
      void stopTts(ttsRunId).catch((cause) => setError(toMessage(cause)));
    }
  }

  async function pumpStreamingSpeech(session: StreamingSpeechSession) {
    if (session.speaking || session.cancelled) return;
    session.speaking = true;
    try {
      while (streamingSpeechSessionRef.current === session && !session.cancelled) {
        const text = session.queue.shift();
        if (!text) break;
        const runId = `speech_${crypto.randomUUID()}`;
        activeTtsRunIdRef.current = runId;
        setActiveTtsRunId(runId);
        try {
          await speakText({
            runId,
            conversationId: session.conversationId,
            text,
            voice: session.voice,
          });
        } catch (cause) {
          if (!session.cancelled && session.interruptRunId !== runId) {
            session.cancelled = true;
            session.queue = [];
            speechDisabledRunIdsRef.current.add(session.sourceRunId);
            setError(`Speech playback failed: ${toMessage(cause)}`);
          }
        } finally {
          if (session.interruptRunId === runId) session.interruptRunId = null;
          if (activeTtsRunIdRef.current === runId) {
            activeTtsRunIdRef.current = null;
            setActiveTtsRunId(null);
          }
        }
      }
    } finally {
      session.speaking = false;
      if (
        streamingSpeechSessionRef.current === session
        && (session.cancelled || (session.finalized && session.queue.length === 0))
      ) {
        streamingSpeechSessionRef.current = null;
      }
    }
  }

  async function stopStreamingSpeech(sourceRunId: string) {
    if (streamingSpeechSessionRef.current?.sourceRunId !== sourceRunId) return;
    await stopSpeech();
  }

  async function stopSpeech() {
    const session = streamingSpeechSessionRef.current;
    if (session) {
      if (session.finalized) speechDisabledRunIdsRef.current.delete(session.sourceRunId);
      else speechDisabledRunIdsRef.current.add(session.sourceRunId);
      session.cancelled = true;
      session.queue = [];
      session.chunker.reset();
      streamingSpeechSessionRef.current = null;
    }
    const runId = activeTtsRunIdRef.current;
    if (!runId) return;
    try {
      await stopTts(runId);
    } catch (cause) {
      setError(toMessage(cause));
    } finally {
      if (activeTtsRunIdRef.current === runId) {
        activeTtsRunIdRef.current = null;
        setActiveTtsRunId(null);
      }
    }
  }

  if (loading) return <main className="boot-screen">SAAA Runtime を起動しています…</main>;

  return <main className="app-shell">
    <aside className="sidebar">
      <div className="brand"><span className="brand-mark">S</span><strong>SAAA</strong></div>
      <nav className="primary-nav" aria-label="Primary navigation">
        <button className={surface === "chat" ? "primary-nav-item active" : "primary-nav-item"} onClick={() => setSurface("chat")}><AppIcon name="chat" />会話</button>
        <button className={surface === "meeting" ? "primary-nav-item active" : "primary-nav-item"} onClick={() => setSurface("meeting")}><AppIcon name="calendar" />ミーティング {meetingActive && <span className="meeting-active-indicator">進行中</span>}</button>
        <button className={surface === "situation" ? "primary-nav-item active" : "primary-nav-item"} onClick={() => setSurface("situation")}><AppIcon name="situation" />状況</button>
      </nav>
      <button className={surface === "settings" ? "sidebar-settings active" : "sidebar-settings"} onClick={() => setSurface("settings")}><AppIcon name="settings" />設定</button>
    </aside>

    <div className="meeting-surface-host" hidden={surface !== "meeting"}><MeetingPage voiceSettings={voiceSettings} chatVoiceBusy={voiceBusy} onStateChanged={setMeetingState} /></div>
    {surface === "settings" ? <SettingsPage documents={snapshot.settings} larmRuntime={snapshot.larmRuntime} voiceProfile={snapshot.voiceProfile} voiceEnrollmentBlocked={voiceBusy || meetingActive || Boolean(activeTtsRunId)} onSaved={(settings) => setSnapshot((current) => ({ ...current, settings }))} onVoiceProfileChanged={(voiceProfile) => setSnapshot((current) => ({ ...current, voiceProfile }))} /> : surface === "situation" ? <SituationPage onSettingsChanged={refreshSnapshot} /> : surface === "chat" ? <section className="chat-panel">
      <header className="topbar"><div><p className="eyebrow">CONTINUOUS CONVERSATION</p><h1>SAAAとの会話</h1></div><div className="topbar-status"><span className="status-pill local-status"><span className="status-dot" />ローカル処理</span><button className={!modelProviderStatus.ready ? "status-pill provider-status warning" : "status-pill provider-status"} onClick={() => setSurface("settings")}><AppIcon name="model" />{modelProviderStatus.ready ? modelProviderStatus.label : "モデル未選択"}</button></div></header>
      <div className="message-area" aria-live="polite">{messages.length === 0 && !streamingText ? <div className="empty-state"><h2>今日はどうしましたか？</h2><p>声で話すか、メッセージを入力してください。</p><div className="suggestion-list"><button type="button" onClick={() => setComposer("考えを整理するのを手伝ってください")}>考えを整理する</button><button type="button" onClick={() => setSurface("meeting")}>会議を文字起こし</button><button type="button" onClick={() => setSurface("situation")}>状況を確認</button></div></div> : messages.map((message) => <article className={`message ${message.role}`} key={message.id}><span className="message-role">{message.role === "user" ? "You" : message.role}</span><p>{message.content}</p></article>)}{streamingText && <article className="message assistant streaming"><span className="message-role">assistant · streaming</span><p>{streamingText}</p></article>}{interimTranscript && voiceState !== "idle" && <article className="message transcript streaming"><span className="message-role">transcript · {voiceState}</span><p>{interimTranscript}</p></article>}{runtimeActivity.length > 0 && <details className="activity-panel"><summary>Runtime activity</summary>{runtimeActivity.map((activity, index) => <p key={`${index}-${activity}`}>{activity}</p>)}</details>}</div>
      <form className="composer" onSubmit={handleSubmit}><div className="composer-row"><button className={voiceState === "recording" ? "voice-button recording" : "voice-button"} type="button" aria-label={voiceStarting ? "マイクを準備中" : voiceState === "recording" ? "録音を停止" : voiceState === "transcribing" ? "文字起こしを停止" : "音声入力"} title={voiceStarting ? "マイクを準備中" : voiceState === "recording" ? "録音を停止（無音で自動送信）" : voiceState === "transcribing" ? "文字起こしを停止" : "音声入力"} onClick={() => void toggleVoiceCapture()} disabled={voiceStarting || meetingActive || (Boolean(activeRunId) && !snapshot.voiceProfile.filterEnabled)}><AppIcon name={voiceState === "recording" || voiceState === "transcribing" ? "stop" : "mic"} /></button><textarea rows={1} aria-label="Message" onChange={(event) => setComposer(event.currentTarget.value)} onKeyDown={(event) => { if ((event.metaKey || event.ctrlKey) && event.key === "Enter") event.currentTarget.form?.requestSubmit(); }} placeholder="SAAAにメッセージ" value={composer} disabled={Boolean(activeRunId)} /><div className="composer-end">{activeRunId ? <button className="stop-button composer-stop" type="button" onClick={() => void stopActiveRun()}><AppIcon name="stop" /><span>停止</span></button> : <button className="send-button" type="submit" aria-label="送信" disabled={!composer.trim() || !selectedConversation || voiceBusy}><AppIcon name="send" /></button>}</div></div><div className="composer-meta">{voiceState === "recording" && <span className="composer-hint">話し終えて{DEFAULT_VOICE_SILENCE_TIMEOUT_MS / 1_000}秒待つと自動送信します。{snapshot.voiceProfile.filterEnabled ? " 本人フィルター中は応答・読み上げ中も待ち受けます。" : ""}</span>}{activeTtsRunId && <button className="text-button" type="button" onClick={() => void stopSpeech()}>読み上げを停止</button>}{error && lastPrompt && !activeRunId && <button className="text-button" type="button" onClick={() => void submitPrompt(lastPrompt)}>再試行</button>}</div></form>
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
  return "gnosis-qwen";
}

function updateConversationTimestamp(snapshot: AppSnapshot, conversationId: string, title: string): AppSnapshot {
  const conversation = snapshot.conversations.find((item) => item.id === conversationId);
  if (!conversation) return snapshot;
  return { ...snapshot, conversations: [{ ...conversation, title: conversation.title ?? title.slice(0, 60), updatedAt: "pending" }, ...snapshot.conversations.filter((item) => item.id !== conversationId)] };
}

function toMessage(cause: unknown): string { return cause instanceof Error ? cause.message : String(cause); }

function isMeetingBlocking(state: MeetingState): boolean {
  return state === "preflight" || state === "active" || state === "paused" || state === "stopping";
}

function mergePcmFrames(frames: Float32Array[], length: number): Float32Array {
  const merged = new Float32Array(length);
  let offset = 0;
  for (const frame of frames) {
    merged.set(frame, offset);
    offset += frame.length;
  }
  return merged;
}

type AppIconName = "calendar" | "chat" | "mic" | "model" | "send" | "settings" | "situation" | "stop";

function AppIcon({ name }: { name: AppIconName }) {
  const common = { width: 20, height: 20, viewBox: "0 0 24 24", fill: "none", stroke: "currentColor", strokeWidth: 1.8, strokeLinecap: "round" as const, strokeLinejoin: "round" as const, "aria-hidden": true };
  switch (name) {
    case "calendar":
      return <svg {...common}><path d="M7 3v3M17 3v3M4 9h16" /><rect x="4" y="5" width="16" height="16" rx="3" /></svg>;
    case "chat":
      return <svg {...common}><path d="M20 15a4 4 0 0 1-4 4H9l-5 3 1.5-4.5A8 8 0 1 1 20 15Z" /></svg>;
    case "mic":
      return <svg {...common}><rect x="8" y="3" width="8" height="13" rx="4" /><path d="M5 11a7 7 0 0 0 14 0M12 18v3M9 21h6" /></svg>;
    case "model":
      return <svg {...common}><path d="M8 4a4 4 0 0 0-3 6.7A4.5 4.5 0 0 0 8.5 19H10V5.5A1.5 1.5 0 0 0 8.5 4ZM16 4a4 4 0 0 1 3 6.7A4.5 4.5 0 0 1 15.5 19H14V5.5A1.5 1.5 0 0 1 15.5 4Z" /><path d="M6 12h4M14 12h4" /></svg>;
    case "send":
      return <svg {...common}><path d="M12 20V5M6 11l6-6 6 6" /></svg>;
    case "settings":
      return <svg {...common}><circle cx="12" cy="12" r="3" /><path d="M19.4 15a1.7 1.7 0 0 0 .3 1.9l.1.1-2.8 2.8-.1-.1a1.7 1.7 0 0 0-1.9-.3 1.7 1.7 0 0 0-1 1.6v.2h-4V21a1.7 1.7 0 0 0-1-1.6 1.7 1.7 0 0 0-1.9.3l-.1.1L4.2 17l.1-.1a1.7 1.7 0 0 0 .3-1.9A1.7 1.7 0 0 0 3 14H2.8v-4H3a1.7 1.7 0 0 0 1.6-1 1.7 1.7 0 0 0-.3-1.9L4.2 7 7 4.2l.1.1a1.7 1.7 0 0 0 1.9.3A1.7 1.7 0 0 0 10 3V2.8h4V3a1.7 1.7 0 0 0 1 1.6 1.7 1.7 0 0 0 1.9-.3l.1-.1L19.8 7l-.1.1a1.7 1.7 0 0 0-.3 1.9 1.7 1.7 0 0 0 1.6 1h.2v4H21a1.7 1.7 0 0 0-1.6 1Z" /></svg>;
    case "situation":
      return <svg {...common}><circle cx="12" cy="12" r="9" /><path d="M12 3v9h9" /></svg>;
    case "stop":
      return <svg {...common}><rect x="7" y="7" width="10" height="10" rx="2" /></svg>;
  }
}

export default App;
