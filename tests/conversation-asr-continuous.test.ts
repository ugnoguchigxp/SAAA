import { expect, mock, test } from "bun:test";

let onFrame: ((frame: Float32Array) => void) | null = null;
const audio = new Map<string, Float32Array>();
let uploads = 0;
let releases = 0;
let holdNextRecognition = false;
let releaseRecognition: (() => void) | null = null;
let nativeAvailable = false;
let nativeAecActive = true;
let nativePlaybackActive = false;
let nativeStartFails = false;
let macosMajor = 0;
let browserEchoCancellation: boolean | null = null;
let reportNativeStatus: ((status: { aecActive: boolean; playbackActive: boolean }) => void) | null = null;
const auditEvents: Array<{ eventName: string; correlationId?: string | null }> = [];
const recognitions: Array<{ utteranceId: string; kind: string }> = [];

mock.module("../src/lib/auditRuntime", () => ({
  recordConversationCaptureAuditEvent: (event: { eventName: string; correlationId?: string | null }) => {
    auditEvents.push(event);
  },
}));

mock.module("../src/lib/browserVoiceCapture", () => ({
  startBrowserVoiceCapture: async (
    handler: (frame: Float32Array) => void,
    _onEnded: (reason: string) => void,
    _inputDeviceId: string,
    echoCancellation: boolean,
  ) => {
    browserEchoCancellation = echoCancellation;
    onFrame = handler;
    return {
      stop: async () => {
        onFrame = null;
      },
    };
  },
}));
mock.module("../src/lib/audioBackend", () => ({
  audioBackendStatus: async () => ({ available: nativeAvailable, macosMajor }),
  nativeCapturePreferred: (status: { available: boolean }) => status.available,
  startNativeVoiceCapture: async (
    handler: (frame: Float32Array) => void,
    _onEnded: (reason: string) => void,
    onStatus: (status: { aecActive: boolean; playbackActive: boolean }) => void,
  ) => {
    if (nativeStartFails) throw new Error("native capture unavailable");
    onFrame = handler;
    reportNativeStatus = onStatus;
    return { available: true, aecActive: nativeAecActive, playbackActive: nativePlaybackActive };
  },
  stopNativeVoiceCapture: async () => {
    onFrame = null;
    reportNativeStatus = null;
  },
}));
mock.module("../src/lib/audioIpc", () => ({
  stageAudioUpload: async (samples: Float32Array) => {
    const id = String(++uploads);
    audio.set(id, samples.slice());
    return id;
  },
}));
mock.module("../src/lib/runtime", () => ({
  transcribeConversationAudio: async (id: string, utteranceId: string, kind: string) => {
    recognitions.push({ utteranceId, kind });
    if (holdNextRecognition) {
      holdNextRecognition = false;
      await new Promise<void>((resolve) => {
        releaseRecognition = resolve;
      });
    }
    return {
      text: `認識${audio.get(id)?.length ?? 0}`,
      language: "ja",
      providerLabel: "fixture ASR",
    };
  },
  releaseConversationAsrSession: async () => {
    releases += 1;
  },
}));

const capture = await import("../src/lib/conversationAsrCapture");
const tick = () => new Promise((resolve) => setTimeout(resolve, 20));
const feed = (value: number, frames: number) => {
  for (let index = 0; index < frames; index += 1) {
    const frame = new Float32Array(1_600);
    for (let sample = 0; sample < frame.length; sample += 1)
      frame[sample] = sample % 2 ? value : -value;
    onFrame?.(frame);
  }
};

test("recognizes partial text during speech, finalizes on silence, and keeps listening", async () => {
  await capture.startConversationAsr("default", true, "medium", 1_500);
  expect(capture.conversationAsrSnapshot().phase).toBe("recording");

  feed(0.1, 20);
  await tick();
  expect(capture.conversationAsrSnapshot().interimText.startsWith("認識")).toBe(true);
  expect(capture.conversationAsrSnapshot().entries).toHaveLength(0);

  feed(0, 15);
  await tick();
  expect(capture.conversationAsrSnapshot().entries[0]?.status).toBe("completed");
  const utteranceId = capture.conversationAsrSnapshot().entries[0]!.id;
  expect(recognitions.some((request) => request.utteranceId === utteranceId && request.kind === "partial")).toBe(true);
  expect(recognitions.some((request) => request.utteranceId === utteranceId && request.kind === "final")).toBe(true);
  expect(auditEvents.some((event) => event.eventName === "conversation-asr-utterance-finalized" && event.correlationId === utteranceId)).toBe(true);
  expect(capture.conversationAsrSnapshot().phase).toBe("recording");
  const firstId = capture.conversationAsrSnapshot().entries[0]!.id;
  expect(capture.queueConversationAsrDelivery(firstId)).toBe(true);
  expect(capture.queueConversationAsrDelivery(firstId)).toBe(false);

  feed(0.1, 20);
  feed(0, 15);
  await tick();
  expect(capture.conversationAsrSnapshot().entries).toHaveLength(2);
  expect(
    capture.conversationAsrSnapshot().entries.every((entry) => entry.status === "completed"),
  ).toBe(true);

  await capture.stopConversationAsr();
  expect(capture.conversationAsrSnapshot().phase).toBe("idle");
  expect(releases).toBe(1);
});

test("does not turn playback audio into a new utterance", async () => {
  const previousEntries = capture.conversationAsrSnapshot().entries.length;
  const previousUploads = uploads;
  await capture.startConversationAsr("default", true);
  capture.setConversationAsrPlaybackActive(true);
  feed(0.1, 40);
  await tick();
  expect(capture.conversationAsrSnapshot().entries).toHaveLength(previousEntries);
  expect(uploads).toBe(previousUploads);
  capture.setConversationAsrPlaybackActive(false);
  feed(0.1, 20);
  feed(0, 15);
  await tick();
  expect(capture.conversationAsrSnapshot().entries).toHaveLength(previousEntries + 1);
  await capture.stopConversationAsr();
});

test("delayed partial recognition cannot overwrite a finalized utterance", async () => {
  const previousEntries = capture.conversationAsrSnapshot().entries.length;
  await capture.startConversationAsr("default", true);
  holdNextRecognition = true;
  feed(0.1, 20);
  await tick();
  expect(releaseRecognition).not.toBeNull();
  feed(0, 15);
  expect(capture.conversationAsrSnapshot().entries).toHaveLength(previousEntries + 1);
  expect(capture.conversationAsrSnapshot().phase).toBe("recording");
  releaseRecognition?.();
  await tick();
  const finalized = capture.conversationAsrSnapshot().entries[0]!;
  expect(finalized.status).toBe("completed");
  expect(auditEvents.some((event) => event.eventName === "conversation-asr-partial-ignored" && event.correlationId === finalized.id)).toBe(true);
  expect(capture.conversationAsrSnapshot().interimText).toBe("");
  await capture.stopConversationAsr();
});

test("uses native capture when VoiceProcessing is available", async () => {
  const previousEntries = capture.conversationAsrSnapshot().entries.length;
  nativeAvailable = true;
  await capture.startConversationAsr("default", true);
  expect(capture.conversationAsrSnapshot().phase).toBe("recording");
  feed(0.1, 20);
  feed(0, 15);
  await tick();
  expect(capture.conversationAsrSnapshot().entries).toHaveLength(previousEntries + 1);
  await capture.stopConversationAsr();
  expect(capture.conversationAsrSnapshot().phase).toBe("idle");
  nativeAvailable = false;
});

test("accepts speech during playback only when native AEC is active", async () => {
  nativeAvailable = true;
  nativeAecActive = true;
  await capture.startConversationAsr("default", true);
  const before = capture.conversationAsrSnapshot().entries.length;
  capture.setConversationAsrPlaybackActive(true);
  expect(capture.conversationAsrSnapshot().playbackLimited).toBe(true);
  nativePlaybackActive = true;
  reportNativeStatus?.({ aecActive: true, playbackActive: true });
  expect(capture.conversationAsrSnapshot().playbackLimited).toBe(false);
  feed(0.1, 20);
  feed(0, 15);
  await tick();
  expect(capture.conversationAsrSnapshot().entries).toHaveLength(before + 1);
  feed(0.1, 4);
  nativePlaybackActive = false;
  reportNativeStatus?.({ aecActive: true, playbackActive: false });
  expect(capture.conversationAsrSnapshot().playbackLimited).toBe(true);
  expect(capture.conversationAsrSnapshot().entries).toHaveLength(before + 2);
  feed(0.1, 20);
  expect(capture.conversationAsrSnapshot().entries).toHaveLength(before + 2);
  capture.setConversationAsrPlaybackActive(false);
  await capture.stopConversationAsr();

  nativeAecActive = false;
  await capture.startConversationAsr("default", true);
  const after = capture.conversationAsrSnapshot().entries.length;
  capture.setConversationAsrPlaybackActive(true);
  expect(capture.conversationAsrSnapshot().playbackLimited).toBe(true);
  feed(0.1, 20);
  feed(0, 15);
  await tick();
  expect(capture.conversationAsrSnapshot().entries).toHaveLength(after);
  capture.setConversationAsrPlaybackActive(false);
  await capture.stopConversationAsr();
  nativeAecActive = true;
  nativeAvailable = false;
});

test("macOS native failure falls back without WebView echo processing", async () => {
  nativeAvailable = true;
  nativeStartFails = true;
  macosMajor = 14;
  browserEchoCancellation = null;
  await capture.startConversationAsr("default", true);
  expect(capture.conversationAsrSnapshot().phase).toBe("recording");
  expect(browserEchoCancellation).toBe(false);
  capture.setConversationAsrPlaybackActive(true);
  expect(capture.conversationAsrSnapshot().playbackLimited).toBe(true);
  capture.setConversationAsrPlaybackActive(false);
  await capture.stopConversationAsr();
  nativeStartFails = false;
  nativeAvailable = false;
  macosMajor = 0;
});
