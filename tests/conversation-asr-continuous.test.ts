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
let selectedTransport: "http" | "qwen-realtime" = "http";
let realtimeSessionId = "";
let realtimeSilenceTimeoutMs = 1_500;
let emulateServerVad = false;
let serverSilenceMs = 0;
let onRealtimeEvent:
  | ((event: {
      type: string;
      sessionId: string;
      utteranceId?: string;
      text?: string;
      language?: string | null;
    }) => void)
  | null = null;
const livePackets: Array<{ utteranceId: string; bytes: number }> = [];
let reportNativeStatus: ((status: { aecActive: boolean; playbackActive: boolean }) => void) | null =
  null;
const auditEvents: Array<{ eventName: string; correlationId?: string | null }> = [];
const recognitions: Array<{ utteranceId: string; kind: string }> = [];

mock.module("../src/lib/auditRuntime", () => ({
  recordConversationCaptureAuditEvent: (event: {
    eventName: string;
    correlationId?: string | null;
  }) => {
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
mock.module("../src/lib/qwenRealtimeAsr", () => ({
  conversationAsrTransport: async () => selectedTransport,
  startQwenAsrSession: async (
    sessionId: string,
    onEvent: typeof onRealtimeEvent,
    silenceTimeoutMs = 1_500,
  ) => {
    realtimeSilenceTimeoutMs = silenceTimeoutMs;
    serverSilenceMs = 0;
    realtimeSessionId = sessionId;
    onRealtimeEvent = onEvent;
  },
  appendQwenAsrAudio: async (_sessionId: string, utteranceId: string, packet: Uint8Array) => {
    livePackets.push({ utteranceId, bytes: packet.length });
    if (emulateServerVad) {
      serverSilenceMs = packet.every((byte) => byte === 0) ? serverSilenceMs + 100 : 0;
      if (serverSilenceMs === realtimeSilenceTimeoutMs) {
        onRealtimeEvent?.({
          type: "final",
          sessionId: realtimeSessionId,
          utteranceId,
          text: "無音で確定",
          language: "ja",
        });
      }
    }
  },
  stopQwenAsrSession: async () => {
    onRealtimeEvent?.({ type: "stopped", sessionId: realtimeSessionId });
    onRealtimeEvent = null;
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

  feed(0, 14);
  expect(capture.conversationAsrSnapshot().entries).toHaveLength(0);
  feed(0, 1);
  await tick();
  expect(capture.conversationAsrSnapshot().entries[0]?.status).toBe("completed");
  const utteranceId = capture.conversationAsrSnapshot().entries[0]!.id;
  expect(
    recognitions.some(
      (request) => request.utteranceId === utteranceId && request.kind === "partial",
    ),
  ).toBe(true);
  expect(
    recognitions.some((request) => request.utteranceId === utteranceId && request.kind === "final"),
  ).toBe(true);
  expect(
    auditEvents.some(
      (event) =>
        event.eventName === "conversation-asr-utterance-finalized" &&
        event.correlationId === utteranceId,
    ),
  ).toBe(true);
  expect(capture.conversationAsrSnapshot().phase).toBe("recording");
  const firstId = capture.conversationAsrSnapshot().entries[0]!.id;
  expect(capture.queueConversationAsrDelivery(firstId)).toBe(true);
  expect(capture.queueConversationAsrDelivery(firstId)).toBe(false);
  capture.failConversationAsrDelivery(firstId, "queue unavailable");
  expect(capture.conversationAsrSnapshot().entries[0]?.status).toBe("failed");
  expect(capture.retryConversationAsrDelivery(firstId)).toBe(
    capture.conversationAsrSnapshot().entries[0]?.text,
  );
  expect(capture.retryConversationAsrDelivery(firstId)).toBeNull();
  expect(capture.conversationAsrSnapshot().entries[0]?.deliveryQueued).toBe(true);

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

test("Qwen live ASR streams packets and only delivers final text after 1.5 seconds of silence", async () => {
  selectedTransport = "qwen-realtime";
  const priorRecognitions = recognitions.length;
  const priorEntries = capture.conversationAsrSnapshot().entries.length;
  await capture.startConversationAsr("default", true, "medium", 1_500);
  for (let index = 0; index < 4; index += 1) {
    feed(0.1, 5);
    await tick();
  }
  expect(livePackets.length).toBeGreaterThan(0);
  expect(livePackets.every((packet) => packet.bytes === 3_200)).toBe(true);
  expect(recognitions).toHaveLength(priorRecognitions);
  const id = livePackets.at(-1)!.utteranceId;
  onRealtimeEvent?.({
    type: "partial",
    sessionId: realtimeSessionId,
    utteranceId: id,
    text: "途中",
  });
  expect(capture.conversationAsrSnapshot().interimText).toBe("途中");
  feed(0, 7);
  await tick();
  feed(0, 7);
  await tick();
  expect(capture.conversationAsrSnapshot().entries).toHaveLength(priorEntries);
  feed(0, 1);
  expect(capture.conversationAsrSnapshot().entries[0]?.status).toBe("queued");
  onRealtimeEvent?.({
    type: "final",
    sessionId: realtimeSessionId,
    utteranceId: id,
    text: "確定",
    language: "ja",
  });
  expect(capture.conversationAsrSnapshot().entries[0]?.text).toBe("確定");
  expect(capture.conversationAsrSnapshot().entries[0]?.status).toBe("completed");
  expect(recognitions).toHaveLength(priorRecognitions);
  await capture.stopConversationAsr();
  selectedTransport = "http";
});

test("an early Qwen final waits for the local silence boundary", async () => {
  selectedTransport = "qwen-realtime";
  const before = capture.conversationAsrSnapshot().entries.length;
  await capture.startConversationAsr("default", true, "medium", 1_500);
  for (let index = 0; index < 4; index += 1) {
    feed(0.1, 5);
    await tick();
  }
  const id = livePackets.at(-1)!.utteranceId;
  onRealtimeEvent?.({
    type: "final",
    sessionId: realtimeSessionId,
    utteranceId: id,
    text: "先行確定",
    language: "ja",
  });
  expect(capture.conversationAsrSnapshot().entries).toHaveLength(before);
  feed(0, 7);
  await tick();
  feed(0, 7);
  await tick();
  expect(capture.conversationAsrSnapshot().entries).toHaveLength(before);
  feed(0, 1);
  expect(capture.conversationAsrSnapshot().entries[0]?.status).toBe("completed");
  expect(capture.conversationAsrSnapshot().entries[0]?.text).toBe("先行確定");
  await capture.stopConversationAsr();
  selectedTransport = "http";
});

test("Qwen segments completed before local silence are combined once", async () => {
  selectedTransport = "qwen-realtime";
  const before = capture.conversationAsrSnapshot().entries.length;
  await capture.startConversationAsr("default", true, "medium", 1_500);
  for (let index = 0; index < 4; index += 1) {
    feed(0.1, 5);
    await tick();
  }
  const id = livePackets.at(-1)!.utteranceId;
  for (const text of ["前", "半"]) {
    onRealtimeEvent?.({ type: "final", sessionId: realtimeSessionId, utteranceId: id, text });
  }
  feed(0, 7);
  await tick();
  feed(0, 7);
  await tick();
  feed(0, 1);
  expect(capture.conversationAsrSnapshot().entries).toHaveLength(before + 1);
  expect(capture.conversationAsrSnapshot().entries[0]?.text).toBe("前半");
  onRealtimeEvent?.({ type: "final", sessionId: realtimeSessionId, utteranceId: id, text: "重複" });
  expect(capture.conversationAsrSnapshot().entries[0]?.text).toBe("前半");
  await capture.stopConversationAsr();
  selectedTransport = "http";
});

test("Qwen waits for a later segment final before delivering the full utterance", async () => {
  selectedTransport = "qwen-realtime";
  await capture.startConversationAsr("default", true, "medium", 1_500);
  for (let index = 0; index < 4; index += 1) {
    feed(0.1, 5);
    await tick();
  }
  const id = livePackets.at(-1)!.utteranceId;
  onRealtimeEvent?.({ type: "final", sessionId: realtimeSessionId, utteranceId: id, text: "前半" });
  onRealtimeEvent?.({ type: "speechStarted", sessionId: realtimeSessionId, utteranceId: id });
  onRealtimeEvent?.({ type: "partial", sessionId: realtimeSessionId, utteranceId: id, text: "後" });
  expect(capture.conversationAsrSnapshot().interimText).toBe("前半後");
  feed(0, 7);
  await tick();
  feed(0, 7);
  await tick();
  feed(0, 1);
  expect(capture.conversationAsrSnapshot().entries[0]?.status).toBe("queued");
  onRealtimeEvent?.({ type: "final", sessionId: realtimeSessionId, utteranceId: id, text: "後半" });
  expect(capture.conversationAsrSnapshot().entries[0]?.text).toBe("前半後半");
  expect(capture.conversationAsrSnapshot().entries[0]?.status).toBe("completed");
  await capture.stopConversationAsr();
  selectedTransport = "http";
});

test("Qwen playback gating waits for the configured silence instead of delivering immediately", async () => {
  selectedTransport = "qwen-realtime";
  const before = capture.conversationAsrSnapshot().entries.length;
  await capture.startConversationAsr("default", true, "medium", 80);
  feed(0.1, 5);
  await tick();
  const id = livePackets.at(-1)!.utteranceId;
  onRealtimeEvent?.({
    type: "final",
    sessionId: realtimeSessionId,
    utteranceId: id,
    text: "再生前の発話",
    language: "ja",
  });
  capture.setConversationAsrPlaybackActive(true);
  expect(capture.conversationAsrSnapshot().entries).toHaveLength(before);
  await new Promise((resolve) => setTimeout(resolve, 100));
  expect(capture.conversationAsrSnapshot().entries[0]?.text).toBe("再生前の発話");
  capture.setConversationAsrPlaybackActive(false);
  await capture.stopConversationAsr();
  selectedTransport = "http";
});

test("Qwen session stop fails a turn without a final transcript", async () => {
  selectedTransport = "qwen-realtime";
  await capture.startConversationAsr("default", true, "medium", 1_500);
  for (let index = 0; index < 4; index += 1) {
    feed(0.1, 5);
    await tick();
  }
  feed(0, 7);
  await tick();
  feed(0, 4);
  await tick();
  feed(0, 4);
  expect(capture.conversationAsrSnapshot().entries[0]?.status).toBe("queued");
  await capture.stopConversationAsr();
  expect(capture.conversationAsrSnapshot().entries[0]?.status).toBe("failed");
  selectedTransport = "http";
});

test("an unexpected Qwen disconnect stops capture instead of silently dropping new speech", async () => {
  selectedTransport = "qwen-realtime";
  await capture.startConversationAsr("default", true, "medium", 1_500);
  onRealtimeEvent?.({ type: "stopped", sessionId: realtimeSessionId });
  await tick();
  expect(capture.conversationAsrSnapshot().phase).toBe("idle");
  expect(capture.conversationAsrSnapshot().error).toContain("Qwen ASR接続が終了しました");
  selectedTransport = "http";
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

test("final recognition starts without waiting for a delayed partial result", async () => {
  const previousEntries = capture.conversationAsrSnapshot().entries.length;
  await capture.startConversationAsr("default", true);
  holdNextRecognition = true;
  feed(0.1, 20);
  await tick();
  expect(releaseRecognition).not.toBeNull();
  feed(0, 15);
  expect(capture.conversationAsrSnapshot().entries).toHaveLength(previousEntries + 1);
  expect(capture.conversationAsrSnapshot().phase).toBe("recording");
  await tick();
  const finalId = capture.conversationAsrSnapshot().entries[0]!.id;
  expect(
    recognitions.some((request) => request.utteranceId === finalId && request.kind === "final"),
  ).toBe(true);
  expect(capture.conversationAsrSnapshot().entries[0]?.status).toBe("completed");
  releaseRecognition?.();
  await tick();
  const finalized = capture.conversationAsrSnapshot().entries[0]!;
  expect(finalized.status).toBe("completed");
  expect(
    auditEvents.some(
      (event) =>
        event.eventName === "conversation-asr-partial-ignored" &&
        event.correlationId === finalized.id,
    ),
  ).toBe(true);
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

for (const silenceMs of [800, 1_500, 3_000]) {
  test(`Qwen finalizes with the configured ${silenceMs}ms silence without another utterance`, async () => {
    selectedTransport = "qwen-realtime";
    emulateServerVad = true;
    const before = capture.conversationAsrSnapshot().entries.length;
    try {
      await capture.startConversationAsr("default", true, "medium", silenceMs);
      expect(realtimeSilenceTimeoutMs).toBe(silenceMs);
      feed(0.1, 5);
      await tick();
      for (let i = 0; i < silenceMs / 100; i += 1) {
        feed(0, 1);
        await tick();
      }
      const entries = capture.conversationAsrSnapshot().entries;
      expect(entries).toHaveLength(before + 1);
      expect(entries[0]?.status).toBe("completed");
      expect(entries[0]?.text).toBe("無音で確定");
      expect(capture.conversationAsrSnapshot().phase).toBe("recording");
    } finally {
      await capture.stopConversationAsr();
      selectedTransport = "http";
      emulateServerVad = false;
    }
  });
}
