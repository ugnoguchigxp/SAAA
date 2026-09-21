import { afterEach, describe, expect, test } from "bun:test";
import { JSDOM } from "jsdom";
import { sanitizeMermaidSvg } from "../src/features/chat/ui/mermaid";

describe("Mermaid SVG sanitizer", () => {
  let restore: (() => void) | null = null;

  afterEach(() => restore?.());

  test("rebuilds only allowlisted SVG elements and attributes", () => {
    const dom = new JSDOM("<!doctype html><body></body>");
    const names = ["window", "document", "DOMParser", "Node"] as const;
    const previous = names.map((name) => Object.getOwnPropertyDescriptor(globalThis, name));
    const values = [dom.window, dom.window.document, dom.window.DOMParser, dom.window.Node];
    names.forEach((name, index) =>
      Object.defineProperty(globalThis, name, {
        configurable: true,
        value: values[index],
      }),
    );
    restore = () => {
      names.forEach((name, index) => {
        const descriptor = previous[index];
        if (descriptor) Object.defineProperty(globalThis, name, descriptor);
        else Reflect.deleteProperty(globalThis, name);
      });
      dom.window.close();
    };
    const clean = sanitizeMermaidSvg(
      '<svg viewBox="0 0 10 10" onload="bad()"><script>bad()</script><a href="javascript:bad()"><text>linked</text></a><foreignObject>bad</foreignObject><g><path d="M0 0" onclick="bad()"/><style>.x{fill:url(https://bad)}</style></g></svg>',
    );
    expect(clean).not.toBeNull();
    expect(clean!.querySelector("script, a, foreignObject")).toBeNull();
    expect(clean!.querySelector("path")?.getAttribute("onclick")).toBeNull();
    expect(clean!.querySelector("style")?.textContent).toBe("");
    expect(clean!.getAttribute("viewBox")).toBe("0 0 10 10");
  });
});
