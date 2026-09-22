import { lazy, Suspense, memo, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import type { ConversationMessage } from "../../lib/contracts";
import { renderFinalMarkdown } from "./finalMarkdown";
import type { StreamingTextProjection } from "./streamingTextBuffer";
import { recordMarkdownPaint } from "./streamingPerformance";

import { UiBoundary } from "./ui/UiBoundary";
import { renderMermaidDiagrams } from "./ui/mermaid";

const InlineUi = lazy(() => import("./ui/InlineUi"));

const MarkdownMessage = memo(function MarkdownMessage({
  messageId,
  content,
}: {
  messageId: string;
  content: string;
}) {
  const { t } = useTranslation();
  const [html, setHtml] = useState<string | null>(null);
  const host = useRef<HTMLDivElement>(null);
  useEffect(() => {
    let active = true;
    setHtml(null);
    void renderFinalMarkdown(messageId, content)
      .then((rendered) => {
        if (active) {
          setHtml(rendered);
          requestAnimationFrame(() => recordMarkdownPaint(messageId));
        }
      })
      .catch(() => undefined);
    return () => {
      active = false;
    };
  }, [messageId, content]);
  useEffect(() => {
    if (html !== null && host.current) {
      void renderMermaidDiagrams(host.current, t("genui.diagramFailed"));
    }
  }, [html, t]);
  return html === null ? (
    <p className="markdown-pending">{content}</p>
  ) : (
    <div ref={host} className="markdown-content" dangerouslySetInnerHTML={{ __html: html }} />
  );
});

export const CompletedMessage = memo(function CompletedMessage({
  message,
}: {
  message: ConversationMessage;
}) {
  const { t } = useTranslation();
  return (
    <article
      className={`message ${message.role}${message.parts?.some((part) => part.type === "ui") ? " has-ui" : ""}`}
    >
      <span className="message-role">
        {message.id.startsWith("lfm_request_")
          ? "Qwenへの依頼 · 発言の原文"
          : message.role === "user"
            ? t("chat.you")
            : message.id.startsWith("lfm_reply_")
              ? "LFM · 会話応対"
              : t("chat.assistant")}
      </span>
      {message.parts?.length ? (
        message.parts.map((part, index) =>
          part.type === "ui" ? (
            <UiBoundary
              key={part.instanceId}
              fallback={
                <p>
                  {part.summary} · {t("genui.unavailable")}
                </p>
              }
            >
              <Suspense fallback={<p>{part.summary}</p>}>
                <InlineUi
                  instanceId={part.instanceId}
                  conversationId={message.conversationId}
                  summary={part.summary}
                />
              </Suspense>
            </UiBoundary>
          ) : (
            <MarkdownMessage key={index} messageId={`${message.id}:${index}`} content={part.text} />
          ),
        )
      ) : message.role === "assistant" ? (
        <MarkdownMessage messageId={message.id} content={message.content} />
      ) : (
        <p>{message.content}</p>
      )}
    </article>
  );
});

const StreamingChunk = memo(function StreamingChunk({ value }: { value: string }) {
  return <span>{value}</span>;
});

export const StreamingPlainText = memo(function StreamingPlainText({
  projection,
}: {
  projection: StreamingTextProjection;
}) {
  return (
    <p className="streaming-plain-text">
      {projection.chunks.map((chunk, index) => (
        <StreamingChunk key={index} value={chunk} />
      ))}
      <span>{projection.tail}</span>
    </p>
  );
});
