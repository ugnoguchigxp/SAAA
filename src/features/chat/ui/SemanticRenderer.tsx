import type { UiNode } from '../../../lib/generated/generativeUi';
import { MetricView, TableView, ChartView, ActionsView } from './components';
/** Only backend-validated semantic nodes reach this renderer. No eval/HTML or runtime tools. */
export function SemanticRenderer({ node }: { node: UiNode }) {
  const children = node.children.map(child => <SemanticRenderer key={child.id} node={child} />);
  const [source, field, label] = node.args;
  switch (node.kind) {
    case 'Grid': return <div className="ui-grid">{children}</div>;
    case 'Stack': return <div className="ui-stack">{children}</div>;
    case 'Cell': return <div className={`ui-cell ui-span-${node.span}`}>{children}</div>;
    case 'Text': return <p>{source}</p>;
    case 'Metric': case 'Status': return <MetricView source={source} field={field} label={label} />;
    case 'Table': return <TableView source={source} columns={field} stateId={node.id} />;
    case 'ModelStatus': return <TableView source={source} columns={source === 'larm.status' ? 'provider,runtime,status,updatedAt' : source === 'runtime.summary' ? 'running,completed,failed,total' : source === 'runtime.history' ? 'time,count' : 'provider,status,startedAt'} stateId={node.id} />;
    case 'Chart': return <ChartView source={source} />;
    case 'Actions': return <ActionsView action={source} />;
    default: throw new Error('Unsupported UI component');
  }
}
