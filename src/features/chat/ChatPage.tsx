import { type FormEvent } from "react";
import { AppIcon } from "../../components/AppIcon";
import type { Conversation, ConversationMessage } from "../../lib/contracts";
import { DEFAULT_VOICE_SILENCE_TIMEOUT_MS } from "../../lib/voiceActivity";
import type { VoiceCaptureState } from "../voice/useAmbientVoiceSession";

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
  runtimeActivity: string[];
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
  return <section className="chat-panel">
    <header className="topbar"><div><p className="eyebrow">CONTINUOUS CONVERSATION</p><h1>SAAAとの会話</h1></div><div className="topbar-status"><span className="status-pill local-status"><span className="status-dot" />{modelProviderStatus.location === "cloud" ? "クラウド処理" : modelProviderStatus.location === "local" ? "ローカル処理" : "処理先未選択"}</span><button className={!modelProviderStatus.ready ? "status-pill provider-status warning" : "status-pill provider-status"} onClick={onOpenSettings}><AppIcon name="model" />{modelProviderStatus.state === "failed" ? `${modelProviderStatus.label}（失敗）` : modelProviderStatus.state === "unchecked" ? `${modelProviderStatus.label}（未確認）` : `${modelProviderStatus.label}${modelProviderStatus.fallbackUsed ? "（Fallback）" : ""}`}</button></div></header>
    <div className="message-area" aria-live="polite">{messages.length === 0 && !streamingText ? <div className="empty-state"><h2>今日はどうしましたか？</h2><p>声で話すか、メッセージを入力してください。</p><div className="suggestion-list"><button type="button" onClick={() => onComposerChange("考えを整理するのを手伝ってください")}>考えを整理する</button><button type="button" onClick={onOpenMeeting}>会議を文字起こし</button><button type="button" onClick={onOpenSituation}>状況を確認</button></div></div> : messages.map((message) => <article className={`message ${message.role}`} key={message.id}><span className="message-role">{message.role === "user" ? "You" : message.role}</span><p>{message.content}</p></article>)}{streamingText && <article className="message assistant streaming"><span className="message-role">assistant · streaming</span><p>{streamingText}</p></article>}{interimTranscript && voiceState !== "idle" && <article className="message transcript streaming"><span className="message-role">transcript · {voiceState}</span><p>{interimTranscript}</p></article>}{runtimeActivity.length > 0 && <details className="activity-panel"><summary>Runtime activity</summary>{runtimeActivity.map((activity, index) => <p key={`${index}-${activity}`}>{activity}</p>)}</details>}</div>
    <form className="composer" onSubmit={onSubmit}><div className="composer-row"><button className={voiceState === "recording" ? "voice-button recording" : "voice-button"} type="button" aria-pressed={listeningEnabled} aria-label={voiceStarting ? "マイクの準備を中止" : meetingActive ? "Meeting中は常時待ち受けを一時停止中" : !listeningEnabled ? "常時待ち受けを再開" : voiceState === "recording" || voiceState === "transcribing" || activeTtsRunId ? "常時待ち受けを一時停止" : "常時待ち受けを再試行"} title={voiceStarting ? "マイクの準備を中止" : meetingActive ? "Meeting終了後に常時待ち受けを自動再開します" : !listeningEnabled ? "常時待ち受けを再開" : voiceState === "recording" || voiceState === "transcribing" || activeTtsRunId ? "常時待ち受けを一時停止" : "常時待ち受けを再試行"} onClick={onToggleVoice} disabled={meetingActive}><AppIcon name={listeningEnabled && (voiceStarting || voiceState === "recording" || voiceState === "transcribing" || Boolean(activeTtsRunId)) ? "stop" : "mic"} /></button><textarea rows={1} aria-label="Message" onChange={(event) => onComposerChange(event.currentTarget.value)} onKeyDown={(event) => { if ((event.metaKey || event.ctrlKey) && event.key === "Enter") event.currentTarget.form?.requestSubmit(); }} placeholder="SAAAにメッセージ" value={composer} disabled={Boolean(activeRunId)} /><div className="composer-end">{activeRunId ? <button className="stop-button composer-stop" type="button" onClick={onStopRun}><AppIcon name="stop" /><span>停止</span></button> : <button className="send-button" type="submit" aria-label="送信" disabled={!composer.trim() || !selectedConversation}><AppIcon name="send" /></button>}</div></div><div className="composer-meta" aria-live="polite">{voiceState === "recording" && <span className="composer-hint">常時待ち受け中です。話し終えて{DEFAULT_VOICE_SILENCE_TIMEOUT_MS / 1_000}秒待つと自動送信します。</span>}{!listeningEnabled && <span className="composer-hint">常時待ち受けは一時停止中です。</span>}{activeTtsRunId && listeningEnabled && <span className="composer-hint">読み上げ中は待ち受けを一時停止し、終了後に自動再開します。</span>}{activeTtsRunId && <button className="text-button" type="button" onClick={onStopSpeech}>読み上げを停止</button>}{error && retryKind && !activeRunId && <button className="text-button" type="button" onClick={onRetry}>{retryKind === "speech" ? "読み上げを再試行" : "応答を再試行"}</button>}</div></form>
    {error && <p className="error-banner" role="alert">{error}</p>}
  </section>;
}
