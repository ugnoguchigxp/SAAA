import { describe, expect, test } from "bun:test";
import { extractAnswerUrls } from "../src/features/chat/artifacts/answerUrls";

describe("answer urls", () => {
  test("keeps markdown and plain urls in order and skips code and images", () => {
    const urls = extractAnswerUrls(
      "See [News](https://example.com/a) and https://example.com/b.\n```\nhttps://skip.example/c\n```\n`https://code.example/e`\n![pic](https://images.example/d.png)",
    );
    expect(urls.map((item) => item.url)).toEqual([
      "https://example.com/a",
      "https://example.com/b",
    ]);
    expect(urls[0]?.label).toBe("News");
  });
  test("preserves a plain URL before a later markdown link", () => {
    expect(extractAnswerUrls("https://first.example/a then [Second](https://second.example/b)").map((item) => item.url)).toEqual([
      "https://first.example/a",
      "https://second.example/b",
    ]);
  });
});
