import { type Dispatch, type FormEvent, type MutableRefObject, type SetStateAction, useEffect, useRef, useState } from "react";
import { isMeetingBlocking, toMessage } from "../../lib/appHelpers";
import { updateConversationTimestamp } from "../../lib/conversationRouting";
import type { AppSnapshot, ConversationMessage, MeetingState, RuntimeEvent, VoiceSettings } from "../../lib/contracts";
import { cancelRun, listMessages, speakText, startTurn, stopTts } from "../../lib/runtime";
import { StreamingSpeechChunker } from "../../lib/streamingSpeech";

export type StreamingSpeechSession = {
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

export function useConversationTurn({
  selectedConversationId,
  voiceSettings,
  meetingState,
  voiceActionRef,
  voiceState,
  pendingVoicePromptsRef,
  activeRunIdRef,
  activeTtsRunIdRef,
  streamingSpeechSessionRef,
  setSnapshot,
  setError,
}: {
  selectedConversationId: string | null;
  voiceSettings: VoiceSettings | null;
  meetingState: MeetingState;
  voiceActionRef: MutableRefObject<boolean>;
  voiceState: "idle" | "recording" | "transcribing";
  pendingVoicePromptsRef: MutableRefObject<string[]>;
  activeRunIdRef: MutableRefObject<string | null>;
  activeTtsRunIdRef: MutableRefObject<string | null>;
  streamingSpeechSessionRef: MutableRefObject<StreamingSpeechSession | null>;
  setSnapshot: Dispatch<SetStateAction<AppSnapshot>>;
  setError: Dispatch<SetStateAction<string | null>>;
}) {
  const [messages, setMessages] = useState<ConversationMessage[]>([]);
  const [composer, setComposer] = useState("");
  const [activeRunId, setActiveRunId] = useState<string | null>(null);
  const [streamingText, setStreamingText] = useState("");
  const [runtimeActivity, setRuntimeActivity] = useState<string[]>([]);
  const [lastPrompt, setLastPrompt] = useState<string | null>(null);
  const [activeTtsRunId, setActiveTtsRunId] = useState<string | null>(null);
  const selectedConversationIdRef = useRef<string | null>(null);
  const messagesRequestRef = useRef(0);
  const speechDisabledRunIdsRef = useRef(new Set<string>());
  const meetingStateRef = useRef<MeetingState>("idle");
  selectedConversationIdRef.current = selectedConversationId;
  meetingStateRef.current = meetingState;

  useEffect(() => {
    if (!selectedConversationId) { setMessages([]); return; }
    setMessages([]);
    setStreamingText("");
    setRuntimeActivity([]);
    void loadMessages(selectedConversationId);
  }, [selectedConversationId]);

  useEffect(() => () => {
    const speechSession = streamingSpeechSessionRef.current;
    if (speechSession) {
      speechSession.cancelled = true;
      speechSession.queue = [];
      speechSession.chunker.reset();
      streamingSpeechSessionRef.current = null;
    }
    const ttsRunId = activeTtsRunIdRef.current;
    if (ttsRunId) void stopTts(ttsRunId);
  }, [activeTtsRunIdRef, streamingSpeechSessionRef]);

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

  return {
    messages,
    composer,
    setComposer,
    activeRunId,
    streamingText,
    runtimeActivity,
    setRuntimeActivity,
    lastPrompt,
    activeTtsRunId,
    handleSubmit,
    submitPrompt,
    stopActiveRun,
    stopSpeech,
  };
}
