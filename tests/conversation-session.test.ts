import { describe, expect, test } from "bun:test";
import { initialConversationSession, transitionConversationSession } from "../src/lib/conversationSession";

describe("conversation session coordinator", () => {
  test("tracks model and speech runs without allowing duplicate owners", () => {
    const running = transitionConversationSession(initialConversationSession, { type: "runStarted", runId: "run_1" });
    const speaking = transitionConversationSession(running, { type: "speechStarted", runId: "speech_1" });
    expect(speaking).toEqual({ runId: "run_1", speechRunId: "speech_1" });
    expect(() => transitionConversationSession(speaking, { type: "runStarted", runId: "run_2" })).toThrow();
    expect(() => transitionConversationSession(speaking, { type: "speechStarted", runId: "speech_2" })).toThrow();
  });

  test("ignores stale completion events", () => {
    const running = transitionConversationSession(initialConversationSession, { type: "runStarted", runId: "run_1" });
    expect(transitionConversationSession(running, { type: "runFinished", runId: "old" })).toEqual(running);
    expect(transitionConversationSession(running, { type: "runFinished", runId: "run_1" })).toEqual(initialConversationSession);
  });
});
