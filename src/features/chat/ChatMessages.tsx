import { memo, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import type { ConversationMessage } from "../../lib/contracts";
import { renderFinalMarkdown } from "./finalMarkdown";
import type { StreamingTextProjection } from "./streamingTextBuffer";
import { recordMarkdownPaint } from "./streamingPerformance";

const MarkdownMessage = memo(function MarkdownMessage({ messageId, content }: { messageId: string; content: string }) {
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
    return () => { active = false; };
  }, [messageId, content]);
  return html === null
    ? <p className="markdown-pending">{content}</p>
    : <div className="markdown-content" dangerouslySetInnerHTML={{ __html: html }} />;
});

export const CompletedMessage = memo(function CompletedMessage({ message }: { message: ConversationMessage }) {
  const { t } = useTranslation();
  return <article className={`message ${message.role}`}>
    <span className="message-role">{message.role === "user" ? t("chat.you") : t("chat.assistant")}</span>
    {message.role === "assistant" ? <MarkdownMessage messageId={message.id} content={message.content} /> : <p>{message.content}</p>}
  </article>;
});

const StreamingChunk = memo(function StreamingChunk({ value }: { value: string }) {
  return <span>{value}</span>;
});

export const StreamingPlainText = memo(function StreamingPlainText({ projection }: { projection: StreamingTextProjection }) {
  return <p className="streaming-plain-text">
    {projection.chunks.map((chunk, index) => <StreamingChunk key={index} value={chunk} />)}
    <span>{projection.tail}</span>
  </p>;
});
