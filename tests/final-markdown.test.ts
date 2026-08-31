import { describe, expect, test } from "bun:test";
import { renderSafeMarkdown } from "../src/features/chat/markdownRenderer";

describe("final Markdown projection", () => {
  test("renders completed structures while escaping raw HTML", () => {
    const html = renderSafeMarkdown([
      "# Result",
      "",
      "**safe** <script>alert(1)</script>",
      "",
      "| A | B |",
      "| --- | --- |",
      "| 1 | 2 |",
      "",
      "```ts",
      "const value = '<tag>';",
      "```",
    ].join("\n"));
    expect(html).toContain("<h1>Result</h1>");
    expect(html).toContain("<strong>safe</strong>");
    expect(html).toContain("<table>");
    expect(html).toContain("language-ts");
    expect(html).not.toContain("<script>");
    expect(html).toContain("&lt;script&gt;");
  });

  test("allows only explicit safe link protocols", () => {
    const html = renderSafeMarkdown("[safe](https://example.com) [unsafe](javascript:alert(1))");
    expect(html).toContain('href="https://example.com"');
    expect(html).not.toContain('href="javascript:');
  });

  test("treats an unfinished code fence as escaped code at completion", () => {
    const html = renderSafeMarkdown("```html\n<img src=x onerror=alert(1)>");
    expect(html).toContain("<pre><code class=\"language-html\">");
    expect(html).toContain("&lt;img src=x onerror=alert(1)&gt;");
  });

  test("renders long plain and marker-heavy text without changing content", () => {
    const plain = "x".repeat(64_000);
    expect(renderSafeMarkdown(plain)).toBe(`<p>${plain}</p>`);

    const unmatchedMarkers = "[".repeat(16_000);
    expect(renderSafeMarkdown(unmatchedMarkers)).toBe(`<p>${unmatchedMarkers}</p>`);
  });
});
