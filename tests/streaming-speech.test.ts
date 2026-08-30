import { describe, expect, test } from "bun:test";
import { readFileSync } from "node:fs";

const conversationTurn = readFileSync(
  new URL("../src/features/chat/useConversationTurn.ts", import.meta.url),
  "utf8",
);

describe("gapless response speech", () => {
  test("never synthesizes incomplete model deltas", () => {
    const deltaCase = conversationTurn.slice(
      conversationTurn.indexOf('case "delta"'),
      conversationTurn.indexOf('case "activity"'),
    );
    expect(deltaCase).not.toContain("speakText");
    expect(conversationTurn).not.toContain("StreamingSpeechChunker");
  });

  test("speaks the persisted final assistant message exactly once", () => {
    const completedCase = conversationTurn.slice(
      conversationTurn.indexOf('case "messageCompleted"'),
      conversationTurn.indexOf('case "cancelled"'),
    );
    expect(completedCase).toContain("startSpeech(event.message.content, conversationId)");
    expect(completedCase.match(/startSpeech/g)).toHaveLength(1);
  });

  test("speech retry keeps the complete final response", () => {
    const startSpeech = conversationTurn.slice(
      conversationTurn.indexOf("async function startSpeech"),
      conversationTurn.indexOf("async function stopSpeech"),
    );
    expect(startSpeech).toContain('setRetryAction({ kind: "speech", text, conversationId })');
  });
});
