import { lazy, Suspense, memo, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import type { ConversationMessage } from "../../lib/contracts";
import { renderFinalMarkdown } from "./finalMarkdown";
import type { StreamingTextProjection } from "./streamingTextBuffer";
import { recordMarkdownPaint } from "./streamingPerformance";

import { UiBoundary } from "./ui/UiBoundary";

const InlineUi = lazy(() => import("./ui/InlineUi"));

const MarkdownMessage = memo(function MarkdownMessage({
  messageId,
  content,
}: {
  messageId: string;
  content: string;
}) {
  const [html, setHtml] = useState<string | null>(null);
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
  return html === null ? (
    <p className="markdown-pending">{content}</p>
  ) : (
    <div className="markdown-content" dangerouslySetInnerHTML={{ __html: html }} />
  );
});

export const CompletedMessage = memo(function CompletedMessage({
  message,
  scopeKeys,
}: {
  message: ConversationMessage;
  scopeKeys?: string[];
}) {
  const { t } = useTranslation();
  return (
    <article
      className={`message ${message.role}${message.parts?.some((part) => part.type === "ui") ? " has-ui" : ""}`}
    >
      <span className="message-role">
        {message.role === "user" ? t("chat.you") : t("chat.assistant")}
      </span>
      {scopeKeys?.length ? <p className="message-scope">対象: {scopeKeys.join("、")}</p> : null}
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
