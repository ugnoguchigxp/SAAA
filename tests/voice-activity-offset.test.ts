import { expect, test } from "bun:test";
import { VoiceActivityDetector } from "../src/lib/voiceActivity";

test("voice activity ignores a steady microphone DC offset", () => {
  const detector = new VoiceActivityDetector({ sampleRate: 1_000 });
  expect(detector.observe(new Float32Array(1_000).fill(0.02)).hasSpeech).toBe(false);
});

test("voice activity reports when an utterance owns its detector snapshot", () => {
  const detector = new VoiceActivityDetector({ sampleRate: 1_000, requiredSpeechMs: 10 });
  expect(detector.hasDetectedSpeech()).toBe(false);
  detector.observe(Float32Array.from({ length: 10 }, (_, index) => index % 2 ? 0.05 : -0.05));
  expect(detector.hasDetectedSpeech()).toBe(true);
});
