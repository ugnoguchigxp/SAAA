import { type Dispatch, type FormEvent, type MutableRefObject, type SetStateAction, useEffect, useRef, useState } from "react";
import { isMeetingBlocking, toMessage } from "../../lib/appHelpers";
import { updateConversationTimestamp, updateEffectiveRoute } from "../../lib/conversationRouting";
import type { AppSnapshot, ConversationMessage, MeetingState, RuntimeEvent, VoiceSettings } from "../../lib/contracts";
import { cancelRun, listMessages, speakText, startTurn, stopTts } from "../../lib/runtime";
import { toSpeakableText } from "../../lib/speakableText";
import {
  transitionConversationSession,
  type ConversationSession,
  type InputOrigin,
  type PendingConversationPrompt,
  type SubmitPromptOptions,
} from "../../lib/conversationSession";
type RetryAction =
  | { kind: "response"; prompt: string; inputMessageId: string; inputOrigin: InputOrigin }
  | { kind: "speech"; text: string; conversationId: string };
export function useConversationTurn({
  selectedConversationId,
  voiceSettings,
  meetingState,
  isVoiceBusy,
  pendingVoicePromptsRef,
  conversationSessionRef,
  suspendVoiceForSpeech,
  resumeVoiceAfterSpeech,
  setSnapshot,
  setError,
}: {
  selectedConversationId: string | null;
  voiceSettings: VoiceSettings | null;
  meetingState: MeetingState;
  isVoiceBusy: () => boolean;
  pendingVoicePromptsRef: MutableRefObject<PendingConversationPrompt[]>;
  conversationSessionRef: MutableRefObject<ConversationSession>;
  suspendVoiceForSpeech: () => Promise<boolean>;
  resumeVoiceAfterSpeech: () => Promise<void>;
  setSnapshot: Dispatch<SetStateAction<AppSnapshot>>;
  setError: Dispatch<SetStateAction<string | null>>;
}) {
  const [messages, setMessages] = useState<ConversationMessage[]>([]);
  const [composer, setComposer] = useState("");
  const [activeRunId, setActiveRunId] = useState<string | null>(null);
  const [streamingText, setStreamingText] = useState("");
  const [runtimeActivity, setRuntimeActivity] = useState<string[]>([]);
  const [lastPrompt, setLastPrompt] = useState<string | null>(null);
  const [retryAction, setRetryAction] = useState<RetryAction | null>(null);
  const [activeTtsRunId, setActiveTtsRunId] = useState<string | null>(null);
  const selectedConversationIdRef = useRef<string | null>(null);
  const messagesRequestRef = useRef(0);
  const meetingStateRef = useRef<MeetingState>("idle");
  const failedRunIdsRef = useRef(new Set<string>());
  const speechStopRequestsRef = useRef(new Set<string>());
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
    const ttsRunId = conversationSessionRef.current.speechRunId;
    if (ttsRunId) void stopTts(ttsRunId);
  }, [conversationSessionRef]);
  async function loadMessages(conversationId: string): Promise<ConversationMessage[]> {
    const request = ++messagesRequestRef.current;
    try {
      const nextMessages = await listMessages(conversationId);
      if (request === messagesRequestRef.current && selectedConversationIdRef.current === conversationId) {
        setMessages(nextMessages);
      }
      return nextMessages;
    } catch (cause) {
      if (request === messagesRequestRef.current && selectedConversationIdRef.current === conversationId) {
        setError(toMessage(cause));
      }
      return [];
    }
  }
  async function handleSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    await submitPrompt(composer);
  }
  async function submitPrompt(prompt: string, options: SubmitPromptOptions = {}) {
    const {
      allowVoiceBusy = false,
      retryInputMessageId = null,
      inputOrigin = "text",
    } = options;
    if (
      !selectedConversationId
      || !prompt.trim()
      || conversationSessionRef.current.runId
      || (!allowVoiceBusy && isVoiceBusy())
    ) return;
    const conversationId = selectedConversationId;
    const content = prompt.trim();
    const runId = `run_${crypto.randomUUID()}`;
    const shouldStreamSpeech = Boolean(voiceSettings?.autoSpeak)
      && !isMeetingBlocking(meetingStateRef.current);
    const presentationMode = shouldStreamSpeech ? "visual-and-spoken" : "visual";
    try {
      setError(null);
      setRetryAction(null);
      setLastPrompt(content);
      conversationSessionRef.current = transitionConversationSession(
        conversationSessionRef.current,
        { type: "runStarted", runId },
      );
      setActiveRunId(runId);
      setStreamingText("");
      setRuntimeActivity([]);
      if (!retryInputMessageId) {
        setMessages((current) => [...current, { id: `pending_${runId}`, conversationId, role: "user", content, createdAt: String(Date.now()) }]);
      }
      setComposer("");
      setSnapshot((current) => updateConversationTimestamp(current, conversationId, content));
      if (shouldStreamSpeech) {
        await stopSpeech();
      }
      await startTurn(
        { runId, conversationId, content, workspacePath: null, retryInputMessageId, inputOrigin, presentationMode },
        (event) => handleRuntimeEvent(event, conversationId),
      );
    } catch (cause) {
      failedRunIdsRef.current.add(runId);
      setError((current) => current ?? toMessage(cause));
    } finally {
      if (conversationSessionRef.current.runId === runId) {
        conversationSessionRef.current = transitionConversationSession(
          conversationSessionRef.current,
          { type: "runFinished", runId },
        );
        setActiveRunId(null);
      }
      if (selectedConversationIdRef.current === conversationId) {
        setStreamingText("");
        const nextMessages = await loadMessages(conversationId);
        if (failedRunIdsRef.current.delete(runId)) {
          const input = [...nextMessages].reverse().find((message) => message.role === "user" && message.content === content);
          if (input) setRetryAction({ kind: "response", prompt: content, inputMessageId: input.id, inputOrigin });
        }
      }
      const nextVoicePrompt = pendingVoicePromptsRef.current.shift();
      if (nextVoicePrompt && selectedConversationIdRef.current === conversationId) {
        if (conversationSessionRef.current.speechRunId) await stopSpeech();
        await submitPrompt(nextVoicePrompt.content, { allowVoiceBusy: true, inputOrigin: nextVoicePrompt.inputOrigin });
      }
    }
  }
  function handleRuntimeEvent(event: RuntimeEvent, conversationId: string) {
    if (
      selectedConversationIdRef.current !== conversationId ||
      conversationSessionRef.current.runId !== event.runId
    ) return;
    switch (event.type) {
      case "started":
        setSnapshot((current) => updateEffectiveRoute(current, event.providerId, "active", { reasonCode: "turn-active" }));
        setRuntimeActivity((current) => [...current, `${event.route} → ${event.providerId}`].slice(-8));
        break;
      case "providerSelected":
        setSnapshot((current) => updateEffectiveRoute(current, event.providerId, "active", { fallbackUsed: event.fallbackUsed, reasonCode: event.selectionReasonCode === "other" ? "provider-selected-other" : "turn-active" }));
        setRuntimeActivity((current) => [...current, `${event.routeId} → ${event.runtimeId} · ${event.selectionReasonCode}${event.fallbackUsed ? " · fallback" : ""}`].slice(-8));
        break;
      case "delta":
        setStreamingText((current) => current + event.text);
        break;
      case "activity":
        setRuntimeActivity((current) => [...current, `${event.kind}: ${event.summary}`].slice(-8));
        break;
      case "providerFailed":
        setSnapshot((current) => updateEffectiveRoute(current, event.providerId, "failed", { reasonCode: "provider-failed" }));
        setRuntimeActivity((current) => [...current, `${event.providerId} failed: ${event.reason}`].slice(-8));
        setStreamingText("");
        break;
      case "messageCompleted":
        setRetryAction(null);
        setMessages((current) => [...current.filter((message) => !message.id.startsWith("streaming_")), event.message]);
        setSnapshot((current) => current.effectiveRoute.providerId
          ? updateEffectiveRoute(current, current.effectiveRoute.providerId, "ready", { fallbackUsed: current.effectiveRoute.fallbackUsed, reasonCode: "last-turn-completed" })
          : current);
        setStreamingText("");
        if (voiceSettings?.autoSpeak && !isMeetingBlocking(meetingStateRef.current)) {
          void startSpeech(event.message.content, conversationId);
        }
        break;
      case "cancelled":
        failedRunIdsRef.current.delete(event.runId);
        setRuntimeActivity((current) => [...current, "Generation cancelled"].slice(-8));
        break;
      case "failed":
        failedRunIdsRef.current.add(event.runId);
        setError(`${event.message} ${event.recovery}`);
        break;
    }
  }
  async function stopActiveRun() {
    const runId = conversationSessionRef.current.runId;
    if (!runId) return;
    try { await cancelRun(runId); } catch (cause) { setError(toMessage(cause)); }
  }
  async function startSpeech(text: string, conversationId = selectedConversationId) {
    if (!conversationId || !voiceSettings || conversationSessionRef.current.speechRunId || isMeetingBlocking(meetingStateRef.current)) return;
    const runId = `speech_${crypto.randomUUID()}`;
    conversationSessionRef.current = transitionConversationSession(
      conversationSessionRef.current,
      { type: "speechStarted", runId },
    );
    setActiveTtsRunId(runId);
    let voiceSuspended = false;
    try {
      const speakable = toSpeakableText(text);
      if (!speakable) return;
      voiceSuspended = await suspendVoiceForSpeech();
      if (conversationSessionRef.current.speechRunId !== runId) return;
      await speakText({ runId, conversationId, text: speakable, voice: voiceSettings.ttsVoice });
    } catch (cause) {
      if (!speechStopRequestsRef.current.has(runId)) {
        setError(`Speech playback failed: ${toMessage(cause)}`);
        setRetryAction({ kind: "speech", text, conversationId });
      }
    } finally {
      if (conversationSessionRef.current.speechRunId === runId) {
        conversationSessionRef.current = transitionConversationSession(
          conversationSessionRef.current,
          { type: "speechFinished", runId },
        );
        setActiveTtsRunId(null);
      }
      if (voiceSuspended) {
        try {
          await resumeVoiceAfterSpeech();
        } catch (cause) {
          setError(`Microphone resume failed: ${toMessage(cause)}`);
        }
      }
      speechStopRequestsRef.current.delete(runId);
    }
  }
  async function stopSpeech() {
    const runId = conversationSessionRef.current.speechRunId;
    if (!runId) return;
    speechStopRequestsRef.current.add(runId);
    let stopped = false;
    try {
      await stopTts(runId);
      stopped = true;
    } catch (cause) {
      speechStopRequestsRef.current.delete(runId);
      setError(toMessage(cause));
    } finally {
      if (stopped && conversationSessionRef.current.speechRunId === runId) {
        conversationSessionRef.current = transitionConversationSession(
          conversationSessionRef.current,
          { type: "speechFinished", runId },
        );
        setActiveTtsRunId(null);
      }
    }
  }
  async function retryFailedAction() {
    const action = retryAction;
    if (!action) return;
    setRetryAction(null);
    if (action.kind === "speech") {
      await startSpeech(action.text, action.conversationId);
    } else {
      await submitPrompt(action.prompt, { retryInputMessageId: action.inputMessageId, inputOrigin: action.inputOrigin });
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
    retryKind: retryAction?.kind ?? null,
    retryFailedAction,
    activeTtsRunId,
    handleSubmit,
    submitPrompt,
    stopActiveRun,
    stopSpeech,
  };
}
