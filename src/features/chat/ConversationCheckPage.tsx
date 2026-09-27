import {
  useCallback,
  useEffect,
  useRef,
  useState,
  useSyncExternalStore,
  type FormEvent,
} from "react";
import { AppIcon } from "../../components/AppIcon";
import type { ConversationMessage } from "../../lib/contracts";
import {
  conversationAsrSnapshot,
  startConversationAsr,
  stopConversationAsr,
  subscribeConversationAsr,
  setConversationAsrPlaybackActive,
  queueConversationAsrDelivery,
} from "../../lib/conversationAsrCapture";
import { listMessages, speakConversationAnswer, submitConversationText } from "../../lib/runtime";
import "./conversationCheckPage.css";

type RouteStage = "qwen" | "ornith" | "tts" | null;
const routeNodes = [
  { id: "asr", label: "ASR" },
  { id: "qwen", label: "Qwen 2B" },
  { id: "ornith", label: "Ornith 1.5" },
  { id: "tts", label: "TTS" },
] as const;

export function ConversationCheckPage({
  conversationId,
  inputDeviceId,
  echoCancellation,
  listeningEnabled,
  vadSensitivity,
  silenceTimeoutMs,
  onOpenSettings,
}: {
  conversationId: string;
  providerLabel: string;
  inputDeviceId: string;
  echoCancellation: boolean;
  listeningEnabled: boolean;
  vadSensitivity: "low" | "medium" | "high";
  silenceTimeoutMs: number;
  onOpenSettings: () => void;
}) {
  const [messages, setMessages] = useState<ConversationMessage[]>([]);
  const [text, setText] = useState("");
  const [sending, setSending] = useState(false);
  const [stage, setStage] = useState<RouteStage>(null);
  const [lastReplySource, setLastReplySource] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const audio = useSyncExternalStore(subscribeConversationAsr, conversationAsrSnapshot);
  const pendingId = useRef<string | null>(null);
  const audioResponseQueue = useRef(Promise.resolve());
  const autoStartAttempted = useRef(false);
  const bottomRef = useRef<HTMLDivElement | null>(null);
  const latestAsrError = audio.entries[0]?.status === "failed" ? audio.entries[0].error : null;
  const transcribing = audio.entries.some((entry) => entry.status === "transcribing");
  const asrActive =
    audio.phase === "starting" ||
    (audio.phase === "recording" && !audio.playbackLimited);
  const displayedText = text || audio.interimText;
  const status =
    stage === "qwen" ? "Qwen 2B が振り分け中" :
    stage === "ornith" ? "Ornith 1.5 が回答を作成中" :
    stage === "tts" ? "TTS で回答を再生中" :
    asrActive ? (audio.speechDetected || audio.interimText || transcribing ? "音声を認識中" : "音声を待っています") :
    audio.phase === "starting" ? "マイクを準備中" : "マイクは停止中";

  const refresh = useCallback(async () => {
    const page = await listMessages(conversationId, null);
    setMessages(page.messages.filter((message) => message.role === "user" || message.role === "assistant"));
  }, [conversationId]);

  useEffect(() => {
    let active = true;
    setLoading(true);
    void listMessages(conversationId, null)
      .then((page) => {
        if (active) {
          setMessages(page.messages.filter((message) => message.role === "user" || message.role === "assistant"));
          setError(null);
        }
      })
      .catch((cause) => { if (active) setError(String(cause)); })
      .finally(() => { if (active) setLoading(false); });
    return () => { active = false; };
  }, [conversationId]);

  useEffect(() => {
    bottomRef.current?.scrollIntoView({ block: "end" });
  }, [messages, sending]);

  useEffect(() => {
    if (!listeningEnabled || autoStartAttempted.current) return;
    autoStartAttempted.current = true;
    void startConversationAsr(inputDeviceId, echoCancellation, vadSensitivity, silenceTimeoutMs);
  }, [listeningEnabled, inputDeviceId, echoCancellation, vadSensitivity, silenceTimeoutMs]);

  const send = useCallback(async (content: string, inputId: string) => {
    if (!content.trim() || new TextEncoder().encode(content).length > 4096) return;
    pendingId.current = inputId;
    setText(content);
    setSending(true);
    setError(null);
    setStage("qwen");
    try {
      const result = await submitConversationText(inputId, content, "larm", (nextStage) => setStage(nextStage));
      setLastReplySource(result.providerLabel.includes("Qwen") ? "Qwen 2B" : result.providerLabel.includes("ornith") ? "Ornith 1.5" : result.providerLabel);
      await refresh();
      setText((current) => current === content ? "" : current);
      pendingId.current = null;
      setStage("tts");
      setConversationAsrPlaybackActive(true, inputId);
      try {
        await speakConversationAnswer(inputId);
      } catch (cause) {
        setError(`回答は保存しましたが、読み上げに失敗しました: ${String(cause)}`);
      } finally {
        setConversationAsrPlaybackActive(false, inputId);
      }
    } catch (cause) {
      const message = String(cause);
      setError(message.includes("larm_authentication_failed")
        ? "LARM の認証に失敗しました。保存済みの接続設定と Qwen の Provider 認証を確認してください。"
        : message);
    } finally {
      setStage(null);
      setSending(false);
    }
  }, [refresh]);

  useEffect(() => {
    for (const capture of audio.entries.slice().reverse()) {
      if (capture.status !== "completed" || !capture.text?.trim() || capture.deliveryQueued) continue;
      if (!queueConversationAsrDelivery(capture.id)) continue;
      const content = capture.text;
      audioResponseQueue.current = audioResponseQueue.current.then(() => send(content, capture.id));
    }
  }, [audio.entries, send]);

  async function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (sending || !displayedText.trim()) return;
    const content = displayedText;
    const inputId = pendingId.current ?? crypto.randomUUID();
    audioResponseQueue.current = audioResponseQueue.current.then(() => send(content, inputId));
    await audioResponseQueue.current;
  }

  const toggleRecording = () => {
    void (audio.phase === "recording"
      ? stopConversationAsr()
      : startConversationAsr(inputDeviceId, echoCancellation, vadSensitivity, silenceTimeoutMs));
  };

  return (
    <section className="conversation-check" aria-label="会話">
      <header className="conversation-check-header">
        <h1>会話</h1>
        <button type="button" onClick={onOpenSettings}>設定</button>
      </header>
      <div className="conversation-check-history" aria-live="polite">
        {loading && <p>会話を読み込み中…</p>}
        {!loading && messages.length === 0 && <p>マイクを有効にするか、メッセージを入力してください。</p>}
        {messages.map((message) => (
          <article key={message.id} className={`conversation-check-message ${message.role}`}>
            <strong>{message.role === "user" ? "あなた" : "SAAA"}</strong>
            <p>{message.content}</p>
          </article>
        ))}
        <div ref={bottomRef} />
      </div>
      {(error || audio.error || latestAsrError) && <p role="alert" className="conversation-check-error">{error || audio.error || latestAsrError}</p>}
      <div className="conversation-route" aria-label="回答の経路">
        <div className="conversation-route-track">
          {routeNodes.map((node, index) => (
            <div className="conversation-route-segment" key={node.id}>
              {index > 0 && <span className="conversation-route-link" aria-hidden="true">→</span>}
              <div className={`conversation-route-node${(node.id === "asr" ? asrActive && !stage : stage === node.id) ? " active" : ""}`}>
                <span className="conversation-route-lamp" aria-hidden="true" />
                <span>{node.label}</span>
              </div>
            </div>
          ))}
        </div>
        <p className="conversation-route-status" role="status">{status}{!stage && lastReplySource ? ` · 前回回答: ${lastReplySource}` : ""}</p>
      </div>
      <form className="composer conversation-check-composer" onSubmit={(event) => void submit(event)}>
        <div className="composer-row">
          <button
            className={audio.phase === "recording" ? "voice-button recording" : "voice-button"}
            type="button"
            aria-label={audio.phase === "recording" ? "録音を停止" : "録音を開始"}
            aria-pressed={audio.phase === "recording"}
            disabled={audio.phase === "starting" || audio.phase === "stopping"}
            onClick={toggleRecording}
          >
            <AppIcon name={audio.phase === "recording" ? "stop" : "mic"} />
          </button>
          <textarea
            rows={1}
            aria-label="プロンプト全文"
            value={displayedText}
            onChange={(event) => { setText(event.currentTarget.value); pendingId.current = null; }}
            onKeyDown={(event) => {
              if ((event.metaKey || event.ctrlKey) && event.key === "Enter") event.currentTarget.form?.requestSubmit();
            }}
            placeholder="メッセージを入力、または話しかけてください"
          />
          <div className="composer-end">
            <button className="send-button" type="submit" aria-label="送信" disabled={sending || !displayedText.trim() || new TextEncoder().encode(displayedText).length > 4096}>
              <AppIcon name="send" />
            </button>
          </div>
        </div>
      </form>
    </section>
  );
}
