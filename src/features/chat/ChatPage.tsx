import { SetupChecklist } from "./SetupChecklist";
import {
  useCallback,
  useEffect,
  useRef,
  useState,
  type CSSProperties,
  type FormEvent,
} from "react";
import { useTranslation } from "react-i18next";
import { AppIcon } from "../../components/AppIcon";
import { localizeRuntimeActivity, localizeUiMessage } from "../../i18n/presentation";
import { DEFAULT_VOICE_SILENCE_TIMEOUT_MS } from "../../lib/voiceActivity";
import { ConversationBehaviorMenu } from "./ConversationBehaviorMenu";
import { VirtualMessages } from "./VirtualMessages";
import { StreamingPlainText } from "./ChatMessages";
import { RoutingProposal } from "./RoutingProposal";
import type { ChatPageProps } from "./chatPageTypes";

const LATEST_THRESHOLD_PX = 24;
const VOICE_BAR_WEIGHTS = [0.18, 0.32, 0.54, 0.78, 1, 0.7, 0.48, 0.72, 0.46, 0.28, 0.16];
const scrollMemory = new Map<string, { scrollTop: number; followLatest: boolean }>();

export function ChatPage({
  setupSnapshot,
  worldScope,
  messages,
  hasMoreMessages,
  loadingOlderMessages,
  onLoadOlderMessages,
  hasNewerMessages = false,
  loadingNewerMessages = false,
  onLoadNewerMessages,
  onReturnToLatest,
  streamingText,
  voiceState,
  voiceActivityLevel,
  voiceActivityDetected,
  listeningEnabled,
  runtimeActivity,
  composer,
  onComposerChange,
  onSubmit,
  onToggleVoice,
  activeRunId,
  modelProviderStatus,
  onOpenSettings,
  onStopRun,
  onStopSpeech,
  onRetry,
  selectedConversation,
  activeTtsRunId,
  error,
  retryKind,
  requiredContextFailure,
  onPrepareRequiredContextRecovery,
  voicePolicy,
  voicePolicyUpdating,
  onSetConversationSpeechOutput,
  onSetConversationListeningPace,
  onResetConversationVoiceOverrides,
  routingSnapshot,
  routingEvents,
  routingCancellingRootId,
  routingDecidingProposalId,
  routingProposalError,
  onCancelRouting,
  onDecideRoutingProposal,
}: ChatPageProps) {
  const { t } = useTranslation();
  const messageAreaRef = useRef<HTMLDivElement>(null);
  const followLatestRef = useRef(true);
  const [showLatestButton, setShowLatestButton] = useState(false);

  const rememberScroll = useCallback(
    (messageArea: HTMLDivElement, followLatest: boolean) => {
      const conversationId = selectedConversation?.id;
      if (!conversationId) return;
      scrollMemory.set(conversationId, { scrollTop: messageArea.scrollTop, followLatest });
      if (scrollMemory.size > 8) scrollMemory.delete(scrollMemory.keys().next().value!);
    },
    [selectedConversation?.id],
  );

  useEffect(() => {
    const conversationId = selectedConversation?.id;
    const memory = conversationId ? scrollMemory.get(conversationId) : undefined;
    const shouldFollow = memory?.followLatest ?? true;
    followLatestRef.current = shouldFollow;
    setShowLatestButton(!shouldFollow);
    const frame = requestAnimationFrame(() => {
      const messageArea = messageAreaRef.current;
      if (!messageArea) return;
      messageArea.scrollTop = shouldFollow ? messageArea.scrollHeight : (memory?.scrollTop ?? 0);
    });
    return () => cancelAnimationFrame(frame);
  }, [selectedConversation?.id]);

  useEffect(() => {
    const messageArea = messageAreaRef.current;
    if (!messageArea || !followLatestRef.current || hasNewerMessages) return;
    const frame = requestAnimationFrame(() => {
      messageArea.scrollTop = messageArea.scrollHeight;
      setShowLatestButton(false);
      rememberScroll(messageArea, true);
    });
    return () => cancelAnimationFrame(frame);
  }, [messages, streamingText, runtimeActivity, activeRunId, hasNewerMessages, rememberScroll]);

  async function handleMessageAreaScroll() {
    const messageArea = messageAreaRef.current;
    if (!messageArea) return;
    const distanceFromBottom =
      messageArea.scrollHeight - messageArea.scrollTop - messageArea.clientHeight;
    const atLatest = distanceFromBottom < LATEST_THRESHOLD_PX && !hasNewerMessages;
    followLatestRef.current = atLatest;
    setShowLatestButton(!atLatest);
    rememberScroll(messageArea, atLatest);
    if (distanceFromBottom < 100 && hasNewerMessages && !loadingNewerMessages) {
      await onLoadNewerMessages?.();
      return;
    }
    if (messageArea.scrollTop < 100 && hasMoreMessages && !loadingOlderMessages) {
      const previousHeight = messageArea.scrollHeight;
      const previousTop = messageArea.scrollTop;
      await onLoadOlderMessages();
      requestAnimationFrame(() => {
        const current = messageAreaRef.current;
        if (!current || followLatestRef.current) return;
        current.scrollTop = previousTop + current.scrollHeight - previousHeight;
        rememberScroll(current, false);
      });
    }
  }

  async function returnToLatest() {
    followLatestRef.current = true;
    setShowLatestButton(false);
    await onReturnToLatest();
    requestAnimationFrame(() => {
      const messageArea = messageAreaRef.current;
      if (!messageArea) return;
      messageArea.scrollTop = messageArea.scrollHeight;
      rememberScroll(messageArea, true);
    });
  }

  function submitFromLatest(event: FormEvent<HTMLFormElement>) {
    followLatestRef.current = true;
    setShowLatestButton(false);
    onSubmit(event);
  }
  return (
    <section className="chat-panel">
      <RoutingProposal
        snapshot={routingSnapshot}
        events={routingEvents}
        cancellingRootId={routingCancellingRootId}
        decidingProposalId={routingDecidingProposalId}
        proposalError={routingProposalError}
        onCancel={onCancelRouting}
        onDecideProposal={onDecideRoutingProposal}
      />
      <div
        className="message-area"
        ref={messageAreaRef}
        onScroll={() => void handleMessageAreaScroll()}
      >
        {loadingOlderMessages && <p className="history-loading">{t("chat.loadingHistory")}</p>}
        {messages.length === 0 && streamingText.length === 0 ? (
          <div className="empty-state">
            <h2>{t("chat.emptyTitle")}</h2>
            {setupSnapshot && (
              <SetupChecklist snapshot={setupSnapshot} onOpenSettings={onOpenSettings} />
            )}
            <p>{t("chat.emptyDescription")}</p>
            <div className="suggestion-list">
              <button
                type="button"
                onClick={() => onComposerChange(t("chat.suggestionOrganizePrompt"))}
              >
                {t("chat.suggestionOrganize")}
              </button>
              <button
                type="button"
                onClick={() => onComposerChange(t("chat.suggestionUnfinishedPrompt"))}
              >
                {t("chat.suggestionUnfinished")}
              </button>
              <button
                type="button"
                onClick={() => onComposerChange(t("chat.suggestionDelegatePrompt"))}
              >
                {t("chat.suggestionDelegate")}
              </button>
            </div>
          </div>
        ) : (
          <VirtualMessages
            messages={messages}
            scrollRef={messageAreaRef}
            messageScopes={worldScope?.status?.messageScopes}
          />
        )}
        {streamingText.length > 0 && (
          <article className={`message assistant ${!activeRunId ? "incomplete" : "streaming"}`}>
            <span className="message-role">
              {t("chat.assistant")} · {t(!activeRunId ? "chat.incomplete" : "chat.streaming")}
            </span>
            <StreamingPlainText projection={streamingText} />
          </article>
        )}
        {activeRunId ? (
          <div className="llm-thinking-indicator" role="status" aria-label={t("chat.thinking")}>
            <span />
            <span />
            <span />
          </div>
        ) : null}
        {runtimeActivity.length > 0 && (
          <details className="activity-panel">
            <summary>{t("chat.runtimeActivity")}</summary>
            {runtimeActivity.map((activity, index) => (
              <p key={`${index}-${activity.type}`}>{localizeRuntimeActivity(t, activity)}</p>
            ))}
          </details>
        )}
      </div>
      {(showLatestButton || hasNewerMessages) && messages.length > 0 ? (
        <button
          type="button"
          className="latest-message-button"
          aria-label={t("chat.returnToLatest")}
          title={t("chat.returnToLatest")}
          disabled={loadingNewerMessages}
          onClick={() => void returnToLatest()}
        >
          <AppIcon name="down" />
        </button>
      ) : null}
      <form className="composer" onSubmit={submitFromLatest}>
        <div className="composer-row">
          <button
            className={voiceState === "listening" ? "voice-button recording" : "voice-button"}
            type="button"
            aria-pressed={listeningEnabled}
            aria-label={
              voiceState === "preparing"
                ? t("chat.micCancel")
                : voiceState === "stopped"
                  ? t("chat.micResume")
                  : voiceState === "listening" || activeTtsRunId
                    ? t("chat.micPause")
                    : t("chat.micRetry")
            }
            title={
              voiceState === "preparing"
                ? t("chat.micCancel")
                : voiceState === "stopped"
                  ? t("chat.micResume")
                  : voiceState === "listening" || activeTtsRunId
                    ? t("chat.micPause")
                    : t("chat.micRetry")
            }
            onClick={onToggleVoice}
          >
            <AppIcon name={voiceState !== "stopped" || Boolean(activeTtsRunId) ? "stop" : "mic"} />
          </button>
          <div
            className={`voice-activity-indicator${listeningEnabled ? " listening" : " paused"}${voiceActivityDetected ? " detecting" : ""}`}
            role="img"
            aria-label={t(
              !listeningEnabled
                ? "chat.voiceIndicatorPaused"
                : voiceActivityDetected
                  ? "chat.voiceIndicatorActive"
                  : "chat.voiceIndicatorIdle",
            )}
          >
            {VOICE_BAR_WEIGHTS.map((weight, index) => (
              <span
                key={index}
                style={
                  {
                    transform: `scaleY(${0.62 + voiceActivityLevel * (0.75 + weight * 1.35)})`,
                  } as CSSProperties
                }
              />
            ))}
          </div>
          <textarea
            rows={1}
            aria-label={t("chat.messageLabel")}
            onChange={(event) => onComposerChange(event.currentTarget.value)}
            onKeyDown={(event) => {
              if ((event.metaKey || event.ctrlKey) && event.key === "Enter")
                event.currentTarget.form?.requestSubmit();
            }}
            placeholder={t("chat.placeholder")}
            value={composer}
            disabled={Boolean(activeRunId)}
          />
          <div className="composer-end">
            {voicePolicy && (
              <ConversationBehaviorMenu
                conversationId={selectedConversation?.id}
                policy={voicePolicy}
                disabled={voicePolicyUpdating}
                onOpenSettings={onOpenSettings}
                onSetSpeechOutput={onSetConversationSpeechOutput}
                onSetListeningPace={onSetConversationListeningPace}
                onReset={onResetConversationVoiceOverrides}
              />
            )}
            {activeRunId ? (
              <button className="stop-button composer-stop" type="button" onClick={onStopRun}>
                <AppIcon name="stop" />
                <span>{t("chat.stop")}</span>
              </button>
            ) : (
              <button
                className="send-button"
                type="submit"
                aria-label={t("chat.send")}
                disabled={!composer.trim() || !selectedConversation}
              >
                <AppIcon name="send" />
              </button>
            )}
          </div>
        </div>
        <div className="composer-meta" aria-live="polite">
          {!modelProviderStatus.ready && (
            <button
              className="text-button provider-recovery"
              type="button"
              onClick={onOpenSettings}
            >
              {modelProviderStatus.state === "failed"
                ? t("chat.providerUnavailable")
                : t("chat.processingNotSelected")}
            </button>
          )}
          {voiceState === "listening" && (
            <span className="composer-hint">
              {t("chat.listeningHint", {
                seconds:
                  (voicePolicy?.effectiveSilenceTimeoutMs ?? DEFAULT_VOICE_SILENCE_TIMEOUT_MS) /
                  1_000,
              })}
            </span>
          )}
          {!listeningEnabled && <span className="composer-hint">{t("chat.pausedHint")}</span>}
          {activeTtsRunId && listeningEnabled && (
            <span className="composer-hint">{t("chat.speakingHint")}</span>
          )}
          {activeTtsRunId && (
            <button className="text-button" type="button" onClick={onStopSpeech}>
              {t("chat.stopSpeech")}
            </button>
          )}
          {error && retryKind && !activeRunId && (
            <button className="text-button" type="button" onClick={onRetry}>
              {retryKind === "speech" ? t("chat.retrySpeech") : t("chat.retryResponse")}
            </button>
          )}
          {requiredContextFailure && !activeRunId && (
            <div
              className="context-recovery"
              role="group"
              aria-label={t("chat.contextRecovery.label")}
            >
              <span className="composer-hint">
                {t(`chat.contextRecovery.${requiredContextFailure}`)}
              </span>
              {requiredContextFailure !== "required-context-unavailable" && (
                <button
                  className="text-button"
                  type="button"
                  onClick={() => onPrepareRequiredContextRecovery("narrow")}
                >
                  {t("chat.contextRecovery.narrow")}
                </button>
              )}
              <button className="text-button" type="button" onClick={onOpenSettings}>
                {t("chat.contextRecovery.review")}
              </button>
              <button
                className="text-button"
                type="button"
                onClick={() => onPrepareRequiredContextRecovery("correct")}
              >
                {t("chat.contextRecovery.correct")}
              </button>
            </div>
          )}
        </div>
      </form>
      {error && (
        <p className="error-banner" role="alert">
          {localizeUiMessage(t, error, "chat")}
        </p>
      )}
    </section>
  );
}
