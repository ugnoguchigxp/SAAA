import { describe, expect, test } from "bun:test";
import { projectLatestResponsePipeline } from "../src/features/audit/voicePipelineMonitor";
import { audit, recognized } from "./voice-pipeline-monitor-fixture";

describe("voice pipeline monitor", () => {
  test("a newer active ASR session replaces an older capture failure", () => {
    const snapshot = projectLatestResponsePipeline([
      audit(1, "capture-start-failed", {
        component: "voice-asr",
        sessionId: "old_session",
        outcome: "failure",
        failureCode: "larm-session-prepare-failed",
      }),
      audit(2, "asr-session-start-requested", {
        component: "voice-asr",
        sessionId: "current_session",
        phase: "request",
      }),
      audit(3, "asr-ready", {
        component: "voice-asr",
        sessionId: "current_session",
        outcome: "success",
      }),
      audit(4, "capture-started", {
        component: "microphone",
        sessionId: "current_session",
        outcome: "success",
      }),
      audit(5, "asr-utterance-discarded", {
        component: "voice-asr",
        sessionId: "current_session",
        outcome: "cancelled",
        failureCode: "target-speaker-empty",
      }),
    ]);

    expect(snapshot.anchor?.sequence).toBe(2);
    expect(snapshot.sessionId).toBe("current_session");
    expect(snapshot.diagnosis).toBe("asr-listening");
    expect(snapshot.stages.map((stage) => stage.state)).toEqual(["running", "idle", "idle"]);
    expect(snapshot.stages[0].failureCode).toBe("target-speaker-empty");
    expect(snapshot.relatedEvents.map((event) => event.sequence)).toEqual([2, 3, 4, 5]);
  });

  test("a newer stopped ASR session does not reveal an older failure again", () => {
    const snapshot = projectLatestResponsePipeline([
      audit(1, "capture-start-failed", {
        component: "voice-asr",
        sessionId: "old_session",
        outcome: "failure",
      }),
      audit(2, "asr-session-start-requested", {
        component: "voice-asr",
        sessionId: "current_session",
      }),
      audit(3, "asr-stop-finished", {
        component: "voice-asr",
        sessionId: "current_session",
        phase: "terminal",
        outcome: "success",
      }),
    ]);

    expect(snapshot.diagnosis).toBe("asr-stopped");
    expect(snapshot.stages[0].state).toBe("success");
    expect(snapshot.anchor?.sequence).toBe(2);
  });

});
