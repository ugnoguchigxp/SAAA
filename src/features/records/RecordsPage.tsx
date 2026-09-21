import {
  createColumnHelper,
  rowSortingFeature,
  tableFeatures,
  useTable,
  type SortingState,
} from "@tanstack/react-table";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import type { Conversation, ConversationMessage } from "../../lib/contracts";
import { listMessages } from "../../lib/runtime";
import "../workspacePages.css";

const recordsTableFeatures = tableFeatures({ rowSortingFeature });
const columnHelper = createColumnHelper<typeof recordsTableFeatures, ConversationMessage>();

function parseTimestamp(value: string): Date {
  const numeric = Number(value);
  return new Date(Number.isFinite(numeric) ? numeric : value);
}

function dateKey(value: string): string {
  const date = parseTimestamp(value);
  if (Number.isNaN(date.getTime())) return value;
  const year = date.getFullYear();
  const month = String(date.getMonth() + 1).padStart(2, "0");
  const day = String(date.getDate()).padStart(2, "0");
  return `${year}-${month}-${day}`;
}

function formatDate(value: string, locale: string): string {
  const date = parseTimestamp(value);
  if (Number.isNaN(date.getTime())) return value;
  return new Intl.DateTimeFormat(locale, {
    year: "numeric",
    month: "short",
    day: "numeric",
    weekday: "short",
  }).format(date);
}

function formatTime(value: string, locale: string): string {
  const date = parseTimestamp(value);
  if (Number.isNaN(date.getTime())) return value;
  return new Intl.DateTimeFormat(locale, {
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
  }).format(date);
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
  const locale = i18n.resolvedLanguage ?? i18n.language;
  const [conversationId, setConversationId] = useState(
    initialConversationId ?? conversations[0]?.id ?? null,
  );
  const [messages, setMessages] = useState<ConversationMessage[]>([]);
  const [nextCursor, setNextCursor] = useState<string | null>(null);
  const [hasMore, setHasMore] = useState(false);
  const [selectedDate, setSelectedDate] = useState<string | null>(null);
  const [query, setQuery] = useState("");
  const [sorting, setSorting] = useState<SortingState>([{ id: "time", desc: false }]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState("");
  const targetRef = useRef<HTMLTableRowElement | null>(null);

  useEffect(() => {
    setConversationId(initialConversationId ?? conversations[0]?.id ?? null);
  }, [conversations, initialConversationId]);

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
    if (!conversationId) {
      setMessages([]);
      return;
    }
    setMessages([]);
    setNextCursor(null);
    setHasMore(false);
    setSelectedDate(null);
    void load(conversationId, null, false);
  }, [conversationId, load]);

  const recordMessages = useMemo(
    () => messages.filter((message) => message.role === "user" || message.role === "assistant"),
    [messages],
  );

  const dates = useMemo(() => {
    const grouped = new Map<string, { key: string; timestamp: string; count: number }>();
    for (const message of recordMessages) {
      const key = dateKey(message.createdAt);
      const current = grouped.get(key);
      if (current) current.count += 1;
      else grouped.set(key, { key, timestamp: message.createdAt, count: 1 });
    }
    return [...grouped.values()].sort(
      (a, b) => parseTimestamp(b.timestamp).getTime() - parseTimestamp(a.timestamp).getTime(),
    );
  }, [recordMessages]);

  useEffect(() => {
    const target = targetMessageId
      ? recordMessages.find((message) => message.id === targetMessageId)
      : null;
    if (target) {
      setSelectedDate(dateKey(target.createdAt));
      return;
    }
    if (!selectedDate || !dates.some((date) => date.key === selectedDate)) {
      setSelectedDate(dates[0]?.key ?? null);
    }
  }, [dates, recordMessages, selectedDate, targetMessageId]);

  const visible = useMemo(() => {
    const normalized = query.trim().toLocaleLowerCase();
    return recordMessages.filter(
      (message) =>
        (!selectedDate || dateKey(message.createdAt) === selectedDate) &&
        (!normalized || message.content.toLocaleLowerCase().includes(normalized)),
    );
  }, [query, recordMessages, selectedDate]);

  useEffect(() => {
    if (!targetMessageId || !visible.some((message) => message.id === targetMessageId)) return;
    const frame = requestAnimationFrame(() => {
      targetRef.current?.scrollIntoView({ block: "center" });
      targetRef.current?.focus();
      onTargetHandled();
    });
    return () => cancelAnimationFrame(frame);
  }, [onTargetHandled, targetMessageId, visible]);

  const columns = useMemo(
    () =>
      columnHelper.columns([
        columnHelper.accessor("createdAt", {
          id: "time",
          header: t("recordsPage.columns.time"),
          cell: ({ getValue }) => <time>{formatTime(getValue(), locale)}</time>,
        }),
        columnHelper.accessor("role", {
          id: "speaker",
          header: t("recordsPage.columns.speaker"),
          cell: ({ getValue }) => (
            <span className={`record-role ${getValue()}`}>
              {getValue() === "user" ? t("recordsPage.user") : t("recordsPage.assistant")}
            </span>
          ),
        }),
        columnHelper.accessor("content", {
          id: "content",
          header: t("recordsPage.columns.content"),
          enableSorting: false,
          cell: ({ getValue }) => <span className="record-content">{getValue()}</span>,
        }),
      ]),
    [locale, t],
  );

  const table = useTable({
    features: recordsTableFeatures,
    columns,
    data: visible,
    state: { sorting },
    onSortingChange: setSorting,
    enableMultiSort: false,
  });

  return (
    <section className="workspace-page records-page" aria-label={t("navigation.records")}>
      <div
        className="workspace-page-header workspace-page-toolbar records-header"
        role="toolbar"
        aria-label={t("navigation.records")}
      >
        <div className="records-header-controls">
          {conversations.length > 1 ? (
            <select
              className="records-conversation-select"
              value={conversationId ?? ""}
              aria-label={t("recordsPage.conversations")}
              onChange={(event) => setConversationId(event.currentTarget.value)}
            >
              {conversations.map((conversation) => (
                <option key={conversation.id} value={conversation.id}>
                  {conversation.title || t("recordsPage.untitled")}
                </option>
              ))}
            </select>
          ) : null}
          <input
            className="records-search"
            type="search"
            value={query}
            onChange={(event) => setQuery(event.currentTarget.value)}
            placeholder={t("recordsPage.search")}
            aria-label={t("recordsPage.search")}
          />
        </div>
      </div>
      <div className="records-layout">
        <nav className="records-dates" aria-label={t("recordsPage.dates")}>
          <div className="records-date-list">
            {dates.map((date) => (
              <button
                type="button"
                key={date.key}
                className={date.key === selectedDate ? "active" : undefined}
                aria-current={date.key === selectedDate ? "date" : undefined}
                onClick={() => setSelectedDate(date.key)}
              >
                <span>{formatDate(date.timestamp, locale)}</span>
                <small>{t("recordsPage.messageCount", { count: date.count })}</small>
              </button>
            ))}
          </div>
          {hasMore ? (
            <button
              type="button"
              className="records-load-more"
              disabled={loading || !conversationId}
              onClick={() => conversationId && void load(conversationId, nextCursor, true)}
            >
              {t("recordsPage.older")}
            </button>
          ) : null}
        </nav>
        <div className="records-table-region">
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
          {visible.length > 0 ? (
            <div className="records-table-scroll" tabIndex={0}>
              <table className="records-table">
                <thead>
                  {table.getHeaderGroups().map((group) => (
                    <tr key={group.id}>
                      {group.headers.map((header) => (
                        <th key={header.id} scope="col">
                          {header.isPlaceholder ? null : header.column.getCanSort() ? (
                            <button type="button" onClick={header.column.getToggleSortingHandler()}>
                              <table.FlexRender header={header} />
                              <span aria-hidden="true">
                                {header.column.getIsSorted() === "asc"
                                  ? "▲"
                                  : header.column.getIsSorted() === "desc"
                                    ? "▼"
                                    : "↕"}
                              </span>
                            </button>
                          ) : (
                            <table.FlexRender header={header} />
                          )}
                        </th>
                      ))}
                    </tr>
                  ))}
                </thead>
                <tbody>
                  {table.getRowModel().rows.map((row) => (
                    <tr
                      key={row.original.id}
                      ref={row.original.id === targetMessageId ? targetRef : undefined}
                      tabIndex={row.original.id === targetMessageId ? -1 : undefined}
                      className={row.original.id === targetMessageId ? "target" : undefined}
                    >
                      {row.getAllCells().map((cell) => (
                        <td key={cell.id}>
                          <table.FlexRender cell={cell} />
                        </td>
                      ))}
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          ) : null}
        </div>
      </div>
    </section>
  );
}
