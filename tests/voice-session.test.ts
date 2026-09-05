import { describe, expect, test } from "bun:test";
import { initialVoiceSession, transitionVoiceSession, voiceCaptureState, voiceSessionBusy, voiceSessionProcessing } from "../src/lib/voiceSession";

describe("voice session state machine", () => {
  test("serializes finalize requests without dropping a stop", () => {
    const finalizing = transitionVoiceSession(initialVoiceSession, { type: "finalizeRequested", mode: "continue" });
    const queued = transitionVoiceSession(finalizing, { type: "finalizeRequested", mode: "stop" });
    expect(queued.pendingFinalize).toBe("stop");
    expect(transitionVoiceSession(queued, { type: "finalizeRequested", mode: "continue" }).pendingFinalize).toBe("stop");
  });

  test("derives public busy state from the single snapshot", () => {
    const recording = transitionVoiceSession(initialVoiceSession, { type: "captureStarted" });
    expect(voiceCaptureState(recording)).toBe("recording");
    expect(voiceSessionBusy(recording)).toBe(true);
    const suspended = transitionVoiceSession(recording, { type: "captureSuspended" });
    expect(voiceCaptureState(suspended)).toBe("idle");
    expect(voiceSessionBusy(suspended)).toBe(true);
    const starting = transitionVoiceSession(initialVoiceSession, { type: "captureStarting" });
    expect(transitionVoiceSession(starting, { type: "captureSuspended" }).capture).toBe("suspended");
  });

  test("does not treat ambient capture alone as turn processing", () => {
    const recording = transitionVoiceSession(initialVoiceSession, { type: "captureStarted" });
    expect(voiceSessionProcessing(recording)).toBe(false);
    const finalizing = transitionVoiceSession(recording, { type: "finalizeRequested", mode: "stop" });
    expect(voiceSessionProcessing(finalizing)).toBe(true);
    const detached = transitionVoiceSession(finalizing, { type: "captureDetached" });
    expect(voiceCaptureState(detached)).toBe("transcribing");
    expect(voiceSessionBusy(detached)).toBe(true);
    const completed = transitionVoiceSession(detached, { type: "finalizeCompleted" });
    expect(voiceCaptureState(completed)).toBe("idle");
    expect(voiceSessionBusy(completed)).toBe(false);
  });
});
