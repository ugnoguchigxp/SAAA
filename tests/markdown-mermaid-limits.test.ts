import { describe, expect, test } from "bun:test";
import { renderSafeMarkdown } from "../src/features/chat/markdownRenderer";

describe("Markdown Mermaid and size limits", () => {
  test("keeps Mermaid source as escaped text for main-thread rendering", () => {
    const html = renderSafeMarkdown("```mermaid\ngraph TD\nA[<script>] --> B\n```");
    expect(html).toContain('<pre data-lang="mermaid">');
    expect(html).toContain('<div class="mermaid-source" hidden>');
    expect(html).toContain("A[&lt;script&gt;] --&gt; B");
    expect(html).not.toContain("<script>");
  });

  test("renders long plain and marker-heavy text without changing content", () => {
    const plain = "x".repeat(64_000);
    expect(renderSafeMarkdown(plain)).toBe(`<p>${plain}</p>`);
    const unmatchedMarkers = "[".repeat(16_000);
    expect(renderSafeMarkdown(unmatchedMarkers)).toBe(`<p>${unmatchedMarkers}</p>`);
  });
});
