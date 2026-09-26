import { useCallback, useEffect, useRef, useState, type FormEvent } from "react";
import type { ConversationMessage } from "../../lib/contracts";
import { listMessages, submitConversationText } from "../../lib/runtime";
import "./conversationCheckPage.css";

export function ConversationCheckPage({
  conversationId,
  providerLabel,
  onOpenSettings,
}: {
  conversationId: string;
  providerLabel: string;
  onOpenSettings: () => void;
}) {
  const [messages, setMessages] = useState<ConversationMessage[]>([]);
  const [text, setText] = useState("");
  const [source, setSource] = useState<"configured" | "larm">("configured");
  const [sending, setSending] = useState(false);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [lastModel, setLastModel] = useState<string | null>(null);
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
        現在は今回の入力だけを送るテキスト回答の確認段階です。ASR、推論用
        LLM、読み上げはまだ接続されていません。
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
