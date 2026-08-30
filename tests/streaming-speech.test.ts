import { describe, expect, test } from "bun:test";
import { FinalResponseSpeechGate, speechRetry } from "../src/features/chat/speechPlaybackPolicy";
import type { RuntimeEvent } from "../src/lib/contracts";

const completed = (id: string, content: string): RuntimeEvent => ({
  type: "messageCompleted",
  runId: "run_1",
  message: { id, conversationId: "conversation", role: "assistant", content, createdAt: "1" },
});

describe("gapless response speech", () => {
  test("never synthesizes incomplete model deltas", () => {
    const gate = new FinalResponseSpeechGate();
    expect(gate.accept({ type: "delta", runId: "run_1", text: "incomplete" })).toBeNull();
    expect(gate.accept({ type: "activity", runId: "run_1", kind: "tool", summary: "working" })).toBeNull();
  });

  test("speaks the persisted final assistant message exactly once", () => {
    const gate = new FinalResponseSpeechGate();
    const event = completed("message_1", "complete answer");
    expect(gate.accept(event)).toBe("complete answer");
    expect(gate.accept(event)).toBeNull();
  });

  test("speech retry keeps the complete final response", () => {
    expect(speechRetry("the complete final response", "conversation")).toEqual({
      kind: "speech",
      text: "the complete final response",
      conversationId: "conversation",
    });
  });
});
