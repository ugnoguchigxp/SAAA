import { lazy, Suspense, memo, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import type { ConversationMessage } from "../../lib/contracts";
import { renderFinalMarkdown } from "./finalMarkdown";
import type { StreamingTextProjection } from "./streamingTextBuffer";
import { recordMarkdownPaint } from "./streamingPerformance";

import { UiBoundary } from "./ui/UiBoundary";
import { renderMermaidDiagrams } from "./ui/mermaid";
import { useArtifactWorkspace } from "./artifacts/ArtifactDrawer";

const InlineUi = lazy(() => import("./ui/InlineUi"));

const MarkdownMessage = memo(function MarkdownMessage({
  messageId,
  conversationId,
  content,
}: {
  messageId: string;
  conversationId: string;
  content: string;
}) {
  const { t } = useTranslation();
  const artifacts = useArtifactWorkspace();
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
    <div
      ref={host}
      className="markdown-content"
      onClick={(event) => {
        const anchor = (event.target as Element).closest("a[href]");
        if (!anchor || !event.currentTarget.contains(anchor) || !artifacts) return;
        const href = anchor.getAttribute("href");
        if (!href || !/^https?:\/\//i.test(href)) return;
        event.preventDefault();
        artifacts.openSource({ conversationId, url: href, title: anchor.textContent?.trim() || href });
      }}
      dangerouslySetInnerHTML={{ __html: html }}
    />
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
            <MarkdownMessage key={index} messageId={`${message.id}:${index}`} conversationId={message.conversationId} content={part.text} />
          ),
        )
      ) : message.role === "assistant" ? (
        <MarkdownMessage messageId={message.id} conversationId={message.conversationId} content={message.content} />
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
