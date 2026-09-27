import { describe, expect, test } from "bun:test";
import { observeCaptureFrame } from "../src/features/voice/ambientWorkletVoiceCapture";

function speechFrame(): Float32Array {
  return new Float32Array(160).fill(0.2);
}

describe("barge-in hold", () => {
  test("fires once after sustained speech for the same run", () => {
    const interrupts: number[] = [];
    const since = { current: 0 };
    const fired = { current: null as string | null };
    const started = performance.now() - 1_000;
    const input = {
      activityDetector: {
        current: {
          observe: () => ({ hasSpeech: true, shouldFinalize: false, rms: 0.2 }),
        },
      },
      packetFrame: () => undefined,
      packetCount: () => 1,
      finishSegment: () => undefined,
      bargeInEnabled: true,
      speechIsPlaying: () => true,
      ttsStartedAtMs: () => started,
      speechRunId: () => "run-1",
      bargeInSpeechSince: since,
      bargeInFiredFor: fired,
      interruptSpeech: () => interrupts.push(1),
    };
    observeCaptureFrame({ ...input, frame: speechFrame() });
    expect(interrupts).toEqual([]);
    since.current = performance.now() - 250;
    observeCaptureFrame({ ...input, frame: speechFrame() });
    observeCaptureFrame({ ...input, frame: speechFrame() });
    expect(interrupts).toEqual([1]);
    expect(fired.current).toBe("run-1");
  });
});
