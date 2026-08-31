import { describe, expect, test } from "bun:test";
import { initialVoiceAsrProjection, projectVoiceAsrEvent } from "../src/features/voice/voiceAsrProjection";

describe("voice ASR projection", () => {
  const ready = projectVoiceAsrEvent(initialVoiceAsrProjection, {
    type: "ready", sessionId: "s", currentUtteranceId: "u", protocol: "native", scope: "all-speakers",
  });
  const latest = projectVoiceAsrEvent(ready, {
    type: "partial", sessionId: "s", utteranceId: "u", revision: 2,
    startMs: 0, endMs: 100, stableText: "hello", unstableText: " world", language: null,
  });

  test("replaces unstable text and ignores stale or foreign revisions", () => {
    expect(projectVoiceAsrEvent(latest, {
      type: "partial", sessionId: "s", utteranceId: "u", revision: 1,
      startMs: 0, endMs: 100, stableText: "bad", unstableText: "", language: null,
    })).toEqual(latest);
    expect(projectVoiceAsrEvent(latest, {
      type: "partial", sessionId: "other", utteranceId: "u", revision: 3,
      startMs: 0, endMs: 100, stableText: "bad", unstableText: "", language: null,
    })).toEqual(latest);
    const replacement = projectVoiceAsrEvent(latest, {
      type: "partial", sessionId: "s", utteranceId: "u", revision: 3,
      startMs: 0, endMs: 200, stableText: "hello ", unstableText: "again", language: "en",
    });
    expect(replacement.unstableText).toBe("again");
  });

  test("projects final, discard, degradation, and stop without accepting duplicates", () => {
    expect(projectVoiceAsrEvent(latest, {
      type: "final", sessionId: "s", utteranceId: "u", revision: 2,
      startMs: 0, endMs: 100, text: "duplicate", language: null,
    })).toEqual(latest);
    const final = projectVoiceAsrEvent(latest, {
      type: "final", sessionId: "s", utteranceId: "u", revision: 3,
      startMs: 0, endMs: 100, text: "hello world", language: "en",
    });
    expect(final.finalText).toBe("hello world");
    const discarded = projectVoiceAsrEvent(latest, {
      type: "utteranceDiscarded", sessionId: "s", utteranceId: "u", reason: "no-speech",
    });
    expect(discarded.stableText + discarded.unstableText).toBe("");
    const degraded = projectVoiceAsrEvent(latest, {
      type: "degraded", sessionId: "s", from: "native", to: "batch-agreement", reasonCode: "asr-stream-timeout",
    });
    expect(degraded).toMatchObject({ protocol: "batch-agreement", status: "degraded" });
    expect(projectVoiceAsrEvent(degraded, { type: "stopped", sessionId: "s" })).toEqual(initialVoiceAsrProjection);
  });

  test("does not roll the current partial back when an older utterance final arrives", () => {
    const nextUtterance = projectVoiceAsrEvent(latest, {
      type: "partial", sessionId: "s", utteranceId: "u2", revision: 1,
      startMs: 100, endMs: 200, stableText: "next", unstableText: " words", language: "en",
    });
    const delayedFinal = projectVoiceAsrEvent(nextUtterance, {
      type: "final", sessionId: "s", utteranceId: "u", revision: 3,
      startMs: 0, endMs: 100, text: "hello world", language: "en",
    });
    expect(delayedFinal).toEqual(nextUtterance);
  });
});
