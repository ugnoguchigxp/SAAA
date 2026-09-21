import { useEffect, useMemo, useRef } from "react";
import { useTranslation } from "react-i18next";
import { renderSafeMarkdown } from "../markdownRenderer";
import { renderMermaidDiagrams } from "./mermaid";

export function MarkdownView({
  text,
  displayMode,
}: {
  text: string;
  displayMode: "inline" | "artifact";
}) {
  const { t } = useTranslation();
  const host = useRef<HTMLDivElement>(null);
  const html = useMemo(() => renderSafeMarkdown(text), [text]);
  useEffect(() => {
    if (host.current) void renderMermaidDiagrams(host.current, t("genui.diagramFailed"));
  }, [html, t]);
  return (
    <div
      ref={host}
      className={`markdown-content ui-markdown ui-markdown-${displayMode}`}
      dangerouslySetInnerHTML={{ __html: html }}
    />
  );
}
