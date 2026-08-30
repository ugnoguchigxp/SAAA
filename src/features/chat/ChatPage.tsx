import { type FormEvent } from "react";
import { useTranslation } from "react-i18next";
import { AppIcon } from "../../components/AppIcon";
import type { Conversation, ConversationMessage } from "../../lib/contracts";
import { localizeProviderLabel, localizeRuntimeActivity, localizeStatus, localizeUiMessage } from "../../i18n/presentation";
import { DEFAULT_VOICE_SILENCE_TIMEOUT_MS } from "../../lib/voiceActivity";
import type { VoiceCaptureState } from "../voice/useAmbientVoiceSession";
import type { ConversationRuntimeActivity } from "../../lib/conversationActivity";

export function ChatPage({
  messages,
  streamingText,
  interimTranscript,
  voiceState,
  listeningEnabled,
  runtimeActivity,
  composer,
  onComposerChange,
  onSubmit,
  onToggleVoice,
  voiceStarting,
  meetingActive,
  activeRunId,
  modelProviderStatus,
  onOpenSettings,
  onOpenMeeting,
  onOpenSituation,
  onStopRun,
  onStopSpeech,
  onRetry,
  selectedConversation,
  activeTtsRunId,
  error,
  retryKind,
}: {
  messages: ConversationMessage[];
  streamingText: string;
  interimTranscript: string;
  voiceState: VoiceCaptureState;
  listeningEnabled: boolean;
  runtimeActivity: ConversationRuntimeActivity[];
  composer: string;
  onComposerChange: (value: string) => void;
  onSubmit: (event: FormEvent<HTMLFormElement>) => void;
  onToggleVoice: () => void;
  voiceStarting: boolean;
  meetingActive: boolean;
  activeRunId: string | null;
  modelProviderStatus: { ready: boolean; label: string; location: "local" | "cloud" | null; state: "unchecked" | "active" | "ready" | "failed"; fallbackUsed: boolean };
  onOpenSettings: () => void;
  onOpenMeeting: () => void;
  onOpenSituation: () => void;
  onStopRun: () => void;
  onStopSpeech: () => void;
  onRetry: () => void;
  selectedConversation: Conversation | undefined;
  activeTtsRunId: string | null;
  error: string | null;
  lastPrompt: string | null;
  retryKind: "response" | "speech" | null;
}) {
  const { t } = useTranslation();
  return <section className="chat-panel">
    <header className="topbar"><div><p className="eyebrow">{t("chat.eyebrow")}</p><h1>{t("chat.title")}</h1></div><div className="topbar-status"><span className="status-pill local-status"><span className="status-dot" />{modelProviderStatus.location === "cloud" ? t("chat.cloudProcessing") : modelProviderStatus.location === "local" ? t("chat.localProcessing") : t("chat.processingNotSelected")}</span><button className={!modelProviderStatus.ready ? "status-pill provider-status warning" : "status-pill provider-status"} onClick={onOpenSettings}><AppIcon name="model" />{localizeProviderLabel(t, modelProviderStatus.label)}{modelProviderStatus.state === "failed" ? t("chat.failedSuffix") : modelProviderStatus.state === "unchecked" ? t("chat.uncheckedSuffix") : modelProviderStatus.fallbackUsed ? t("chat.fallbackSuffix") : ""}</button></div></header>
    <div className="message-area" aria-live="polite">{messages.length === 0 && !streamingText ? <div className="empty-state"><h2>{t("chat.emptyTitle")}</h2><p>{t("chat.emptyDescription")}</p><div className="suggestion-list"><button type="button" onClick={() => onComposerChange(t("chat.suggestionOrganizePrompt"))}>{t("chat.suggestionOrganize")}</button><button type="button" onClick={onOpenMeeting}>{t("chat.suggestionMeeting")}</button><button type="button" onClick={onOpenSituation}>{t("chat.suggestionSituation")}</button></div></div> : messages.map((message) => <article className={`message ${message.role}`} key={message.id}><span className="message-role">{message.role === "user" ? t("chat.you") : t("chat.assistant")}</span><p>{message.content}</p></article>)}{streamingText && <article className="message assistant streaming"><span className="message-role">{t("chat.assistant")} · {t("chat.streaming")}</span><p>{streamingText}</p></article>}{interimTranscript && voiceState !== "idle" && <article className="message transcript streaming"><span className="message-role">{t("chat.transcript")} · {localizeStatus(t, voiceState)}</span><p>{interimTranscript}</p></article>}{runtimeActivity.length > 0 && <details className="activity-panel"><summary>{t("chat.runtimeActivity")}</summary>{runtimeActivity.map((activity, index) => <p key={`${index}-${activity.type}`}>{localizeRuntimeActivity(t, activity)}</p>)}</details>}</div>
    <form className="composer" onSubmit={onSubmit}><div className="composer-row"><button className={voiceState === "recording" ? "voice-button recording" : "voice-button"} type="button" aria-pressed={listeningEnabled} aria-label={voiceStarting ? t("chat.micCancel") : meetingActive ? t("chat.micPausedForMeeting") : !listeningEnabled ? t("chat.micResume") : voiceState === "recording" || voiceState === "transcribing" || activeTtsRunId ? t("chat.micPause") : t("chat.micRetry")} title={voiceStarting ? t("chat.micCancel") : meetingActive ? t("chat.micResumeAfterMeeting") : !listeningEnabled ? t("chat.micResume") : voiceState === "recording" || voiceState === "transcribing" || activeTtsRunId ? t("chat.micPause") : t("chat.micRetry")} onClick={onToggleVoice} disabled={meetingActive}><AppIcon name={listeningEnabled && (voiceStarting || voiceState === "recording" || voiceState === "transcribing" || Boolean(activeTtsRunId)) ? "stop" : "mic"} /></button><textarea rows={1} aria-label={t("chat.messageLabel")} onChange={(event) => onComposerChange(event.currentTarget.value)} onKeyDown={(event) => { if ((event.metaKey || event.ctrlKey) && event.key === "Enter") event.currentTarget.form?.requestSubmit(); }} placeholder={t("chat.placeholder")} value={composer} disabled={Boolean(activeRunId)} /><div className="composer-end">{activeRunId ? <button className="stop-button composer-stop" type="button" onClick={onStopRun}><AppIcon name="stop" /><span>{t("chat.stop")}</span></button> : <button className="send-button" type="submit" aria-label={t("chat.send")} disabled={!composer.trim() || !selectedConversation}><AppIcon name="send" /></button>}</div></div><div className="composer-meta" aria-live="polite">{voiceState === "recording" && <span className="composer-hint">{t("chat.listeningHint", { seconds: DEFAULT_VOICE_SILENCE_TIMEOUT_MS / 1_000 })}</span>}{!listeningEnabled && <span className="composer-hint">{t("chat.pausedHint")}</span>}{activeTtsRunId && listeningEnabled && <span className="composer-hint">{t("chat.speakingHint")}</span>}{activeTtsRunId && <button className="text-button" type="button" onClick={onStopSpeech}>{t("chat.stopSpeech")}</button>}{error && retryKind && !activeRunId && <button className="text-button" type="button" onClick={onRetry}>{retryKind === "speech" ? t("chat.retrySpeech") : t("chat.retryResponse")}</button>}</div></form>
    {error && <p className="error-banner" role="alert">{localizeUiMessage(t, error, "chat")}</p>}
  </section>;
}
