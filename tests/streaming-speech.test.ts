import { describe, expect, test } from "bun:test";
import { initialConversationSession, transitionConversationSession } from "../src/lib/conversationSession";
import type { RuntimeEvent } from "../src/lib/contracts";

const started = (): RuntimeEvent => ({
  type: "speechStarted",
  runId: "run_1",
});

describe("streaming response speech", () => {
  test("uses a dedicated speech-start event instead of a completed message", () => {
    expect(started()).toEqual({ type: "speechStarted", runId: "run_1" });
    const completed: RuntimeEvent = {
      type: "messageCompleted",
      runId: "run_1",
      message: { id: "message_1", conversationId: "conversation", role: "assistant", content: "complete answer", createdAt: "1" },
      presentation: { decision: "speak", reasonCode: "global_default" },
      voicePolicy: null,
    };
    expect(completed.type).toBe("messageCompleted");
  });

  test("keeps the microphone suppression state until native speech ends", () => {
    const speaking = transitionConversationSession(initialConversationSession, {
      type: "speechStarted",
      runId: "run_1",
    });
    expect(speaking.speechRunId).toBe("run_1");
    expect(transitionConversationSession(speaking, { type: "speechFinished", runId: "run_1" }).speechRunId).toBeNull();
  });

  test("does not let a stale completion clear a newer speech session", () => {
    const speaking = transitionConversationSession(initialConversationSession, {
      type: "speechStarted",
      runId: "run_2",
    });
    expect(transitionConversationSession(speaking, { type: "speechFinished", runId: "run_1" }).speechRunId).toBe("run_2");
  });
});
