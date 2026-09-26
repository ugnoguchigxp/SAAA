import {
  useCallback,
  useEffect,
  useRef,
  useState,
  useSyncExternalStore,
  type FormEvent,
} from "react";
import type { ConversationMessage } from "../../lib/contracts";
import {
  conversationAsrSnapshot,
  startConversationAsr,
  stopConversationAsr,
  subscribeConversationAsr,
} from "../../lib/conversationAsrCapture";
import { listMessages, submitConversationText } from "../../lib/runtime";
import "./conversationCheckPage.css";

export function ConversationCheckPage({
  conversationId,
  providerLabel,
  inputDeviceId,
  echoCancellation,
  onOpenSettings,
}: {
  conversationId: string;
  providerLabel: string;
  inputDeviceId: string;
  echoCancellation: boolean;
  onOpenSettings: () => void;
}) {
  const [messages, setMessages] = useState<ConversationMessage[]>([]);
  const [text, setText] = useState("");
  const [source, setSource] = useState<"configured" | "larm">("configured");
  const [sending, setSending] = useState(false);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [lastModel, setLastModel] = useState<string | null>(null);
  const audio = useSyncExternalStore(subscribeConversationAsr, conversationAsrSnapshot);
  const captures = audio.entries;
  const pendingId = useRef<string | null>(null);
  const bottomRef = useRef<HTMLDivElement | null>(null);

  const refresh = useCallback(async () => {
    const page = await listMessages(conversationId, null);
    setMessages(
      page.messages.filter((message) => message.role === "user" || message.role === "assistant"),
    );
  }, [conversationId]);

  useEffect(() => {
    let active = true;
    setLoading(true);
    void listMessages(conversationId, null)
      .then((page) => {
        if (active) {
          setMessages(
            page.messages.filter(
              (message) => message.role === "user" || message.role === "assistant",
            ),
          );
          setError(null);
        }
      })
      .catch((cause) => {
        if (active) setError(String(cause));
      })
      .finally(() => {
        if (active) setLoading(false);
      });
    return () => {
      active = false;
    };
  }, [conversationId]);

  useEffect(() => {
    bottomRef.current?.scrollIntoView({ block: "end" });
  }, [messages, sending]);

  async function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (sending || !text.trim() || new TextEncoder().encode(text).length > 4096) return;
    const inputId = pendingId.current ?? crypto.randomUUID();
    pendingId.current = inputId;
    setSending(true);
    setError(null);
    try {
      const result = await submitConversationText(inputId, text, source);
      setLastModel(`${result.providerLabel} · ${result.model}`);
      await refresh();
      setText("");
      pendingId.current = null;
    } catch (cause) {
      setError(String(cause));
    } finally {
      setSending(false);
    }
  }

  return (
    <section className="conversation-check" aria-label="会話">
      <header className="conversation-check-header">
        <div>
          <h1>会話</h1>
          <p>接続先: {source === "larm" ? "保存済み LARM" : providerLabel}</p>
        </div>
        <div className="conversation-check-actions">
          <button
            type="button"
            onClick={() => void refresh().catch((cause) => setError(String(cause)))}
          >
            履歴を更新
          </button>
          <button type="button" onClick={onOpenSettings}>
            Provider 設定
          </button>
        </div>
      </header>
      <p className="conversation-check-note" role="status">
        録音開始後は停止するまで音声を取り込み、10秒ごとに保存済みのASRルートへ送ります。各区間の音声と結果を表示します。推論用LLMと読み上げは未接続です。
      </p>
      <div className="conversation-check-history" aria-live="polite">
        {loading && <p>会話を読み込み中…</p>}
        {!loading && messages.length === 0 && <p>入力して会話用 Provider の回答を確認できます。</p>}
        {messages.map((message) => (
          <article key={message.id} className={`conversation-check-message ${message.role}`}>
            <strong>{message.role === "user" ? "あなた" : "SAAA"}</strong>
            <p>{message.content}</p>
          </article>
        ))}
        {sending && <p role="status">回答を取得中…</p>}
        <div ref={bottomRef} />
      </div>
      {lastModel && <p className="conversation-check-model">直近の回答: {lastModel}</p>}
      <div className="conversation-check-audio">
        <button
          type="button"
          disabled={audio.phase === "starting" || audio.phase === "stopping"}
          onClick={() =>
            void (audio.phase === "recording"
              ? stopConversationAsr()
              : startConversationAsr(inputDeviceId, echoCancellation))
          }
        >
          {audio.phase === "recording" ? "録音を停止" : "録音を開始"}
        </button>
        <span role="status">
          {audio.phase === "recording"
            ? "録音中 · 停止するまで継続"
            : audio.phase === "starting"
              ? "マイクを開始中…"
              : audio.phase === "stopping"
                ? "マイクを停止中…"
                : "停止中"}
        </span>
        {audio.error && <p role="alert" className="conversation-check-error">{audio.error}</p>}
        <div className="conversation-check-capture-list" aria-label="ASRの録音と結果">
          <h2>録音・ASR結果（この起動中の全件）</h2>
          {captures.length === 0 && (
            <p>録音した音声と文字起こしを、このアプリの起動中すべて表示します。</p>
          )}
          {captures.map((capture, index) => (
            <article key={capture.id} className="conversation-check-audio-preview">
              <strong>
                録音 {captures.length - index} · {capture.recordedAt}
              </strong>
              {capture.preview ? (
                <>
                  <span>ASRへ送った音声 · {capture.preview.seconds.toFixed(1)}秒</span>
                  <svg viewBox="0 0 256 48" role="img" aria-label="録音した音声の波形">
                    {capture.preview.peaks.map((peak, peakIndex) => {
                      const height = Math.max(2, Math.min(46, peak * 46));
                      return (
                        <rect
                          key={peakIndex}
                          x={peakIndex * 4}
                          y={(48 - height) / 2}
                          width="2"
                          height={height}
                        />
                      );
                    })}
                  </svg>
                  <audio
                    controls
                    src={capture.preview.url}
                    aria-label={`録音 ${captures.length - index} を再生`}
                  />
                </>
              ) : (
                <span>マイクから音声データを受け取れませんでした。</span>
              )}
              <p>
                ASR結果:{" "}
                {capture.status === "transcribing" ? "処理中…" : (capture.text ?? "文字起こしなし")}
              </p>
              {capture.text && (
                <button
                  type="button"
                  onClick={() => {
                    setText((current) => current ? `${current}\n${capture.text}` : capture.text ?? "");
                    pendingId.current = null;
                  }}
                >
                  入力欄に追加
                </button>
              )}
              {capture.provider && (
                <small>
                  {capture.provider} · {capture.language ?? "言語未判定"}
                </small>
              )}
              {capture.error && <small className="conversation-check-error">{capture.error}</small>}
            </article>
          ))}
        </div>
      </div>
      {error && (
        <p role="alert" className="conversation-check-error">
          {error}
        </p>
      )}
      <form className="conversation-check-composer" onSubmit={(event) => void submit(event)}>
        <label htmlFor="conversation-check-source">試行する接続先</label>
        <select
          id="conversation-check-source"
          value={source}
          disabled={sending}
          onChange={(event) => {
            setSource(event.target.value as "configured" | "larm");
            pendingId.current = null;
          }}
        >
          <option value="configured">保存済みの会話ルート</option>
          <option value="larm">保存済みの LARM 接続先と profile</option>
        </select>
        <label htmlFor="conversation-check-input">メッセージ</label>
        <textarea
          id="conversation-check-input"
          value={text}
          disabled={sending}
          onChange={(event) => {
            setText(event.target.value);
            pendingId.current = null;
          }}
          onKeyDown={(event) => {
            if (event.key === "Enter" && !event.shiftKey && !event.nativeEvent.isComposing) {
              event.preventDefault();
              event.currentTarget.form?.requestSubmit();
            }
          }}
          rows={3}
          placeholder="メッセージを入力"
        />
        <button
          type="submit"
          disabled={sending || !text.trim() || new TextEncoder().encode(text).length > 4096}
        >
          送信
        </button>
      </form>
    </section>
  );
}
