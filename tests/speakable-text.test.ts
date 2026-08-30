import { describe, expect, test } from "bun:test";
import { toSpeakableText } from "../src/lib/speakableText";

describe("speakable text", () => {
  test("keeps useful labels while removing Markdown syntax, URLs, and emoji", () => {
    const text = toSpeakableText("## 結論\n- [公式資料](https://example.com/docs) を確認してください ✅");
    expect(text).toBe("結論。公式資料 を確認してください");
    expect(text).not.toContain("http");
    expect(text).not.toContain("[");
  });

  test("does not read fenced source code aloud", () => {
    const text = toSpeakableText("次の例です。\n```ts\nconst secret = 1;\n```\n以上です。");
    expect(text).toBe("次の例です。コードブロックは省略します。以上です。");
    expect(text).not.toContain("const secret");
  });

  test("returns an empty value for non-speakable-only content", () => {
    expect(toSpeakableText("https://example.com 🚀")).toBe("");
  });
});
