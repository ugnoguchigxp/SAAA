import { SetupChecklist } from "./SetupChecklist";
import { useEffect, useRef, useState, type CSSProperties, type DragEvent, type FormEvent } from "react";
import { useLatestMessageScroll } from "./useLatestMessageScroll";
import { useTranslation } from "react-i18next";
import { AppIcon } from "../../components/AppIcon";
import { localizeRuntimeActivity, localizeUiMessage } from "../../i18n/presentation";
import { DEFAULT_VOICE_SILENCE_TIMEOUT_MS } from "../../lib/voiceActivity";
import { conversationActivityOutcome } from "../../lib/conversationActivity";
import { ConversationBehaviorMenu } from "./ConversationBehaviorMenu";
import { VirtualMessages } from "./VirtualMessages";
import { StreamingPlainText } from "./ChatMessages";
import { RoutingProposal } from "./RoutingProposal";
import type { ChatPageProps } from "./chatPageTypes";
import { useArtifactWorkspace } from "./artifacts/ArtifactDrawer";
import {
    armTurnImage,
    droppedImageFile,
    imageDragActive,
    subscribeImageClaimed,
  } from "./composerImage";
import { discardComposerImage, prepareComposerImage } from "../../lib/runtime";
import { useLarmConnectionStatus } from "./useLarmConnectionStatus";
import { invoke } from "@tauri-apps/api/core";

const VOICE_BAR_WEIGHTS = [0.18, 0.32, 0.54, 0.78, 1, 0.7, 0.48, 0.72, 0.46, 0.28, 0.16];
const HANDOFF_PREPARATION_DEFERRED = "回答用の接続を準備中です。少し後に、内容を確認してもう一度お試しください。";

export function ChatPage({
  setupSnapshot,
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
  voiceReady,
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
  const [imageDrag, setImageDrag] = useState(false);
  const larmStatus = useLarmConnectionStatus(
    selectedConversation?.id ?? null,
    Boolean(selectedConversation?.id),
  );
  useEffect(() => {
    const conversationId = selectedConversation?.id;
    if (!conversationId) return;
    return () => {
      void invoke("end_larm_voice_session", {
        ownerId: `conversation-${conversationId}`,
        drain: false,
      }).catch(() => undefined);
    };
  }, [selectedConversation?.id]);
  const [imagePhase, setImagePhase] = useState<"idle" | "processing" | "ready" | "submitting" | "failed">("idle");
  const [imageNote, setImageNote] = useState<string | null>(null);
  const [imagePreview, setImagePreview] = useState<{
    id: string;
    url: string;
    width: number;
    height: number;
    byteLength: number;
  } | null>(null);
  const imagePreviewRef = useRef(imagePreview);
  imagePreviewRef.current = imagePreview;
  const preparationRef = useRef(0);
  async function clearImage() {
    const current = imagePreviewRef.current;
    preparationRef.current += 1;
    if (current) {
      URL.revokeObjectURL(current.url);
      await discardComposerImage(current.id).catch(() => undefined);
    }
    armTurnImage(null);
    setImagePreview(null);
    setImagePhase("idle");
    setImageNote(null);
  }
  useEffect(() => {
    return subscribeImageClaimed(() => {
      const current = imagePreviewRef.current;
      if (!current) return;
      URL.revokeObjectURL(current.url);
      armTurnImage(null);
      setImagePreview(null);
      setImagePhase("idle");
      setImageNote(null);
    });
  }, []);
  useEffect(() => {
    setImagePreview(null);
    setImagePhase("idle");
    setImageNote(null);
    return () => {
      preparationRef.current += 1;
      const current = imagePreviewRef.current;
      armTurnImage(null);
      setImagePreview(null);
      setImagePhase("idle");
      setImageNote(null);
      if (current) {
        URL.revokeObjectURL(current.url);
        void discardComposerImage(current.id);
      }
    };
  }, [selectedConversation?.id]);
  async function acceptImage(file: File) {
    const ticket = preparationRef.current + 1;
    preparationRef.current = ticket;
    const previous = imagePreviewRef.current;
    armTurnImage(null);
    if (previous) {
      URL.revokeObjectURL(previous.url);
      void discardComposerImage(previous.id);
      setImagePreview(null);
    }
    setImagePhase("processing");
    setImageNote(null);
    try {
      if (file.size > 12_000_000) throw new Error(t("chat.imageTooLarge"));
      const prepared = await prepareComposerImage(new Uint8Array(await file.arrayBuffer()));
      if (preparationRef.current !== ticket) {
        await discardComposerImage(prepared.id);
        return;
      }
      const url = URL.createObjectURL(new Blob([new Uint8Array(prepared.preview)], { type: "image/webp" }));
      setImagePreview({
        id: prepared.id,
        url,
        width: prepared.width,
        height: prepared.height,
        byteLength: prepared.byteLength,
      });
      setImagePhase("ready");
    } catch (cause) {
      if (preparationRef.current !== ticket) return;
      setImagePhase("failed");
      setImageNote(cause instanceof Error ? cause.message : t("chat.imageFailed"));
    }
  }
  function onComposerDragOver(event: DragEvent<HTMLFormElement>) {
    if (!imageDragActive([...event.dataTransfer.types])) return;
    event.preventDefault();
    setImageDrag(true);
  }
  const artifacts = useArtifactWorkspace();
  const lastMessage = messages[messages.length - 1];
  const precedingMessage = messages[messages.length - 2];
  const deferredHandoffInput = !activeRunId && lastMessage?.role === "assistant" &&
    lastMessage.content === HANDOFF_PREPARATION_DEFERRED && precedingMessage?.role === "user"
      ? precedingMessage
      : undefined;
  useEffect(() => {
    if (selectedConversation) artifacts?.focusConversation(selectedConversation.id);
  }, [selectedConversation, artifacts]);
  const {
    messageAreaRef,
    messageContentRef,
    followLatestRef,
    showLatestButton,
    setShowLatestButton,
    scrollToLatest,
    updateFollowLatest,
  } = useLatestMessageScroll(selectedConversation?.id, hasNewerMessages);
  const returnToLatestRef = useRef(onReturnToLatest);
  returnToLatestRef.current = onReturnToLatest;
  useEffect(() => {
    // A remounted conversation may still hold a paginated, older history window.
    void returnToLatestRef.current();
  }, [selectedConversation?.id]);

  async function handleMessageAreaScroll() {
    const messageArea = messageAreaRef.current;
    if (!messageArea) return;
    const distanceFromBottom =
      messageArea.scrollHeight - messageArea.scrollTop - messageArea.clientHeight;
    updateFollowLatest();
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
      });
    }
  }

  async function returnToLatest() {
    followLatestRef.current = true;
    setShowLatestButton(false);
    await onReturnToLatest();
    requestAnimationFrame(scrollToLatest);
  }

  function submitFromLatest(event: FormEvent<HTMLFormElement>) {
    followLatestRef.current = true;
    setShowLatestButton(false);
    if (imagePhase === "processing" || imagePhase === "submitting" || (activeRunId && imagePreview)) {
      event.preventDefault();
      if (activeRunId && imagePreview) setImageNote(t("chat.imageWait"));
      return;
    }
    if (imagePreview && imagePhase === "ready") {
      armTurnImage(imagePreview.id);
      setImagePhase("submitting");
      const restore = () => {
        if (imagePreviewRef.current?.id === imagePreview.id) setImagePhase("ready");
      };
      void Promise.resolve(onSubmit(event)).then(restore, restore);
      return;
    }
    onSubmit(event);
  }
  function onComposerDrop(event: DragEvent<HTMLFormElement>) {
    setImageDrag(false);
    if (imagePhase === "submitting") {
      event.preventDefault();
      return;
    }
    const dropped = droppedImageFile(event.dataTransfer.files);
    if (!dropped && event.dataTransfer.files.length === 0) return;
    event.preventDefault();
    if (dropped === "multiple") {
      if (!imagePreview) setImagePhase("failed");
      setImageNote(t("chat.imageMultiple"));
      return;
    }
    if (!dropped) return;
    void acceptImage(dropped);
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
        <div className="message-content" ref={messageContentRef}>
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
            <VirtualMessages messages={messages} scrollRef={messageAreaRef} />
          )}
          {deferredHandoffInput && (
            <button
              type="button"
              className="text-button"
              onClick={() => onComposerChange(deferredHandoffInput.content)}
            >
              認識された依頼を確認して再試行
            </button>
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
            <div role="status">
              <div className="llm-thinking-indicator" aria-label={t("chat.thinking")}>
                <span />
                <span />
                <span />
              </div>
              {runtimeActivity.length > 0 ? (
                <p>{localizeRuntimeActivity(t, runtimeActivity[runtimeActivity.length - 1])}</p>
              ) : null}
            </div>
          ) : null}
          {runtimeActivity.length > 0 && (
            <details className="activity-panel" open={Boolean(conversationActivityOutcome(runtimeActivity))}>
              <summary>
                {conversationActivityOutcome(runtimeActivity) === "cancelled-after-search"
                  ? t("chat.activity.cancelledAfterSearch")
                  : conversationActivityOutcome(runtimeActivity) === "cancelled"
                    ? t("chat.activity.generationCancelled")
                    : conversationActivityOutcome(runtimeActivity) === "failed"
                      ? t("chat.activity.providerFailed")
                      : t("chat.runtimeActivity")}
              </summary>
              {runtimeActivity.map((activity, index) => (
                <p key={`${index}-${activity.type}`}>{localizeRuntimeActivity(t, activity)}</p>
              ))}
            </details>
          )}
        </div>
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
      <form
        className={imageDrag ? "composer image-drag" : "composer"}
        onSubmit={submitFromLatest}
        onDragOver={onComposerDragOver}
        onDragLeave={() => setImageDrag(false)}
        onDrop={onComposerDrop}
      >
        <div className="composer-row">
          <button
            className={voiceReady ? "voice-button recording" : "voice-button"}
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
            className={`voice-activity-indicator${voiceReady ? " listening" : " paused"}${voiceReady && voiceActivityDetected ? " detecting" : ""}`}
            role="img"
            aria-label={t(
              !listeningEnabled
                ? "chat.voiceIndicatorPaused"
                : !voiceReady
                  ? "chat.voiceIndicatorPreparing"
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
            ) : null}
            <button
              className="send-button"
              type="submit"
              aria-label={t("chat.send")}
              disabled={
                imagePhase === "processing" ||
                imagePhase === "submitting" ||
                (Boolean(activeRunId) && imagePhase === "ready") ||
                (!composer.trim() && imagePhase !== "ready") ||
                !selectedConversation
              }
            >
              <AppIcon name="send" />
            </button>
          </div>
        </div>
        <div className="composer-meta" aria-live="polite">
          {larmStatus && <span className="composer-hint" data-larm-connection-status={larmStatus.state}>{larmStatus.message}</span>}
          {imagePhase === "processing" && (
            <span className="composer-hint">{t("chat.imageProcessing")}</span>
          )}
          {imagePreview && (imagePhase === "ready" || imagePhase === "submitting") && (
            <span className="composer-image">
              <img src={imagePreview.url} alt={t("chat.imagePreview")} />
              <span>
                {imagePreview.width}×{imagePreview.height} · {imagePreview.byteLength} B
              </span>
              <button className="text-button" type="button" disabled={imagePhase === "submitting"} onClick={() => void clearImage()}>
                {t("chat.imageRemove")}
              </button>
              <span className="composer-hint">{t("chat.imageNotKept")}</span>
            </span>
          )}
          {imageNote && (
            <span className="composer-hint">{imageNote}</span>
          )}
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
          {listeningEnabled && !voiceReady && !activeTtsRunId && (
            <span className="composer-hint">{t("chat.voicePreparingHint")}</span>
          )}
          {voiceReady && (
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
