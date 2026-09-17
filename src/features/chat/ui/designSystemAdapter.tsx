import type { ButtonHTMLAttributes, HTMLAttributes, InputHTMLAttributes, ReactNode } from "react";
import {
  Button,
  Grid,
  GridCell,
  Input,
  Stack,
  Surface,
  Table,
  TableViewport,
  Text,
} from "../../../design-system";

type SemanticSpan = 3 | 4 | 6 | 8 | 12;

function normalizeSpan(span: number): SemanticSpan {
  return span === 3 || span === 4 || span === 6 || span === 8 ? span : 12;
}

/**
 * Stable boundary between persisted Semantic UI nodes and the visual Design System.
 * Design System component names and props must not be persisted or exposed to the LLM.
 */
export const semanticDesignSystem = {
  Grid({ children }: { children: ReactNode }) {
    return <Grid data-semantic-component="Grid">{children}</Grid>;
  },
  Stack({ children }: { children: ReactNode }) {
    return <Stack data-semantic-component="Stack">{children}</Stack>;
  },
  Cell({ span, children }: { span: number; children: ReactNode }) {
    return (
      <GridCell data-semantic-component="Cell" span={normalizeSpan(span)}>
        {children}
      </GridCell>
    );
  },
  Text({ children }: { children: ReactNode }) {
    return <Text data-semantic-component="Text">{children}</Text>;
  },
  Metric({ children, kind = "Metric" }: { children: ReactNode; kind?: "Metric" | "Status" }) {
    return (
      <Surface className="ui-metric" data-semantic-component={kind}>
        {children}
      </Surface>
    );
  },
  DataPanel({
    children,
    className,
    kind = "Table",
  }: {
    children: ReactNode;
    className: string;
    kind?: "Table" | "ModelStatus";
  }) {
    return (
      <section className={className} data-semantic-component={kind}>
        {children}
      </section>
    );
  },
  Chart({ children }: { children: ReactNode }) {
    return (
      <figure className="ui-chart" data-semantic-component="Chart">
        {children}
      </figure>
    );
  },
  MutedText({ children, ...props }: HTMLAttributes<HTMLParagraphElement>) {
    return (
      <Text {...props} tone="muted" size="sm">
        {children}
      </Text>
    );
  },
  Button(props: ButtonHTMLAttributes<HTMLButtonElement>) {
    return <Button {...props} />;
  },
  ActionButton(props: ButtonHTMLAttributes<HTMLButtonElement>) {
    return <Button {...props} data-semantic-component="Actions" />;
  },
  QuietButton(props: ButtonHTMLAttributes<HTMLButtonElement>) {
    return <Button {...props} variant="quiet" />;
  },
  Input(props: InputHTMLAttributes<HTMLInputElement>) {
    return <Input {...props} />;
  },
  Table({ children }: { children: ReactNode }) {
    return (
      <TableViewport>
        <Table>{children}</Table>
      </TableViewport>
    );
  },
};
