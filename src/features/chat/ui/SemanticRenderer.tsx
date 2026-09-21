import { useEffect, useMemo, useRef } from "react";
import { useTranslation } from "react-i18next";
import type { UiNode } from "../../../lib/generated/generativeUi";
import { renderSafeMarkdown } from "../markdownRenderer";
import { MetricView, TableView, ChartView, ActionsView } from "./components";
import { semanticDesignSystem as ui } from "./designSystemAdapter";
import { renderMermaidDiagrams } from "./mermaid";

function MarkdownView({ text, displayMode }: { text: string; displayMode: "inline" | "artifact" }) {
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
/** Only backend-validated semantic nodes reach this renderer. No eval/HTML or runtime tools. */
export function SemanticRenderer({
  node,
  displayMode = "inline",
}: {
  node: UiNode;
  displayMode?: "inline" | "artifact";
}) {
  const children = node.children.map((child) => (
    <SemanticRenderer key={child.id} node={child} displayMode={displayMode} />
  ));
  const [source, field, label] = node.args;
  switch (node.kind) {
    case "Grid":
      return <ui.Grid>{children}</ui.Grid>;
    case "Stack":
      return <ui.Stack>{children}</ui.Stack>;
    case "Cell":
      return <ui.Cell span={node.span}>{children}</ui.Cell>;
    case "Text":
      return <ui.Text>{source}</ui.Text>;
    case "Markdown":
      return <MarkdownView text={source} displayMode={displayMode} />;
    case "Metric":
    case "Status":
      return <MetricView source={source} field={field} label={label} kind={node.kind} />;
    case "Table":
      return <TableView source={source} columns={field} stateId={node.id} />;
    case "ModelStatus":
      return (
        <TableView
          source={source}
          columns={
            source === "larm.status"
              ? "provider,runtime,status,updatedAt"
              : source === "runtime.summary"
                ? "running,completed,failed,total"
                : source === "runtime.history"
                  ? "time,count"
                  : "provider,status,startedAt"
          }
          stateId={node.id}
          kind="ModelStatus"
        />
      );
    case "Chart":
      return <ChartView source={source} />;
    case "Actions":
      return <ActionsView action={source} />;
    default:
      throw new Error("Unsupported UI component");
  }
}
