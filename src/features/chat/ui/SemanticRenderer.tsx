import type { UiNode } from "../../../lib/generated/generativeUi";
import { MetricView, TableView, ChartView, ActionsView } from "./components";
import { semanticDesignSystem as ui } from "./designSystemAdapter";
import { MarkdownView } from "./MarkdownView";
import { ModelStatusView } from "./ModelStatusView";
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
      return <ModelStatusView source={source} stateId={node.id} />;
    case "Chart":
      return <ChartView source={source} />;
    case "Actions":
      return <ActionsView action={source} />;
    default:
      throw new Error("Unsupported UI component");
  }
}
