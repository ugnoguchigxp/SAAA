import { expect, test } from "bun:test";
import { renderToStaticMarkup } from "react-dom/server";
import { createElement } from "react";
import { SemanticRenderer } from "../src/features/chat/ui/SemanticRenderer";
import type { UiNode } from "../src/lib/generated/generativeUi";
test("semantic renderer uses the design system adapter and treats generated text as text", () => {
  const text: UiNode = {
    id: "label",
    kind: "Text",
    args: ['<script>alert("x")</script>'],
    children: [],
    span: 12,
  };
  const html = renderToStaticMarkup(
    createElement(SemanticRenderer, {
      node: {
        id: "root",
        kind: "Grid",
        args: [],
        span: 12,
        children: [{ id: "cell", kind: "Cell", args: [], span: 8, children: [text] }],
      },
    }),
  );
  expect(html).toContain('data-ds-component="grid"');
  expect(html).toContain('data-semantic-component="Cell"');
  expect(html).toContain('data-span="8"');
  expect(html).toContain("&lt;script&gt;");
  expect(html).not.toContain("<script>");
});

test("unknown nodes fail within the host error boundary instead of executing arbitrary code", () => {
  expect(() =>
    renderToStaticMarkup(
      createElement(SemanticRenderer, {
        node: { id: "x", kind: "eval", args: ["1+1"], span: 12, children: [] },
      }),
    ),
  ).toThrow("Unsupported UI component");
});
