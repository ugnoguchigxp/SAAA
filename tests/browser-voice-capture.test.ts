import { afterEach, expect, test } from "bun:test";
import { currentAudioCaptureOwner } from "../src/lib/audioCaptureCoordinator";
import { startBrowserVoiceCapture } from "../src/lib/browserVoiceCapture";

const originalNavigator = Object.getOwnPropertyDescriptor(globalThis, "navigator");
const originalAudioContext = Object.getOwnPropertyDescriptor(globalThis, "AudioContext");
const originalAudioWorkletNode = Object.getOwnPropertyDescriptor(globalThis, "AudioWorkletNode");

afterEach(() => {
  for (const [name, original] of [
    ["navigator", originalNavigator],
    ["AudioContext", originalAudioContext],
    ["AudioWorkletNode", originalAudioWorkletNode],
  ] as const) {
    if (original) Object.defineProperty(globalThis, name, original);
    else Reflect.deleteProperty(globalThis, name);
  }
});

test("browser capture uses the selected microphone and flushes its last frame", async () => {
  let constraints: MediaStreamConstraints | undefined;
  let trackStopped = false;
  let contextClosed = false;
  let loadedProcessor = "";
  const track = {
    stop: () => { trackStopped = true; },
    addEventListener: () => undefined,
    removeEventListener: () => undefined,
  };
  const stream = {
    getTracks: () => [track],
    getAudioTracks: () => [track],
  } as unknown as MediaStream;
  Object.defineProperty(globalThis, "navigator", {
    configurable: true,
    value: {
      mediaDevices: {
        getUserMedia: async (next: MediaStreamConstraints) => {
          constraints = next;
          return stream;
        },
      },
    },
  });
  class FakeContext {
    sampleRate = 48_000;
    state = "running";
    destination = {};
    audioWorklet = { addModule: async (url: string) => { loadedProcessor = url; } };
    createMediaStreamSource() {
      return { connect: () => undefined, disconnect: () => undefined };
    }
    async close() { contextClosed = true; }
  }
  class FakeNode {
    static current: FakeNode;
    port = {
      onmessage: null as ((event: MessageEvent<Float32Array | { type: "flushed" }>) => void) | null,
      postMessage: (message: { type: string }) => {
        if (message.type === "flush") {
          this.port.onmessage?.({ data: new Float32Array(2_400).fill(0.5) } as MessageEvent<Float32Array>);
          this.port.onmessage?.({ data: { type: "flushed" } } as MessageEvent<{ type: "flushed" }>);
        }
      },
    };
    constructor() { FakeNode.current = this; }
    connect() { return undefined; }
    disconnect() { return undefined; }
  }
  Object.defineProperty(globalThis, "AudioContext", { configurable: true, value: FakeContext });
  Object.defineProperty(globalThis, "AudioWorkletNode", { configurable: true, value: FakeNode });

  const lengths: number[] = [];
  const capture = await startBrowserVoiceCapture(
    (frame) => lengths.push(frame.length),
    () => undefined,
    "usb-mic",
    false,
  );
  FakeNode.current.port.onmessage?.({ data: new Float32Array(4_800).fill(0.25) } as MessageEvent<Float32Array>);
  await capture.stop();

  expect((constraints?.audio as MediaTrackConstraints).deviceId).toEqual({ exact: "usb-mic" });
  expect((constraints?.audio as MediaTrackConstraints).echoCancellation).toBe(false);
  expect(loadedProcessor).toBe("/audio/voice-capture-processor.js");
  expect(lengths).toEqual([1_600, 800]);
  expect(trackStopped).toBe(true);
  expect(contextClosed).toBe(true);
  expect(currentAudioCaptureOwner()).toBeNull();
});
