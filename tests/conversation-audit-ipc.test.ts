import { beforeEach, expect, test } from "bun:test";
import { invokeCalls, resetTauriCoreMock } from "./tauriCoreMock";

const { recordConversationCaptureAuditEvent } = await import("../src/lib/auditRuntime");

beforeEach(resetTauriCoreMock);

test("conversation ASR details use the dedicated audit command", () => {
  recordConversationCaptureAuditEvent({
    eventName: "conversation-asr-speech-detected",
    phase: "start",
    correlationId: "utterance-1",
    attributes: { observedAtMs: 123, text: "こんにちは。" },
  });
  expect(invokeCalls.at(-1)).toEqual({
    command: "record_conversation_capture_audit_event",
    args: {
      input: {
        eventName: "conversation-asr-speech-detected",
        phase: "start",
        correlationId: "utterance-1",
        attributes: { observedAtMs: 123, text: "こんにちは。" },
      },
    },
    options: undefined,
  });
});
