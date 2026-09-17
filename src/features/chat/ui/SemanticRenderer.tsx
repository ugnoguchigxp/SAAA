import type { UiNode } from "../../../lib/generated/generativeUi";
import { MetricView, TableView, ChartView, ActionsView } from "./components";
import { semanticDesignSystem as ui } from "./designSystemAdapter";
/** Only backend-validated semantic nodes reach this renderer. No eval/HTML or runtime tools. */
export function SemanticRenderer({ node }: { node: UiNode }) {
  const children = node.children.map((child) => <SemanticRenderer key={child.id} node={child} />);
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
