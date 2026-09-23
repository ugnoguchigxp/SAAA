import { describe, expect, test } from "bun:test";
import { projectLatestResponsePipeline } from "../src/features/audit/voicePipelineMonitor";
import { audit, recognized } from "./voice-pipeline-monitor-fixture";

describe("voice pipeline monitor", () => {
  test("shows a reasoning request without calling it delegation", () => {
    const snapshot = projectLatestResponsePipeline([
      recognized,
      audit(2, "lfm-utterance-received", { subjectId: "utterance_1" }),
      audit(3, "lfm-requested-qwen-reasoning", {
        subjectId: "utterance_1",
        attributes: { reasoningRequestId: "lfm_reasoning_1" },
      }),
    ]);
    expect(snapshot.diagnosis).toBe("lfm-reasoning-requested");
    expect(snapshot.stages.find((stage) => stage.key === "qwen")?.state).toBe("waiting");
  });

  test("does not attach an older utterance's Qwen run to the latest LFM reply", () => {
    const snapshot = projectLatestResponsePipeline([
      recognized,
      audit(2, "lfm-utterance-received", { subjectId: "utterance_1" }),
      audit(3, "lfm-requested-qwen-reasoning", {
        subjectId: "utterance_1",
        attributes: { reasoningRequestId: "lfm_reasoning_1" },
      }),
      audit(4, "turn-requested", {
        causationId: "lfm_reasoning_1",
        runtimeRunId: "run_1",
      }),
      audit(5, "asr-final-received", {
        component: "voice-asr",
        subjectId: "utterance_2",
        sessionId: "voice_1",
      }),
      audit(6, "lfm-utterance-received", { subjectId: "utterance_2" }),
      audit(7, "lfm-replied-without-reasoning-request", { subjectId: "utterance_2" }),
    ]);

    expect(snapshot.utteranceId).toBe("utterance_2");
    expect(snapshot.runId).toBeNull();
    expect(snapshot.diagnosis).toBe("lfm-responded");
    expect(snapshot.stages.find((stage) => stage.key === "qwen")?.state).toBe("skipped");
  });

  test("shows an LLM run with no terminal response as running", () => {
    const snapshot = projectLatestResponsePipeline([
      recognized,
      audit(2, "voice-utterance-submitted", {
        subjectId: "utterance_1",
        sessionId: "voice_1",
        outcome: "success",
      }),
      audit(3, "turn-requested", {
        phase: "request",
        causationId: "utterance_1",
        correlationId: "run_1",
        runtimeRunId: "run_1",
        subjectId: "run_1",
        attributes: { inputOrigin: "voice", presentationMode: "visual-and-spoken" },
      }),
      audit(4, "runtime-event-started", {
        phase: "start",
        outcome: "success",
        correlationId: "run_1",
        runtimeRunId: "run_1",
      }),
    ]);

    expect(snapshot.diagnosis).toBe("llm-running");
    expect(snapshot.stages[1].state).toBe("running");
    expect(snapshot.runId).toBe("run_1");
    expect(snapshot.relatedEvents).toHaveLength(4);
  });

  test("surfaces the durable LLM failure code", () => {
    const snapshot = projectLatestResponsePipeline([
      recognized,
      audit(2, "turn-requested", {
        causationId: "utterance_1",
        correlationId: "run_1",
        runtimeRunId: "run_1",
        attributes: { inputOrigin: "voice", presentationMode: "visual-and-spoken" },
      }),
      audit(3, "runtime-run-finished", {
        phase: "terminal",
        outcome: "failure",
        correlationId: "run_1",
        runtimeRunId: "run_1",
        failureCode: "configuration-error",
      }),
      audit(4, "runtime-failed", {
        phase: "error",
        outcome: "failure",
        correlationId: "run_1",
        runtimeRunId: "run_1",
        failureCode: "context-scope-changed",
      }),
      audit(5, "speech-ended", {
        component: "tts",
        phase: "terminal",
        outcome: "success",
        correlationId: "run_1",
        runtimeRunId: "run_1",
      }),
      audit(6, "voice-delivery-finished", {
        phase: "terminal",
        outcome: "failure",
        sessionId: "voice_1",
        subjectId: "utterance_1",
        failureCode: "turn-not-accepted",
      }),
      audit(7, "asr-utterance-discarded", {
        component: "voice-asr",
        phase: "terminal",
        outcome: "cancelled",
        sessionId: "voice_1",
        subjectId: "utterance_2",
        failureCode: "no-speech",
      }),
    ]);

    expect(snapshot.diagnosis).toBe("llm-failed");
    expect(snapshot.stages[1].state).toBe("failure");
    expect(snapshot.stages[1].failureCode).toBe("context-scope-changed");
    expect(snapshot.stages[2].state).toBe("waiting");
    expect(snapshot.utteranceId).toBe("utterance_1");
  });

  test("distinguishes a completed response from TTS completion", () => {
    const common = [
      recognized,
      audit(2, "turn-requested", {
        causationId: "utterance_1",
        correlationId: "run_1",
        runtimeRunId: "run_1",
        attributes: { inputOrigin: "voice", presentationMode: "visual-and-spoken" },
      }),
      audit(3, "runtime-message-completed", {
        phase: "terminal",
        outcome: "success",
        correlationId: "run_1",
        runtimeRunId: "run_1",
      }),
    ];

    expect(projectLatestResponsePipeline(common).diagnosis).toBe("tts-waiting");
    expect(
      projectLatestResponsePipeline([
        ...common,
        audit(4, "speech-ended", {
          component: "tts",
          phase: "terminal",
          outcome: "success",
          correlationId: "run_1",
          runtimeRunId: "run_1",
        }),
      ]).diagnosis,
    ).toBe("completed");
  });
});
