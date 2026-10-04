import { expect, test } from "bun:test";
import { invokeCalls, resetTauriCoreMock } from "./tauriCoreMock";
const { createConversationCaptureAudit } = await import("../src/lib/conversationCaptureAudit");
test("current capture audit retains session correlation and bounds details", () => {
  resetTauriCoreMock();
  const audit = createConversationCaptureAudit(() => "capture-1");
  audit("conversation-asr-capture-start", "request", "utterance-1", { inputDeviceId: "default" });
  audit(
    "conversation-asr-capture-stop",
    "error",
    "utterance-1",
    { error: "長い詳細".repeat(1000) },
    "failure",
  );
  const events = invokeCalls.map(
    (call) =>
      call.args?.input as {
        correlationId: string;
        attributes: Record<string, unknown>;
        outcome?: string;
      },
  );
  expect(events[0].correlationId).toBe("utterance-1");
  expect(events[0].attributes.captureSessionId).toBe("capture-1");
  expect(events.at(-1)?.outcome).toBe("failure");
  expect(events.at(-1)?.attributes.overflow).toBe(true);
  for (const event of events)
    expect(new TextEncoder().encode(JSON.stringify(event.attributes)).length).toBeLessThanOrEqual(
      1500,
    );
});
