import { afterEach, describe, expect, test } from "bun:test";
import type { MutableRefObject } from "react";
import type { VoiceSettings } from "../src/lib/contracts";
import { invokeCalls, invokeImpl, resetTauriCoreMock } from "./tauriCoreMock";

const settings: VoiceSettings = {
  listeningEnabled: true,
  inputDeviceId: "default",
  outputDeviceId: "default",
  vadSensitivity: "medium",
  silenceTimeoutMs: 1_500,
  allowedLanguages: ["ja"],
  autoSpeak: true,
  aecEnabled: true,
  otherAudioDucking: "min",
  vpioOnBluetooth: false,
  bargeInEnabled: true,
};

function ref<T>(value: T): MutableRefObject<T> {
  return { current: value };
}

const { VoiceActivityDetector } = await import("../src/lib/voiceActivity");
const { tryStartNativeVoiceCapture } = await import(
  "../src/features/voice/ambientNativeVoiceCapture"
);

describe("native VoiceProcessing capture", () => {
  afterEach(() => resetTauriCoreMock());

  test("starts the native backend and forwards frames", async () => {
    const frames: number[] = [];
    const ended: string[] = [];
    invokeImpl.handler = async (command, args) => {
      if (command === "audio_backend_status") {
        return {
          available: true,
          reason: null,
          captureActive: false,
          playbackActive: false,
          aecActive: true,
          duckingLevel: "min",
          agcEnabled: false,
          outputTransport: "built-in",
          macosMajor: 26,
        };
      }
      if (command === "start_native_voice_capture") {
        const onFrame = (args as { onFrame?: { onmessage: ((event: unknown) => void) | null } })
          .onFrame;
        const pcm = new Float32Array([0.1, 0.2]);
        onFrame?.onmessage?.(pcm.buffer);
        onFrame?.onmessage?.({ type: "ended", reason: "airplay" });
      }
      return { available: true };
    };
    const started = await tryStartNativeVoiceCapture({
      settings,
      nativeCapture: ref(false),
      nativeStop: ref(null),
      activityDetector: ref(null),
      createDetector: (sampleRate) =>
        new VoiceActivityDetector({
          sampleRate,
          speechThresholdRms: 0.008,
          silenceTimeoutMs: 1_500,
        }),
      stale: () => false,
      handleFrame: (frame) => frames.push(frame.length),
      onEnded: (reason) => ended.push(reason),
      disposeOwnedCapture: async () => undefined,
      applyEvent: () => undefined,
      clearTranscript: () => undefined,
    });
    expect(started).toBe(true);
    expect(invokeCalls.some((call) => call.command === "start_native_voice_capture")).toBe(true);
    expect(frames).toEqual([2]);
    expect(ended).toEqual(["airplay"]);
  });

  test("falls back when the native backend is unavailable", async () => {
    invokeImpl.handler = async (command) => {
      if (command === "audio_backend_status") {
        return { available: false, reason: "macOS 13", macosMajor: 13 };
      }
      return command;
    };
    const started = await tryStartNativeVoiceCapture({
      settings,
      activityDetector: ref(null),
      createDetector: () =>
        new VoiceActivityDetector({
          sampleRate: 16_000,
          speechThresholdRms: 0.008,
          silenceTimeoutMs: 1_500,
        }),
      stale: () => false,
      handleFrame: () => undefined,
      disposeOwnedCapture: async () => undefined,
      applyEvent: () => undefined,
      clearTranscript: () => undefined,
    });
    expect(started).toBe(false);
    expect(invokeCalls.some((call) => call.command === "start_native_voice_capture")).toBe(false);
  });
});
