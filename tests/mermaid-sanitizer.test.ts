import { afterEach, describe, expect, test } from "bun:test";
import { JSDOM } from "jsdom";
import { renderMermaidDiagrams, sanitizeMermaidSvg } from "../src/features/chat/ui/mermaid";

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
      '<svg viewBox="0 0 10 10" onload="bad()"><script>bad()</script><a href="javascript:bad()"><text>linked</text></a><foreignObject>bad</foreignObject><g><path d="M0 0" onclick="bad()"/><style>@import "https://bad"; .x{fill:red}</style><style>.node{fill:red}</style></g></svg>',
    );
    expect(clean).not.toBeNull();
    expect(clean!.querySelector("script, a, foreignObject")).toBeNull();
    expect(clean!.querySelector("path")?.getAttribute("onclick")).toBeNull();
    expect(clean!.querySelector("style")?.textContent).toBe("");
    expect(clean!.querySelectorAll("style")[1]?.textContent).toBe(".node{fill:red}");
    expect(clean!.getAttribute("viewBox")).toBe("0 0 10 10");
  });

  test("does not render an already completed block twice", async () => {
    const dom = new JSDOM(
      '<!doctype html><body><div id="root"><div class="mermaid-block"><div class="mermaid-diagram"><svg></svg></div><div class="mermaid-source" hidden>graph TD; A--&gt;B</div></div></div></body>',
    );
    const names = ["window", "document", "DOMParser", "Node"] as const;
    const previous = names.map((name) => Object.getOwnPropertyDescriptor(globalThis, name));
    const values = [dom.window, dom.window.document, dom.window.DOMParser, dom.window.Node];
    names.forEach((name, index) =>
      Object.defineProperty(globalThis, name, { configurable: true, value: values[index] }),
    );
    restore = () => {
      names.forEach((name, index) => {
        const descriptor = previous[index];
        if (descriptor) Object.defineProperty(globalThis, name, descriptor);
        else Reflect.deleteProperty(globalThis, name);
      });
      dom.window.close();
    };
    const root = document.getElementById("root")!;
    await renderMermaidDiagrams(root, "failed");
    await renderMermaidDiagrams(root, "failed");
    expect(root.querySelectorAll(".mermaid-diagram")).toHaveLength(1);
    expect(root.querySelector(".mermaid-source, .mermaid-error")).toBeNull();
  });
});
