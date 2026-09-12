import { createLibrary, defineComponent, createParser } from '@openuidev/react-lang';
import { z } from 'zod';
import { MetricView, TableView, ChartView, ActionsView } from '../src/features/chat/ui/components';

const Text = defineComponent({ name: 'Text', description: 'Plain text', props: z.object({ text: z.string() }), component: ({ props }) => <p>{props.text}</p> });
const Grid = defineComponent({ name: 'Grid', description: 'Responsive twelve-column grid', props: z.object({ children: z.array(z.unknown()) }), component: ({ props, renderNode }) => <div className="ui-grid">{renderNode(props.children)}</div> });
const Stack = defineComponent({ name: 'Stack', description: 'Vertical stack', props: z.object({ children: z.array(z.unknown()) }), component: ({ props, renderNode }) => <div className="ui-stack">{renderNode(props.children)}</div> });
const Cell = defineComponent({ name: 'Cell', description: 'Relative width', props: z.object({ child: z.unknown(), span: z.number() }), component: ({ props, renderNode }) => <div className={`ui-cell ui-span-${props.span}`}>{renderNode(props.child)}</div> });
const metricProps = z.object({ source: z.string(), field: z.string(), label: z.string() });
const Metric = defineComponent({ name: 'Metric', description: 'Bound metric', props: metricProps.clone(), component: ({ props }) => <MetricView {...props} /> });
const Status = defineComponent({ name: 'Status', description: 'Bound status', props: metricProps.clone(), component: ({ props }) => <MetricView {...props} /> });
const Table = defineComponent({ name: 'Table', description: 'Bound sortable table', props: z.object({ source: z.string(), columns: z.string() }), component: ({ props, statementId }) => <TableView {...props} stateId={statementId ?? `table:${props.source}:${props.columns}`} /> });
const ModelStatus = defineComponent({ name: 'ModelStatus', description: 'Session status, not model inventory', props: z.object({ source: z.string() }), component: ({ props, statementId }) => <TableView source={props.source} columns={props.source === 'larm.status' ? 'provider,runtime,status,updatedAt' : props.source === 'runtime.summary' ? 'running,completed,failed,total' : props.source === 'runtime.history' ? 'time,count' : 'provider,status,startedAt'} stateId={statementId ?? `models:${props.source}`} /> });
const Chart = defineComponent({ name: 'Chart', description: 'Runs started by minute', props: z.object({ source: z.string(), x: z.string(), y: z.string() }), component: ({ props }) => <ChartView source={props.source} /> });
const Actions = defineComponent({ name: 'Actions', description: 'Bound actions', props: z.object({ action: z.string() }), component: ({ props }) => <ActionsView action={props.action} /> });
export const uiLibrary = createLibrary({ components: [Text, Grid, Stack, Cell, Metric, Status, Table, ModelStatus, Chart, Actions] });
const parser = createParser(uiLibrary.toJSONSchema());
export function validateRendererDefinition(definition: string) {
  const parsed = parser.parse(definition);
  if (!parsed.root || parsed.meta.incomplete || parsed.meta.errors.length || parsed.meta.unresolved.length || parsed.queryStatements.length || parsed.mutationStatements.length || Object.keys(parsed.stateDeclarations).length) throw new Error('Unsupported UI definition');
  return parsed;
}
