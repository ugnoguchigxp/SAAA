import { type Dispatch, type FormEvent, type MutableRefObject, type SetStateAction, useEffect, useRef, useState } from "react";
import { isMeetingBlocking, toMessage } from "../../lib/appHelpers";
import { uiMessage } from "../../i18n/presentation";
import { updateConversationTimestamp, updateEffectiveRoute } from "../../lib/conversationRouting";
import { appendConversationActivity, type ConversationRuntimeActivity } from "../../lib/conversationActivity";
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
import { ConversationIssueCoordinator } from "./conversationIssueCoordinator";
import { FinalResponseSpeechGate, speechRetry, type SpeechRetry } from "./speechPlaybackPolicy";
type RetryAction =
  | { kind: "response"; prompt: string; inputMessageId: string; inputOrigin: InputOrigin }
  | SpeechRetry;
export function useConversationTurn({
  selectedConversationId,
  voiceSettings,
  meetingState,
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
  pendingVoicePromptsRef: MutableRefObject<PendingConversationPrompt[]>;
  conversationSessionRef: MutableRefObject<ConversationSession>;
  suspendVoiceForSpeech: (speechRunId: string) => Promise<boolean>;
  resumeVoiceAfterSpeech: (speechRunId: string) => Promise<void>;
  setSnapshot: Dispatch<SetStateAction<AppSnapshot>>;
  setError: Dispatch<SetStateAction<string | null>>;
}) {
  const [messages, setMessages] = useState<ConversationMessage[]>([]);
  const [composer, setComposer] = useState("");
  const [activeRunId, setActiveRunId] = useState<string | null>(null);
  const [streamingText, setStreamingText] = useState("");
  const [runtimeActivity, setRuntimeActivity] = useState<ConversationRuntimeActivity[]>([]);
  const [lastPrompt, setLastPrompt] = useState<string | null>(null);
  const [retryAction, setRetryAction] = useState<RetryAction | null>(null);
  const [activeTtsRunId, setActiveTtsRunId] = useState<string | null>(null);
  const selectedConversationIdRef = useRef<string | null>(null);
  const messagesRequestRef = useRef(0);
  const meetingStateRef = useRef<MeetingState>("idle");
  const failedRunIdsRef = useRef(new Set<string>());
  const speechStopRequestsRef = useRef(new Set<string>());
  const issueCoordinatorRef = useRef(new ConversationIssueCoordinator());
  const speechGateRef = useRef(new FinalResponseSpeechGate());
  const disposedRef = useRef(false);
  selectedConversationIdRef.current = selectedConversationId;
  meetingStateRef.current = meetingState;
  useEffect(() => {
    disposedRef.current = false;
    return () => {
      disposedRef.current = true;
      issueCoordinatorRef.current.dispose();
      const runId = conversationSessionRef.current.runId;
      const ttsRunId = conversationSessionRef.current.speechRunId;
      if (runId) void cancelRun(runId).catch(() => undefined);
      if (ttsRunId) void stopTts(ttsRunId).catch(() => undefined);
    };
  }, [conversationSessionRef]);
  useEffect(() => {
    if (!selectedConversationId) { setMessages([]); return; }
    setMessages([]);
    setStreamingText("");
    setRuntimeActivity([]);
    void loadMessages(selectedConversationId, issueCoordinatorRef.current.begin());
  }, [selectedConversationId]);
  function publishIssue(scope: number, message: string, retry: RetryAction | null = null) {
    if (disposedRef.current || !issueCoordinatorRef.current.isCurrent(scope)) return;
    setError(message);
    setRetryAction(retry);
  }
  async function loadMessages(conversationId: string, issueScope: number): Promise<ConversationMessage[]> {
    const request = ++messagesRequestRef.current;
    try {
      const nextMessages = await listMessages(conversationId);
      if (!disposedRef.current && request === messagesRequestRef.current && selectedConversationIdRef.current === conversationId) {
        setMessages(nextMessages);
      }
      return nextMessages;
    } catch (cause) {
      if (!disposedRef.current && request === messagesRequestRef.current && selectedConversationIdRef.current === conversationId) {
        publishIssue(issueScope, toMessage(cause));
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
      retryInputMessageId = null,
      inputOrigin = "text",
    } = options;
    if (
      disposedRef.current
      || !selectedConversationId
      || !prompt.trim()
      || conversationSessionRef.current.runId
    ) return;
    const conversationId = selectedConversationId;
    const content = prompt.trim();
    const runId = `run_${crypto.randomUUID()}`;
    const issueScope = issueCoordinatorRef.current.begin();
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
        await stopSpeech(issueScope);
      }
      await startTurn(
        { runId, conversationId, content, workspacePath: null, retryInputMessageId, inputOrigin, presentationMode },
        (event) => handleRuntimeEvent(event, conversationId, issueScope),
      );
    } catch (cause) {
      failedRunIdsRef.current.add(runId);
      publishIssue(issueScope, toMessage(cause));
    } finally {
      if (conversationSessionRef.current.runId === runId) {
        conversationSessionRef.current = transitionConversationSession(
          conversationSessionRef.current,
          { type: "runFinished", runId },
        );
        if (!disposedRef.current) setActiveRunId(null);
      }
      if (!disposedRef.current && selectedConversationIdRef.current === conversationId) {
        setStreamingText("");
        const nextMessages = await loadMessages(conversationId, issueScope);
        if (failedRunIdsRef.current.delete(runId)) {
          const input = [...nextMessages].reverse().find((message) => message.role === "user" && message.content === content);
          if (input && issueCoordinatorRef.current.isCurrent(issueScope)) {
            setRetryAction({ kind: "response", prompt: content, inputMessageId: input.id, inputOrigin });
          }
        }
      }
      const nextVoicePrompt = disposedRef.current ? undefined : pendingVoicePromptsRef.current.shift();
      if (nextVoicePrompt && selectedConversationIdRef.current === conversationId) {
        if (conversationSessionRef.current.speechRunId) await stopSpeech(issueScope);
        await submitPrompt(nextVoicePrompt.content, { inputOrigin: nextVoicePrompt.inputOrigin });
      }
    }
  }
  function handleRuntimeEvent(event: RuntimeEvent, conversationId: string, issueScope: number) {
    if (
      disposedRef.current ||
      selectedConversationIdRef.current !== conversationId ||
      conversationSessionRef.current.runId !== event.runId
    ) return;
    const finalSpeechText = speechGateRef.current.accept(event);
    switch (event.type) {
      case "started":
        setSnapshot((current) => updateEffectiveRoute(current, event.providerId, "active", { reasonCode: "turn-active" }));
        setRuntimeActivity((current) => appendConversationActivity(current, { type: "providerStarted", providerId: event.providerId }));
        break;
      case "providerSelected":
        setSnapshot((current) => updateEffectiveRoute(current, event.providerId, "active", { fallbackUsed: event.fallbackUsed, reasonCode: event.selectionReasonCode === "other" ? "provider-selected-other" : "turn-active" }));
        setRuntimeActivity((current) => appendConversationActivity(current, { type: "providerSelected", providerId: event.runtimeId, fallbackUsed: event.fallbackUsed }));
        break;
      case "delta":
        setStreamingText((current) => current + event.text);
        break;
      case "activity":
        setRuntimeActivity((current) => appendConversationActivity(current, { type: "providerWorking" }));
        break;
      case "providerFailed":
        setSnapshot((current) => updateEffectiveRoute(current, event.providerId, "failed", { reasonCode: "provider-failed" }));
        setRuntimeActivity((current) => appendConversationActivity(current, { type: "providerFailed" }));
        setStreamingText("");
        break;
      case "messageCompleted":
        setRetryAction(null);
        setMessages((current) => [...current.filter((message) => !message.id.startsWith("streaming_")), event.message]);
        setSnapshot((current) => current.effectiveRoute.providerId
          ? updateEffectiveRoute(current, current.effectiveRoute.providerId, "ready", { fallbackUsed: current.effectiveRoute.fallbackUsed, reasonCode: "last-turn-completed" })
          : current);
        setStreamingText("");
        if (finalSpeechText && voiceSettings?.autoSpeak && !isMeetingBlocking(meetingStateRef.current)) {
          void startSpeech(finalSpeechText, conversationId);
        }
        break;
      case "cancelled":
        failedRunIdsRef.current.delete(event.runId);
        setRuntimeActivity((current) => appendConversationActivity(current, { type: "generationCancelled" }));
        break;
      case "failed":
        failedRunIdsRef.current.add(event.runId);
        publishIssue(issueScope, `${event.message} ${event.recovery}`);
        break;
    }
  }
  async function stopActiveRun() {
    const runId = conversationSessionRef.current.runId;
    if (!runId) return;
    const issueScope = issueCoordinatorRef.current.begin();
    try { await cancelRun(runId); } catch (cause) { publishIssue(issueScope, toMessage(cause)); }
  }
  async function startSpeech(text: string, conversationId = selectedConversationId) {
    if (disposedRef.current || !conversationId || !voiceSettings || conversationSessionRef.current.speechRunId || isMeetingBlocking(meetingStateRef.current)) return;
    const issueScope = issueCoordinatorRef.current.begin();
    const runId = `speech_${crypto.randomUUID()}`;
    conversationSessionRef.current = transitionConversationSession(
      conversationSessionRef.current,
      { type: "speechStarted", runId },
    );
    setActiveTtsRunId(runId);
    try {
      setError(null);
      setRetryAction(null);
      const speakable = toSpeakableText(text);
      if (!speakable) return;
      await suspendVoiceForSpeech(runId);
      if (conversationSessionRef.current.speechRunId !== runId) return;
      await speakText({ runId, conversationId, text: speakable });
    } catch (cause) {
      if (!speechStopRequestsRef.current.has(runId)) {
        publishIssue(issueScope, uiMessage("chatSpeechPlaybackFailed"), speechRetry(text, conversationId));
      }
    } finally {
      if (conversationSessionRef.current.speechRunId === runId) {
        conversationSessionRef.current = transitionConversationSession(
          conversationSessionRef.current,
          { type: "speechFinished", runId },
        );
        if (!disposedRef.current) setActiveTtsRunId(null);
      }
      try {
        await resumeVoiceAfterSpeech(runId);
      } catch (cause) {
        publishIssue(issueScope, uiMessage("chatMicrophoneResumeFailed"));
      }
      speechStopRequestsRef.current.delete(runId);
    }
  }
  async function stopSpeech(existingIssueScope?: number) {
    const runId = conversationSessionRef.current.speechRunId;
    if (!runId) return;
    const issueScope = existingIssueScope ?? issueCoordinatorRef.current.begin();
    speechStopRequestsRef.current.add(runId);
    let stopped = false;
    try {
      await stopTts(runId);
      stopped = true;
    } catch (cause) {
      speechStopRequestsRef.current.delete(runId);
      publishIssue(issueScope, toMessage(cause));
    } finally {
      if (stopped && conversationSessionRef.current.speechRunId === runId) {
        conversationSessionRef.current = transitionConversationSession(
          conversationSessionRef.current,
          { type: "speechFinished", runId },
        );
        if (!disposedRef.current) setActiveTtsRunId(null);
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
