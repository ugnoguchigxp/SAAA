import { describe, expect, test } from "bun:test";
import { VoiceActivityDetector } from "../src/lib/voiceActivity";

const sampleRate = 1_000;
const frame = (milliseconds: number, amplitude: number) => Float32Array.from({ length: milliseconds }, (_, index) => amplitude * (index % 2 === 0 ? 1 : -1));
describe("voice activity endpointing", () => {
  test("never finalizes from silence before speech", () => {
    const detector = new VoiceActivityDetector({ sampleRate });
    expect(detector.observe(frame(5_000, 0)).shouldFinalize).toBe(false);
    expect(detector.observe(frame(5_000, 0)).hasSpeech).toBe(false);
  });

  test("ignores a short click followed by silence", () => {
    const detector = new VoiceActivityDetector({ sampleRate });
    detector.observe(frame(100, 0.1));
    expect(detector.observe(frame(2_000, 0)).shouldFinalize).toBe(false);
    expect(detector.observe(frame(1_000, 0)).hasSpeech).toBe(false);
  });

  test("finalizes once after speech and the configured silence", () => {
    const detector = new VoiceActivityDetector({ sampleRate });
    expect(detector.observe(frame(300, 0.02)).hasSpeech).toBe(true);
    expect(detector.observe(frame(1_499, 0)).shouldFinalize).toBe(false);
    expect(detector.observe(frame(1, 0)).shouldFinalize).toBe(true);
    expect(detector.observe(frame(2_000, 0)).shouldFinalize).toBe(false);
  });

  test("resets the silence timer when speech resumes", () => {
    const detector = new VoiceActivityDetector({ sampleRate });
    detector.observe(frame(300, 0.02));
    detector.observe(frame(1_000, 0));
    detector.observe(frame(100, 0.02));
    expect(detector.observe(frame(1_499, 0)).shouldFinalize).toBe(false);
    expect(detector.observe(frame(1, 0)).shouldFinalize).toBe(true);
  });

  test("recognizes quiet but usable speech at the shared 0.008 RMS threshold", () => {
    const detector = new VoiceActivityDetector({ sampleRate });
    expect(detector.observe(frame(300, 0.009)).hasSpeech).toBe(true);
  });
});
