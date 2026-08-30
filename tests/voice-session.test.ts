import { describe, expect, test } from "bun:test";
import { initialVoiceSession, transitionVoiceSession, voiceCaptureState, voiceSessionBusy, voiceSessionProcessing } from "../src/lib/voiceSession";

describe("voice session state machine", () => {
  test("serializes finalize requests without dropping a stop", () => {
    const finalizing = transitionVoiceSession(initialVoiceSession, { type: "finalizeRequested", mode: "continue" });
    const queued = transitionVoiceSession(finalizing, { type: "finalizeRequested", mode: "stop" });
    expect(queued.pendingFinalize).toBe("stop");
    expect(transitionVoiceSession(queued, { type: "finalizeRequested", mode: "continue" }).pendingFinalize).toBe("stop");
  });

  test("ignores stale transcription completion", () => {
    const running = transitionVoiceSession(initialVoiceSession, { type: "transcriptionStarted", runId: "voice_1" });
    expect(transitionVoiceSession(running, { type: "transcriptionFinished", runId: "old" })).toEqual(running);
    expect(transitionVoiceSession(running, { type: "transcriptionFinished", runId: "voice_1" }).transcriptionRunId).toBeNull();
  });

  test("keeps cancellation active until the matching transcription finishes", () => {
    const running = transitionVoiceSession(initialVoiceSession, { type: "transcriptionStarted", runId: "voice_1" });
    const cancelling = transitionVoiceSession(running, { type: "transcriptionCancelRequested" });
    expect(cancelling.cancellationRequested).toBe(true);
    expect(transitionVoiceSession(cancelling, { type: "transcriptionFinished", runId: "old" }).cancellationRequested).toBe(true);
    expect(transitionVoiceSession(cancelling, { type: "transcriptionFinished", runId: "voice_1" }).cancellationRequested).toBe(false);
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
    const transcribing = transitionVoiceSession(recording, { type: "transcriptionStarted", runId: "voice_1" });
    expect(voiceSessionProcessing(transcribing)).toBe(true);
  });
});
