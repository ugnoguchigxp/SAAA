import { afterEach, describe, expect, test } from "bun:test";
import type { MutableRefObject } from "react";
import type { VoiceSettings } from "../src/lib/contracts";
import { currentAudioCaptureOwner } from "../src/lib/audioCaptureCoordinator";
import { attachAmbientVoiceCapture, resetVoiceActivityDetector } from "../src/features/voice/ambientVoiceCapture";

class FakeAudioWorkletNode {
  port = {
    onmessage: null as ((event: MessageEvent) => void) | null,
    postMessage: () => undefined,
  };
  connect() { return this; }
  disconnect() { return this; }
}

class FakeAudioContext {
  sampleRate = 16_000;
  state = "running";
  destination = {};
  audioWorklet = { addModule: async () => undefined };
  constructor(options?: { sampleRate?: number }) {
    if (options?.sampleRate) this.sampleRate = options.sampleRate;
  }
  createMediaStreamSource() {
    return { connect: () => this, disconnect: () => undefined };
  }
  close = async () => { this.state = "closed"; };
  resume = async () => { this.state = "running"; };
}

const stream = { getTracks: () => [{ stop: () => undefined }] } as unknown as MediaStream;

const settings: VoiceSettings = {
  listeningEnabled: true,
  inputDeviceId: "default",
  outputDeviceId: "default",
  vadSensitivity: "high",
  silenceTimeoutMs: 1_200,
  allowedLanguages: ["ja"],
  autoSpeak: true,
};

function ref<T>(value: T): MutableRefObject<T> {
  return { current: value };
}

function context(overrides: Record<string, unknown> = {}) {
  return {
    settings,
    disposed: ref(false),
    listeningEnabled: ref(true),
    meetingState: ref("idle" as const),
    captureAttempt: ref(0),
    stream: ref<MediaStream | null>(null),
    audioContext: ref<AudioContext | null>(null),
    source: ref<MediaStreamAudioSourceNode | null>(null),
    node: ref<AudioWorkletNode | null>(null),
    flushResolver: ref<(() => void) | null>(null),
    activityDetector: ref(null),
    captureLease: ref<(() => void) | null>(null),
    applyEvent: () => undefined,
    finishSegment: () => undefined,
    packetFrame: () => undefined,
    packetCount: () => 0,
    clearTranscript: () => undefined,
    ...overrides,
  };
}

function installCaptureGlobals() {
  const previous = {
    AudioContext: globalThis.AudioContext,
    AudioWorkletNode: globalThis.AudioWorkletNode,
    navigator: Object.getOwnPropertyDescriptor(globalThis, "navigator"),
    window: Object.getOwnPropertyDescriptor(globalThis, "window"),
  };
  (globalThis as { AudioContext: unknown }).AudioContext = FakeAudioContext;
  (globalThis as { AudioWorkletNode: unknown }).AudioWorkletNode = FakeAudioWorkletNode;
  Object.defineProperty(globalThis, "navigator", {
    configurable: true,
    value: {
      mediaDevices: {
        getUserMedia: async () => stream,
      },
    },
  });
  Object.defineProperty(globalThis, "window", {
    configurable: true,
    value: { isSecureContext: true },
  });
  return () => {
    (globalThis as { AudioContext: unknown }).AudioContext = previous.AudioContext;
    (globalThis as { AudioWorkletNode: unknown }).AudioWorkletNode = previous.AudioWorkletNode;
    if (previous.navigator) Object.defineProperty(globalThis, "navigator", previous.navigator);
    else Reflect.deleteProperty(globalThis, "navigator");
    if (previous.window) Object.defineProperty(globalThis, "window", previous.window);
    else Reflect.deleteProperty(globalThis, "window");
  };
}

describe("ambient voice capture", () => {
  let restore: (() => void) | null = null;
  let lease: (() => void) | null = null;

  afterEach(() => {
    lease?.();
    lease = null;
    restore?.();
    restore = null;
  });

  test("attaches a 16 kHz worklet and forwards PCM frames", async () => {
    restore = installCaptureGlobals();
    const frames: number[] = [];
    const target = context({
      packetFrame: (frame: Float32Array) => frames.push(frame.length),
    });
    await attachAmbientVoiceCapture(target as never);
    lease = target.captureLease.current;
    const node = target.node.current as unknown as FakeAudioWorkletNode;
    node.port.onmessage?.({ data: new Float32Array([0.1, 0.2]) } as MessageEvent);
    node.port.onmessage?.({ data: { type: "flushed" } } as MessageEvent);
    expect(frames).toEqual([2]);
    resetVoiceActivityDetector(target.activityDetector, { ...settings, vadSensitivity: "low" }, 16_000);
    expect(target.activityDetector.current).not.toBeNull();
  });

  test("returns immediately when capture is already owned or blocked", async () => {
    await attachAmbientVoiceCapture(context({ stream: ref(stream), captureLease: ref(() => undefined) }) as never);
    await attachAmbientVoiceCapture(context({ listeningEnabled: ref(false) }) as never);
    await attachAmbientVoiceCapture(context({ meetingState: ref("active") }) as never);
    expect(currentAudioCaptureOwner()).toBeNull();
  });
});
