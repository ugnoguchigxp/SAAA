import { afterEach, describe, expect, test } from "bun:test";
import { invokeImpl, resetTauriCoreMock } from "./tauriCoreMock";
const { startNativeVoiceCapture, stopNativeVoiceCapture } = await import("../src/lib/audioBackend");

describe("native capture monitoring", () => {
  afterEach(async () => {
    await stopNativeVoiceCapture();
    resetTauriCoreMock();
  });
  test("reports an unexpected native capture stop once", async () => {
    const ended: string[] = [];
    invokeImpl.handler = async (command) => {
      if (command === "audio_backend_status") return { captureActive: false };
      if (command === "start_native_voice_capture") return { captureActive: true };
      return undefined;
    };
    await startNativeVoiceCapture(
      () => undefined,
      (reason) => ended.push(reason),
    );
    await Bun.sleep(650);
    expect(ended).toEqual(["VoiceProcessing capture stopped unexpectedly"]);
    await Bun.sleep(550);
    expect(ended).toHaveLength(1);
  });

  test("does not report an intentional native capture stop", async () => {
    const ended: string[] = [];
    invokeImpl.handler = async (command) => {
      if (command === "start_native_voice_capture") return { captureActive: true };
      if (command === "audio_backend_status") return { captureActive: false };
      return undefined;
    };
    await startNativeVoiceCapture(
      () => undefined,
      (reason) => ended.push(reason),
    );
    await stopNativeVoiceCapture();
    await Bun.sleep(550);
    expect(ended).toEqual([]);
  });

  test("stops monitoring after repeated backend status failures", async () => {
    const ended: string[] = [];
    invokeImpl.handler = async (command) => {
      if (command === "start_native_voice_capture") return { captureActive: true };
      if (command === "audio_backend_status") throw new Error("IPC unavailable");
      return undefined;
    };
    await startNativeVoiceCapture(
      () => undefined,
      (reason) => ended.push(reason),
    );
    await Bun.sleep(1_650);
    expect(ended).toEqual(["VoiceProcessing capture status is unavailable"]);
  });
});
