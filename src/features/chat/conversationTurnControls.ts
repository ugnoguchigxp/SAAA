import type { Dispatch, MutableRefObject, SetStateAction } from "react";
import { toMessage } from "../../lib/appHelpers";
import { uiMessage } from "../../i18n/presentation";
import { updateEffectiveRoute } from "../../lib/conversationRouting";
import {
  appendConversationActivity,
  type ConversationRuntimeActivity,
} from "../../lib/conversationActivity";
import type { AppSnapshot, ConversationMessage, RuntimeEvent } from "../../lib/contracts";
import { acknowledgeConversationMessage, cancelRun, stopTts } from "../../lib/runtime";
import {
  transitionConversationSession,
  type ConversationSession,
} from "../../lib/conversationSession";
import {
  markReasoningRun,
  markReasoningCancellation,
  clearReasoningCancellation,
  reasoningCancellationRequested,
} from "../../lib/reasoningRun";
import { presentCompletedAnswer } from "./artifacts/answerPresentation";
import { recordRuntimeLifecycleAudit } from "./conversationAudit";
import { ConversationIssueCoordinator } from "./conversationIssueCoordinator";
import {
  requiredContextFailureCode,
  type RequiredContextFailureCode,
  type RequiredContextRecoveryAction,
} from "./requiredContextRecovery";
import {
  recordFirstDelta,
  recordResponseCompleted,
  recordRunWithoutMarkdown,
  recordSocketReceive,
} from "./streamingPerformance";
import type { useConversationVoicePolicy } from "./useConversationVoicePolicy";
import type { useMessageHistory } from "./useMessageHistory";
import type { useStreamingTextProjection } from "./useStreamingTextProjection";

type History = ReturnType<typeof useMessageHistory>;
type VoicePolicy = ReturnType<typeof useConversationVoicePolicy>;
type Streaming = ReturnType<typeof useStreamingTextProjection>;

export function createConversationTurnControls(input: {
  forgottenRuns: MutableRefObject<Set<string>>;
  conversationSessionRef: MutableRefObject<ConversationSession>;
  disposedRef: MutableRefObject<boolean>;
  selectedConversationIdRef: MutableRefObject<string | null>;
  setSnapshot: Dispatch<SetStateAction<AppSnapshot>>;
  setRuntimeActivity: Dispatch<SetStateAction<ConversationRuntimeActivity[]>>;
  appendStreamingText: Streaming["appendStreamingText"];
  resetStreamingText: Streaming["resetStreamingText"];
  history: History;
  setRetryAction: Dispatch<
    SetStateAction<{
      prompt: string;
      inputMessageId: string;
      inputOrigin: import("../../lib/conversationSession").InputOrigin;
      kind: "response";
    } | null>
  >;
  setMessages: History["setMessages"];
  loadMessages: (conversationId: string, issueScope: number) => Promise<ConversationMessage[]>;
  loadMessagesCommitted: (
    conversationId: string,
    issueScope: number,
  ) => Promise<ConversationMessage[]>;
  voice: VoicePolicy;
  incompleteRunIdsRef: MutableRefObject<Set<string>>;
  setActiveTtsRunId: Dispatch<SetStateAction<string | null>>;
  suspendVoiceForSpeech: (speechRunId: string) => Promise<boolean>;
  publishIssue: (scope: number, message: string) => void;
  resumeVoiceAfterSpeech: (speechRunId: string) => Promise<void>;
  speechStopRequestsRef: MutableRefObject<Set<string>>;
  failedRunIdsRef: MutableRefObject<Set<string>>;
  nonRetryableRunIdsRef: MutableRefObject<Set<string>>;
  setRequiredContextFailure: Dispatch<SetStateAction<RequiredContextFailureCode | null>>;
  issueCoordinatorRef: MutableRefObject<ConversationIssueCoordinator>;
  retryAction: {
    prompt: string;
    inputMessageId: string;
    inputOrigin: import("../../lib/conversationSession").InputOrigin;
  } | null;
  submitPrompt: (
    prompt: string,
    options?: import("../../lib/conversationSession").SubmitPromptOptions,
  ) => Promise<void>;
  setComposer: Dispatch<SetStateAction<string>>;
  lastPrompt: string | null;
}) {
  const {
    forgottenRuns,
    conversationSessionRef,
    disposedRef,
    selectedConversationIdRef,
    setSnapshot,
    setRuntimeActivity,
    appendStreamingText,
    resetStreamingText,
    history,
    setRetryAction,
    setMessages,
    loadMessages,
    loadMessagesCommitted,
    voice,
    incompleteRunIdsRef,
    setActiveTtsRunId,
    suspendVoiceForSpeech,
    publishIssue,
    resumeVoiceAfterSpeech,
    speechStopRequestsRef,
    failedRunIdsRef,
    nonRetryableRunIdsRef,
    setRequiredContextFailure,
    issueCoordinatorRef,
    retryAction,
    submitPrompt,
    setComposer,
    lastPrompt,
  } = input;

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
        if (event.kind === "source-available") {
          try {
            const source = JSON.parse(event.summary) as { url?: string; title?: string };
            if (source.url && /^https?:\/\//i.test(source.url)) {
              setRuntimeActivity((current) =>
                appendConversationActivity(current, {
                  type: "sourceAvailable",
                  runId: event.runId,
                  url: source.url!,
                  title: source.title || source.url!,
                }),
              );
            }
          } catch {
            // Ignore malformed source metadata from a runtime event.
          }
          break;
        }
        if (event.kind === "ui-presented") {
          void loadMessages(conversationId, issueScope);
          break;
        }
        setRuntimeActivity((current) =>
          appendConversationActivity(current, {
            type:
              event.kind === "web-search-started"
                ? "webSearching"
                : event.kind === "source-fetch-started"
                  ? "sourceFetching"
                  : event.kind === "source-retrieval-completed"
                    ? "answerPreparing"
                    : "providerWorking",
          }),
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
      case "messageCommitted":
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
        resetStreamingText();
        if (!document.hidden && typeof requestAnimationFrame === "function") {
          requestAnimationFrame(() => requestAnimationFrame(() => {
            const rendered = [...document.querySelectorAll<HTMLElement>("[data-message-id]")]
              .some((element) => element.dataset.messageId === event.message.id);
            if (rendered) {
              void acknowledgeConversationMessage(conversationId, event.runId, event.message.id)
                .catch(() => undefined);
            }
          }));
        }
        break;
      case "runInputAccepted":
        break;
      case "messageCompleted":
        presentCompletedAnswer(event.message);
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
      await cancelRun(runId, "user-stop");
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
    handleRuntimeEvent,
    stopActiveRun,
    stopSpeech,
    retryFailedAction,
    prepareRequiredContextRecovery,
  };
}
