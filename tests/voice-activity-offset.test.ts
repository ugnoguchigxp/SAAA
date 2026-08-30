import { expect, test } from "bun:test";
import { VoiceActivityDetector } from "../src/lib/voiceActivity";

test("voice activity ignores a steady microphone DC offset", () => {
  const detector = new VoiceActivityDetector({ sampleRate: 1_000 });
  expect(detector.observe(new Float32Array(1_000).fill(0.02)).hasSpeech).toBe(false);
});
