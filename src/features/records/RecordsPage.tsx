import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import type { Conversation, ConversationMessage } from "../../lib/contracts";
import { listMessages } from "../../lib/runtime";
import "../workspacePages.css";

function formatTime(value: string, locale: string): string {
  const numeric = Number(value);
  const date = new Date(Number.isFinite(numeric) ? numeric : value);
  return Number.isNaN(date.getTime()) ? value : date.toLocaleString(locale);
}

export function RecordsPage({
  conversations,
  initialConversationId,
  targetMessageId,
  onTargetHandled,
}: {
  conversations: Conversation[];
  initialConversationId: string | null;
  targetMessageId: string | null;
  onTargetHandled: () => void;
}) {
  const { t, i18n } = useTranslation();
  const [conversationId, setConversationId] = useState(initialConversationId);
  const [messages, setMessages] = useState<ConversationMessage[]>([]);
  const [nextCursor, setNextCursor] = useState<string | null>(null);
  const [hasMore, setHasMore] = useState(false);
  const [query, setQuery] = useState("");
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState("");
  const targetRef = useRef<HTMLElement | null>(null);

  useEffect(() => {
    if (initialConversationId) setConversationId(initialConversationId);
  }, [initialConversationId]);

  const load = useCallback(async (id: string, cursor: string | null, append: boolean) => {
    setLoading(true);
    try {
      const page = await listMessages(id, cursor);
      setMessages((current) => {
        const next = append ? [...page.messages, ...current] : page.messages;
        return [...new Map(next.map((message) => [message.id, message])).values()].sort(
          (a, b) => Number(a.createdAt) - Number(b.createdAt),
        );
      });
      setNextCursor(page.nextCursor);
      setHasMore(page.hasMore);
      setError("");
    } catch (cause) {
      setError(String(cause));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    if (!conversationId) return;
    setMessages([]);
    setNextCursor(null);
    setHasMore(false);
    void load(conversationId, null, false);
  }, [conversationId, load]);

  useEffect(() => {
    if (!targetMessageId || !messages.some((message) => message.id === targetMessageId)) return;
    const frame = requestAnimationFrame(() => {
      targetRef.current?.scrollIntoView({ block: "center" });
      targetRef.current?.focus();
      onTargetHandled();
    });
    return () => cancelAnimationFrame(frame);
  }, [messages, onTargetHandled, targetMessageId]);

  const visible = useMemo(() => {
    const normalized = query.trim().toLocaleLowerCase();
    return messages.filter(
      (message) =>
        (message.role === "user" || message.role === "assistant") &&
        (!normalized || message.content.toLocaleLowerCase().includes(normalized)),
    );
  }, [messages, query]);

  return (
    <section className="workspace-page records-page" aria-labelledby="records-page-title">
      <header className="workspace-page-header">
        <h1 id="records-page-title">{t("navigation.records")}</h1>
        <input
          className="records-search"
          type="search"
          value={query}
          onChange={(event) => setQuery(event.currentTarget.value)}
          placeholder={t("recordsPage.search")}
          aria-label={t("recordsPage.search")}
        />
      </header>
      <div className="records-layout">
        <nav className="records-conversations" aria-label={t("recordsPage.conversations")}>
          {conversations
            .slice()
            .sort((a, b) => b.updatedAt.localeCompare(a.updatedAt))
            .map((conversation) => (
              <button
                type="button"
                key={conversation.id}
                className={conversation.id === conversationId ? "active" : undefined}
                aria-current={conversation.id === conversationId ? "page" : undefined}
                onClick={() => setConversationId(conversation.id)}
              >
                <strong>{conversation.title || t("recordsPage.untitled")}</strong>
                <span>
                  {formatTime(conversation.updatedAt, i18n.resolvedLanguage ?? i18n.language)}
                </span>
              </button>
            ))}
        </nav>
        <div className="records-messages">
          {hasMore ? (
            <button
              type="button"
              className="workspace-secondary-button records-load-more"
              disabled={loading || !conversationId}
              onClick={() => conversationId && void load(conversationId, nextCursor, true)}
            >
              {t("recordsPage.older")}
            </button>
          ) : null}
          {error ? (
            <p className="workspace-error" role="alert">
              {error}
            </p>
          ) : null}
          {loading && messages.length === 0 ? (
            <p className="workspace-empty">{t("common.loading")}</p>
          ) : null}
          {!loading && visible.length === 0 ? (
            <p className="workspace-empty">{t("recordsPage.empty")}</p>
          ) : null}
          {visible.map((message) => (
            <article
              key={message.id}
              ref={message.id === targetMessageId ? targetRef : undefined}
              tabIndex={message.id === targetMessageId ? -1 : undefined}
              className={`record-message ${message.role}${message.id === targetMessageId ? " target" : ""}`}
            >
              <header>
                <strong>
                  {message.role === "user" ? t("recordsPage.user") : t("recordsPage.assistant")}
                </strong>
                <time>{formatTime(message.createdAt, i18n.resolvedLanguage ?? i18n.language)}</time>
              </header>
              <p>{message.content}</p>
            </article>
          ))}
        </div>
      </div>
    </section>
  );
}
