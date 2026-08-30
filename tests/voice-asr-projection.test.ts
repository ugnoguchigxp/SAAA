import { describe, expect, test } from "bun:test";
import { initialVoiceAsrProjection, projectVoiceAsrEvent } from "../src/features/voice/voiceAsrProjection";
describe("voice ASR projection", () => test("replaces unstable text and ignores stale revisions", () => {
 const ready = projectVoiceAsrEvent(initialVoiceAsrProjection, { type: "ready", sessionId: "s", currentUtteranceId: "u", protocol: "native", scope: "all-speakers" });
 const latest = projectVoiceAsrEvent(ready, { type: "partial", sessionId: "s", utteranceId: "u", revision: 2, startMs: 0, endMs: 100, stableText: "hello", unstableText: " world", language: null });
 expect(projectVoiceAsrEvent(latest, { type: "partial", sessionId: "s", utteranceId: "u", revision: 1, startMs: 0, endMs: 100, stableText: "bad", unstableText: "", language: null })).toEqual(latest);
 expect(projectVoiceAsrEvent(latest, { type: "final", sessionId: "s", utteranceId: "u", revision: 1, startMs: 0, endMs: 100, text: "bad", language: null })).toEqual(latest);
}));
