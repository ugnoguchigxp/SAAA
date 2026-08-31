import { describe, expect, test } from "bun:test";
import { voiceSegmentCommitReason } from "../src/features/voice/voiceSegmentBoundary";

describe("voice segment boundary", () => {
  test("commits silent capture at the hard duration before backend capacity is exceeded", () => {
    const silence = { hasSpeech: false, shouldFinalize: false, rms: 0 };
    expect(voiceSegmentCommitReason(silence, 299)).toBeNull();
    expect(voiceSegmentCommitReason(silence, 300)).toBe("max-duration");
  });

  test("uses the VAD boundary for a voiced utterance", () => {
    expect(voiceSegmentCommitReason(
      { hasSpeech: true, shouldFinalize: true, rms: 0 },
      20,
    )).toBe("silence");
  });
});
