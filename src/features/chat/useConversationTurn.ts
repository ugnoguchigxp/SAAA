import { useWorldScope } from "./useWorldScope";
import { usePersonalStateForget } from "./usePersonalStateForget";
import {
  requiredContextFailureCode,
  type RequiredContextFailureCode,
  type RequiredContextRecoveryAction,
} from "./requiredContextRecovery";
import { useCommittedCallback } from "../../useCommittedCallback";
import { cancelReasoningRun } from "../../lib/reasoningRunControl";
import {
  markReasoningRun,
  endReasoningRun,
  queueReasoningReplacement,
  markReasoningCancellation,
  clearReasoningCancellation,
  reasoningCancellationRequested,
} from "../../lib/reasoningRun";
import { useMessageHistory } from "./useMessageHistory";
import { useDelegatedReports } from "./useDelegatedReports";
import {
  type Dispatch,
  type FormEvent,
  type MutableRefObject,
  type SetStateAction,
  useEffect,
  useRef,
  useState,
} from "react";
import { toMessage } from "../../lib/appHelpers";
import { uiMessage } from "../../i18n/presentation";
import { updateConversationTimestamp, updateEffectiveRoute } from "../../lib/conversationRouting";
import {
  appendConversationActivity,
  type ConversationRuntimeActivity,
} from "../../lib/conversationActivity";
import type {
  AppSnapshot,
  ConversationMessage,
  RuntimeEvent,
  VoiceSettings,
} from "../../lib/contracts";
import { cancelRun, startTurn, stopTts } from "../../lib/runtime";
import { codingApi } from "../coding/api";
import {
  transitionConversationSession,
  type ConversationSession,
  type InputOrigin,
  type PendingConversationPrompt,
  type SubmitPromptOptions,
} from "../../lib/conversationSession";
import { ConversationIssueCoordinator } from "./conversationIssueCoordinator";
import { recordRuntimeLifecycleAudit } from "./conversationAudit";
import { useConversationVoicePolicy } from "./useConversationVoicePolicy";
import { useStreamingTextProjection } from "./useStreamingTextProjection";
import {
  beginRunPerformance,
  recordFirstDelta,
  recordResponseCompleted,
  recordRunWithoutMarkdown,
  recordSocketReceive,
} from "./streamingPerformance";
type RetryAction = {
  kind: "response";
  prompt: string;
  inputMessageId: string;
  inputOrigin: InputOrigin;
};
export function useConversationTurn({
  selectedConversationId,
  voiceSettings,
  pendingVoicePromptsRef,
  conversationSessionRef,
  suspendVoiceForSpeech,
  resumeVoiceAfterSpeech,
  setSnapshot,
  setError,
}: {
  selectedConversationId: string | null;
  voiceSettings: VoiceSettings | null;
  pendingVoicePromptsRef: MutableRefObject<PendingConversationPrompt[]>;
  conversationSessionRef: MutableRefObject<ConversationSession>;
  suspendVoiceForSpeech: (speechRunId: string) => Promise<boolean>;
  resumeVoiceAfterSpeech: (speechRunId: string) => Promise<void>;
  setSnapshot: Dispatch<SetStateAction<AppSnapshot>>;
  setError: Dispatch<SetStateAction<string | null>>;
}) {
  const history = useMessageHistory();
  const {
    messages,
    setMessages,
    hasMoreMessages,
    hasNewerMessages,
    loadingOlderMessages,
    loadingNewerMessages,
  } = history;
  useDelegatedReports(selectedConversationId, (id) => {
    if (!history.isBrowsingOlder()) void history.latest(id);
  });
  const [composer, setComposer] = useState("");
  const [activeRunId, setActiveRunId] = useState<string | null>(null);
  const worldScope = useWorldScope(selectedConversationId, activeRunId);
  const { streamingText, resetStreamingText, appendStreamingText, hasStreamingText } =
    useStreamingTextProjection();
  const [runtimeActivity, setRuntimeActivity] = useState<ConversationRuntimeActivity[]>([]);
  const [lastPrompt, setLastPrompt] = useState<string | null>(null);
  const [retryAction, setRetryAction] = useState<RetryAction | null>(null);
  const [requiredContextFailure, setRequiredContextFailure] =
    useState<RequiredContextFailureCode | null>(null);
  const [activeTtsRunId, setActiveTtsRunId] = useState<string | null>(null);
  const voice = useConversationVoicePolicy(selectedConversationId, setError);
  const selectedConversationIdRef = useRef<string | null>(null);
  const messagesRequestRef = useRef(0);
  const failedRunIdsRef = useRef(new Set<string>());
  const nonRetryableRunIdsRef = useRef(new Set<string>());
  const incompleteRunIdsRef = useRef(new Set<string>());
  const speechStopRequestsRef = useRef(new Set<string>());
  const issueCoordinatorRef = useRef(new ConversationIssueCoordinator());
  const disposedRef = useRef(false);
  selectedConversationIdRef.current = selectedConversationId;
  useEffect(() => {
    disposedRef.current = false;
    const coordinator = issueCoordinatorRef.current;
    return () => {
      disposedRef.current = true;
      coordinator.dispose();
      const runId = conversationSessionRef.current.runId;
      const ttsRunId = conversationSessionRef.current.speechRunId;
      if (runId) void cancelRun(runId).catch(() => undefined);
      if (ttsRunId) void stopTts(ttsRunId).catch(() => undefined);
    };
  }, [conversationSessionRef]);
  const resetHistory = useCommittedCallback(history.reset);
  const loadMessagesCommitted = useCommittedCallback(loadMessages);
  useEffect(() => {
    resetHistory(selectedConversationId);
    incompleteRunIdsRef.current.clear();
    resetStreamingText();
    setRuntimeActivity([]);
    setRequiredContextFailure(null);
    if (selectedConversationId) {
      void loadMessagesCommitted(selectedConversationId, issueCoordinatorRef.current.begin());
    }
  }, [selectedConversationId, resetHistory, resetStreamingText, loadMessagesCommitted]);
  const forgottenRuns = usePersonalStateForget(() => {
    const conversation = selectedConversationIdRef.current;
    resetHistory(conversation);
    resetStreamingText();
    setRuntimeActivity([]);
    if (conversation) void loadMessagesCommitted(conversation, issueCoordinatorRef.current.begin());
  });
  function publishIssue(scope: number, message: string, retry: RetryAction | null = null) {
    if (disposedRef.current || !issueCoordinatorRef.current.isCurrent(scope)) return;
    setError(message);
    setRetryAction(retry);
  }
  async function loadMessages(
    conversationId: string,
    issueScope: number,
  ): Promise<ConversationMessage[]> {
    const request = ++messagesRequestRef.current;
    try {
      const nextMessages = await history.latest(conversationId);
      return nextMessages;
    } catch (cause) {
      if (
        !disposedRef.current &&
        request === messagesRequestRef.current &&
        selectedConversationIdRef.current === conversationId
      ) {
        publishIssue(issueScope, toMessage(cause));
      }
      return [];
    }
  }
  async function loadOlderMessages(): Promise<void> {
    if (!hasMoreMessages) return;
    try {
      await history.load("before");
    } catch (cause) {
      publishIssue(issueCoordinatorRef.current.begin(), toMessage(cause));
    }
  }
  async function loadNewerMessages(): Promise<void> {
    if (!hasNewerMessages) return;
    try {
      await history.load("after");
    } catch (cause) {
      publishIssue(issueCoordinatorRef.current.begin(), toMessage(cause));
    }
  }
  async function returnToLatestMessages(): Promise<void> {
    try {
      await history.returnLatest();
    } catch (cause) {
      publishIssue(issueCoordinatorRef.current.begin(), toMessage(cause));
    }
  }
  useEffect(() => {
    const changed = (event: Event) => {
      if (
        (event as CustomEvent<string>).detail === selectedConversationIdRef.current &&
        selectedConversationIdRef.current
      ) {
        void loadMessagesCommitted(
          selectedConversationIdRef.current,
          issueCoordinatorRef.current.begin(),
        );
      }
    };
    window.addEventListener("saaa:ui-history", changed);
    return () => window.removeEventListener("saaa:ui-history", changed);
  }, [loadMessagesCommitted]);
  async function handleSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    await submitPrompt(composer);
  }
  async function submitPrompt(prompt: string, options: SubmitPromptOptions = {}) {
    const {
      retryInputMessageId = null,
      inputOrigin = "text",
      sourceId = null,
      onSettled,
    } = options;
    const lfmReasoningRequest =
      sourceId?.startsWith("lfm_reasoning_") || sourceId?.startsWith("lfm_handoff_") || false;
    if (lfmReasoningRequest && conversationSessionRef.current.runId) {
      // A separate reasoning request is queued, without interrupting the existing answer.
      pendingVoicePromptsRef.current.push({
        content: prompt,
        inputOrigin,
        sourceId: sourceId!,
        onSettled,
      });
      return;
    }
    const replacement =
      !disposedRef.current && selectedConversationId
        ? queueReasoningReplacement(
            conversationSessionRef.current.runId,
            prompt,
            pendingVoicePromptsRef.current,
            options,
            selectedConversationId,
          )
        : null;
    if (replacement) {
      if (replacement === "queued") {
        setComposer("");
        await cancelReasoningRun(conversationSessionRef.current.runId).catch((cause) =>
          publishIssue(issueCoordinatorRef.current.begin(), toMessage(cause)),
        );
      }
      return;
    }
    if (
      disposedRef.current ||
      !selectedConversationId ||
      !prompt.trim() ||
      conversationSessionRef.current.runId
    )
      return;
    const conversationId = selectedConversationId;
    const content = prompt.trim();
    const runId = `run_${crypto.randomUUID()}`;
    beginRunPerformance(runId);
    const issueScope = issueCoordinatorRef.current.begin();
    const shouldStreamSpeech = Boolean(voiceSettings?.autoSpeak);
    const presentationMode = shouldStreamSpeech ? "visual-and-spoken" : "visual";
    let delivered = false;
    let deliverySettled = false;
    let handedToRetry = false;
    const settleDelivery = (accepted: boolean) => {
      if (deliverySettled) return;
      deliverySettled = true;
      onSettled?.(accepted);
    };
    try {
      setError(null);
      setRetryAction(null);
      setRequiredContextFailure(null);
      setLastPrompt(content);
      conversationSessionRef.current = transitionConversationSession(
        conversationSessionRef.current,
        { type: "runStarted", runId },
      );
      setActiveRunId(runId);
      incompleteRunIdsRef.current.clear();
      resetStreamingText();
      setRuntimeActivity([]);
      if (!lfmReasoningRequest && !retryInputMessageId && !history.isBrowsingOlder()) {
        setMessages((current) => [
          ...current,
          {
            id: `pending_${runId}`,
            conversationId,
            role: "user",
            content,
            createdAt: String(Date.now()),
          },
        ]);
      }
      setComposer("");
      setSnapshot((current) => updateConversationTimestamp(current, conversationId, content));
      if (shouldStreamSpeech && !lfmReasoningRequest) {
        await stopSpeech(issueScope);
      }
      // A registered coding workspace is a user-selected Project. Carry that selection over the
      // normal chat boundary instead of relying on prompt text (or a backend title match) to
      // decide which work is in scope. A missing/temporarily unavailable snapshot deliberately
      // falls back to user scope; it never invents a project or task reference.
      const scopeRefs =
        (await worldScope.resolve(conversationId)) ??
        (await codingApi
          .snapshot(conversationId)
          .then((coding) => {
            const workspaceId = coding.workspace?.workspaceId;
            if (!workspaceId) return undefined;
            const active = coding.jobs.find((job) =>
              ["queued", "running", "cancel_requested"].includes(job.state),
            );
            return [
              {
                kind: "project" as const,
                id: workspaceId,
                relation: "focus" as const,
              },
              {
                kind: "resource" as const,
                id: workspaceId,
                relation: "parent" as const,
              },
              ...(active
                ? [
                    {
                      kind: "task" as const,
                      id: active.jobId,
                      relation: "current" as const,
                    },
                  ]
                : []),
            ];
          })
          .catch(() => undefined));
      await startTurn(
        {
          runId,
          conversationId,
          content,
          workspacePath: null,
          retryInputMessageId,
          scopeRefs,
          sourceId,
          inputOrigin,
          presentationMode,
        },
        (event) => {
          if (event.type === "started") settleDelivery(true);
          handleRuntimeEvent(event, conversationId, issueScope);
        },
      );
      delivered = true;
    } catch (cause) {
      if (!reasoningCancellationRequested(runId)) {
        failedRunIdsRef.current.add(runId);
        publishIssue(issueScope, toMessage(cause));
      }
    } finally {
      endReasoningRun(runId);
      if (conversationSessionRef.current.runId === runId) {
        conversationSessionRef.current = transitionConversationSession(
          conversationSessionRef.current,
          { type: "runFinished", runId },
        );
        if (!disposedRef.current) setActiveRunId(null);
      }
      if (!disposedRef.current && selectedConversationIdRef.current === conversationId) {
        const terminalIncomplete = incompleteRunIdsRef.current.delete(runId);
        const failed = failedRunIdsRef.current.delete(runId);
        const preserveIncomplete = hasStreamingText() && (terminalIncomplete || failed);
        if (!preserveIncomplete) resetStreamingText();
        const nextMessages = await loadMessages(conversationId, issueScope);
        const dispatchRefused = nonRetryableRunIdsRef.current.delete(runId);
        if (failed && !dispatchRefused) {
          const input = [...nextMessages]
            .reverse()
            .find((message) => message.role === "user" && message.content === content);
          if (input && issueCoordinatorRef.current.isCurrent(issueScope)) {
            setRetryAction({
              kind: "response",
              prompt: content,
              inputMessageId: input.id,
              inputOrigin,
            });
            handedToRetry = true;
          }
        }
      }
      settleDelivery(delivered || handedToRetry);
      const nextVoicePrompt = disposedRef.current
        ? undefined
        : pendingVoicePromptsRef.current.shift();
      if (nextVoicePrompt && selectedConversationIdRef.current === conversationId) {
        if (
          conversationSessionRef.current.speechRunId &&
          !(
            nextVoicePrompt.sourceId?.startsWith("lfm_reasoning_") ||
            nextVoicePrompt.sourceId?.startsWith("lfm_handoff_")
          )
        )
          await stopSpeech(issueScope);
        await submitPrompt(nextVoicePrompt.content, {
          inputOrigin: nextVoicePrompt.inputOrigin,
          sourceId: nextVoicePrompt.sourceId,
          onSettled: nextVoicePrompt.onSettled,
        });
      }
    }
  }
  function handleRuntimeEvent(event: RuntimeEvent, conversationId: string, issueScope: number) {
    if (forgottenRuns.current.has(event.runId)) return;
    const isSpeechLifecycle =
      event.type === "speechStarted" ||
      event.type === "speechEnded" ||
      event.type === "speechFailed";
    const ownsEvent =
      conversationSessionRef.current.runId === event.runId ||
      (isSpeechLifecycle && conversationSessionRef.current.speechRunId === event.runId);
    if (disposedRef.current || selectedConversationIdRef.current !== conversationId || !ownsEvent)
      return;
    if (event.type !== "delta") recordRuntimeLifecycleAudit(event, conversationId);
    if (!isSpeechLifecycle) recordSocketReceive(event.runId);
    switch (event.type) {
      case "started":
        if (event.route === "conversation.reasoning") markReasoningRun(event.runId, conversationId);
        setSnapshot((current) =>
          updateEffectiveRoute(current, event.providerId, "active", {
            reasonCode: "turn-active",
          }),
        );
        setRuntimeActivity((current) =>
          appendConversationActivity(current, {
            type: "providerStarted",
            providerId: event.providerId,
          }),
        );
        break;
      case "delta":
        recordFirstDelta(event.runId);
        appendStreamingText(event.runId, event.text);
        break;
      case "activity":
        if (event.kind === "ui-presented") {
          void loadMessages(conversationId, issueScope);
          break;
        }
        setRuntimeActivity((current) =>
          appendConversationActivity(current, { type: "providerWorking" }),
        );
        break;
      case "providerFailed":
        setSnapshot((current) =>
          updateEffectiveRoute(current, event.providerId, "failed", {
            reasonCode: "provider-failed",
          }),
        );
        setRuntimeActivity((current) =>
          appendConversationActivity(current, { type: "providerFailed" }),
        );
        break;
      case "messageCompleted":
        incompleteRunIdsRef.current.delete(event.runId);
        recordResponseCompleted(event.runId, event.message.id);
        setRetryAction(null);
        setMessages((current) =>
          history.isBrowsingOlder()
            ? current
            : [
                ...current.filter(
                  (message) =>
                    !message.id.startsWith("streaming_") && message.id !== event.message.id,
                ),
                event.message,
              ],
        );
        setSnapshot((current) =>
          current.effectiveRoute.providerId
            ? updateEffectiveRoute(current, current.effectiveRoute.providerId, "ready", {
                fallbackUsed: current.effectiveRoute.fallbackUsed,
                reasonCode: "last-turn-completed",
              })
            : current,
        );
        resetStreamingText();
        if (selectedConversationIdRef.current)
          void loadMessagesCommitted(selectedConversationIdRef.current, issueScope);
        if (event.voicePolicy) voice.setVoicePolicy(event.voicePolicy);
        else voice.clearVoicePolicy();
        break;
      case "speechStarted":
        conversationSessionRef.current = transitionConversationSession(
          conversationSessionRef.current,
          { type: "speechStarted", runId: event.runId },
        );
        setActiveTtsRunId(event.runId);
        void suspendVoiceForSpeech(event.runId).catch(() => {
          publishIssue(issueScope, uiMessage("chatSpeechPlaybackFailed"));
        });
        break;
      case "speechEnded":
        if (conversationSessionRef.current.speechRunId === event.runId) {
          conversationSessionRef.current = transitionConversationSession(
            conversationSessionRef.current,
            { type: "speechFinished", runId: event.runId },
          );
          setActiveTtsRunId(null);
          void resumeVoiceAfterSpeech(event.runId).catch(() => {
            publishIssue(issueScope, uiMessage("chatMicrophoneResumeFailed"));
          });
        }
        break;
      case "speechFailed":
        if (!speechStopRequestsRef.current.has(event.runId)) {
          publishIssue(issueScope, uiMessage("chatSpeechPlaybackFailed"));
        }
        break;
      case "cancelled":
        markReasoningCancellation(event.runId);
        recordRunWithoutMarkdown(event.runId, "cancelled");
        failedRunIdsRef.current.delete(event.runId);
        incompleteRunIdsRef.current.add(event.runId);
        setRuntimeActivity((current) =>
          appendConversationActivity(current, { type: "generationCancelled" }),
        );
        break;
      case "failed":
        if (reasoningCancellationRequested(event.runId)) break;
        recordRunWithoutMarkdown(event.runId, "failed");
        failedRunIdsRef.current.add(event.runId);
        incompleteRunIdsRef.current.add(event.runId);
        const contextFailure = requiredContextFailureCode(event.code);
        if (contextFailure) {
          nonRetryableRunIdsRef.current.add(event.runId);
          setRequiredContextFailure(contextFailure);
        }
        publishIssue(issueScope, `${event.message} ${event.recovery}`);
        break;
    }
  }
  async function stopActiveRun() {
    const runId = conversationSessionRef.current.runId;
    if (!runId) return;
    const issueScope = issueCoordinatorRef.current.begin();
    markReasoningCancellation(runId);
    try {
      await cancelRun(runId);
    } catch (cause) {
      clearReasoningCancellation(runId);
      publishIssue(issueScope, toMessage(cause));
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
        try {
          await resumeVoiceAfterSpeech(runId);
        } catch (cause) {
          publishIssue(issueScope, uiMessage("chatMicrophoneResumeFailed"));
        }
      }
    }
  }
  async function retryFailedAction() {
    const action = retryAction;
    if (!action) return;
    setRetryAction(null);
    await submitPrompt(action.prompt, {
      retryInputMessageId: action.inputMessageId,
      inputOrigin: action.inputOrigin,
    });
  }
  function prepareRequiredContextRecovery(action: RequiredContextRecoveryAction) {
    setRequiredContextFailure(null);
    setRetryAction(null);
    if (action === "narrow") setComposer(lastPrompt ?? "");
    else setComposer("");
  }
  return {
    worldScope,
    messages,
    hasMoreMessages,
    hasNewerMessages,
    loadingNewerMessages,
    loadNewerMessages,
    returnToLatestMessages,
    loadingOlderMessages,
    loadOlderMessages,
    composer,
    setComposer,
    activeRunId,
    streamingText,
    runtimeActivity,
    setRuntimeActivity,
    lastPrompt,
    retryKind: retryAction?.kind ?? null,
    retryFailedAction,
    requiredContextFailure,
    prepareRequiredContextRecovery,
    activeTtsRunId,
    handleSubmit,
    submitPrompt,
    stopActiveRun,
    stopSpeech,
    voicePolicy: voice.voicePolicy,
    voicePolicyUpdating: voice.voicePolicyUpdating,
    setConversationSpeechOutput: voice.setConversationSpeechOutput,
    setConversationListeningPace: voice.setConversationListeningPace,
    resetConversationVoiceOverrides: voice.resetConversationVoiceOverrides,
  };
}
