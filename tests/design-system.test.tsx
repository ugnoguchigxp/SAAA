import { expect, test } from "bun:test";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import {
  Button,
  DESIGN_SYSTEM_VERSION,
  DesignSystemProvider,
  Input,
  Surface,
} from "../src/design-system";
import { SemanticRenderer } from "../src/features/chat/ui/SemanticRenderer";

test("design system provider publishes a stable version and theme contract", () => {
  const html = renderToStaticMarkup(
    createElement(DesignSystemProvider, null, createElement(Surface, null, "content")),
  );
  expect(DESIGN_SYSTEM_VERSION).toBe(1);
  expect(html).toContain('data-design-system-version="1"');
  expect(html).toContain('data-saaa-theme="dark"');
  expect(html).toContain('data-ds-component="surface"');
});

test("interactive primitives provide safe defaults without hiding native attributes", () => {
  const button = renderToStaticMarkup(createElement(Button, { disabled: true }, "Run"));
  const input = renderToStaticMarkup(
    createElement(Input, { "aria-label": "Query", maxLength: 12 }),
  );
  expect(button).toContain('type="button"');
  expect(button).toContain("disabled");
  expect(input).toContain('aria-label="Query"');
  expect(input).toContain('maxLength="12"');
});

test("semantic adapter normalizes unsupported persisted spans", () => {
  const html = renderToStaticMarkup(
    createElement(SemanticRenderer, {
      node: { id: "cell", kind: "Cell", args: [], span: 11, children: [] },
    }),
  );
  expect(html).toContain('data-span="12"');
  expect(html).not.toContain('data-span="11"');
});
