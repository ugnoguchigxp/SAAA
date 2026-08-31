import { describe, expect, test } from "bun:test";
import { readFileSync } from "node:fs";

const source = (path: string) => readFileSync(new URL(`../${path}`, import.meta.url), "utf8");

describe("interrupted streaming UI", () => {
  test("keeps interrupted output as explicitly incomplete plain text", () => {
    const turn = source("src/features/chat/useConversationTurn.ts");
    const chat = source("src/features/chat/ChatPage.tsx");
    expect(turn).toContain("incompleteRunIdsRef.current.add(event.runId)");
    expect(turn).toContain("const preserveIncomplete = hasStreamingText()");
    expect(turn).not.toContain('case "providerFailed":\n        resetStreamingText()');
    expect(chat).toContain('!activeRunId ? "incomplete" : "streaming"');
    expect(chat).toContain('"chat.incomplete"');
    expect(chat).toContain("<StreamingPlainText projection={streamingText} />");
  });
});
