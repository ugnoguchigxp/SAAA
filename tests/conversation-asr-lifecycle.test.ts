import { expect, mock, test } from "bun:test";
let browserStartGate: Promise<void> | null = null;
let browserStartFailure: Error | null = null;
let browserStarts = 0,
  browserStops = 0,
  releases = 0;
mock.module("../src/lib/auditRuntime", () => ({ recordConversationCaptureAuditEvent: () => {} }));
mock.module("../src/lib/qwenRealtimeAsr", () => ({
  conversationAsrTransport: async () => "http",
  startQwenAsrSession: async () => {},
  stopQwenAsrSession: async () => {},
  appendQwenAsrAudio: async () => {},
}));
mock.module("../src/lib/audioBackend", () => ({
  audioBackendStatus: async () => ({ available: false }),
  nativeCapturePreferred: () => false,
  startNativeVoiceCapture: async () => ({ available: false }),
  stopNativeVoiceCapture: async () => {},
}));
mock.module("../src/lib/runtime", () => ({
  releaseConversationAsrSession: async () => {
    releases++;
  },
  transcribeConversationAudio: async () => ({ text: "hello", providerLabel: "fixture" }),
}));
mock.module("../src/lib/browserVoiceCapture", () => ({
  startBrowserVoiceCapture: async () => {
    browserStarts++;
    await browserStartGate;
    if (browserStartFailure) throw browserStartFailure;
    return {
      stop: async () => {
        browserStops++;
      },
    };
  },
}));
const capture = await import("../src/lib/conversationAsrCapture");
const { publish } = await import("../src/lib/conversationAsrState");
const tick = () => new Promise((resolve) => setTimeout(resolve, 20));

test("repeated start shares startup and OFF retires late resources before another start", async () => {
  let finish!: () => void;
  browserStartGate = new Promise<void>((resolve) => {
    finish = resolve;
  });
  const starts = browserStarts;
  const stops = browserStops;
  const first = capture.startConversationAsr("default", false);
  const duplicate = capture.startConversationAsr("default", false);
  expect(duplicate).toBe(first);
  await tick();
  expect(browserStarts).toBe(starts + 1);
  const stop = capture.stopConversationAsr();
  expect(capture.conversationAsrSnapshot().phase).toBe("stopping");
  const duringStop = capture.startConversationAsr("default", false);
  finish();
  await Promise.all([first, duplicate, stop, duringStop]);
  browserStartGate = null;
  expect(browserStarts).toBe(starts + 1);
  expect(browserStops).toBe(stops + 1);
  expect(capture.conversationAsrSnapshot().phase).toBe("idle");
  await capture.startConversationAsr("default", false);
  expect(browserStarts).toBe(starts + 2);
  expect(capture.conversationAsrSnapshot().phase).toBe("recording");
  await capture.stopConversationAsr();
});

test("microphone startup failure releases the ASR session and allows retry", async () => {
  const releasedBefore = releases;
  browserStartFailure = new Error("permission denied");
  await capture.startConversationAsr("default", false);
  expect(capture.conversationAsrSnapshot().phase).toBe("idle");
  expect(capture.conversationAsrSnapshot().error).toContain("permission denied");
  expect(releases).toBe(releasedBefore + 1);
  browserStartFailure = null;
  await capture.startConversationAsr("default", false);
  expect(capture.conversationAsrSnapshot().phase).toBe("recording");
  await capture.stopConversationAsr();
});

test("final delivery is accepted once and a failed enqueue retains a retryable transcript", () => {
  publish({
    entries: [
      {
        id: "recognized",
        status: "completed",
        text: "hello",
        language: "ja",
        provider: "fixture",
        error: null,
        deliveryQueued: false,
      },
    ],
  });
  const entry = capture
    .conversationAsrSnapshot()
    .entries.find((e) => e.status === "completed" && !e.deliveryQueued && e.text?.trim());
  expect(entry).toBeDefined();
  const id = entry!.id;
  expect(capture.queueConversationAsrDelivery(id)).toBe(true);
  expect(capture.queueConversationAsrDelivery(id)).toBe(false);
  capture.failConversationAsrDelivery(id, "queue unavailable");
  expect(capture.retryConversationAsrDelivery(id)).toBe(entry!.text);
  expect(capture.retryConversationAsrDelivery(id)).toBeNull();
  expect(capture.queueConversationAsrDelivery("unknown")).toBe(false);
});
