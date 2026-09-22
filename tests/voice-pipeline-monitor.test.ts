import { describe, expect, test } from "bun:test";
import { projectLatestResponsePipeline } from "../src/features/audit/voicePipelineMonitor";
import type { AuditEvent } from "../src/lib/contracts";

function audit(sequence: number, eventName: string, values: Partial<AuditEvent> = {}): AuditEvent {
  return {
    sequence,
    id: `audit_${sequence}`,
    occurredAt: String(sequence),
    component: "conversation",
    eventName,
    phase: "state",
    outcome: null,
    correlationId: null,
    causationId: null,
    conversationId: "conversation_1",
    runtimeRunId: null,
    sessionId: null,
    subjectId: null,
    failureCode: null,
    attributes: {},
    ...values,
  };
}

const recognized = audit(1, "asr-final-received", {
  component: "voice-asr",
  phase: "terminal",
  outcome: "success",
  correlationId: "voice_1",
  sessionId: "voice_1",
  subjectId: "utterance_1",
});

describe("voice pipeline monitor", () => {
  test("a new text request supersedes an old voice failure", () => {
    const snapshot = projectLatestResponsePipeline([
      recognized,
      audit(2, "turn-requested", { causationId: "utterance_1", runtimeRunId: "old" }),
      audit(3, "runtime-failed", { runtimeRunId: "old", outcome: "failure" }),
      audit(4, "turn-requested", { runtimeRunId: "new", attributes: { inputOrigin: "text" } }),
      audit(5, "runtime-event-started", { runtimeRunId: "new", phase: "start" }),
    ]);
    expect(snapshot.runId).toBe("new");
    expect(snapshot.stages[0].state).toBe("skipped");
    expect(snapshot.stages[1].state).toBe("running");
    expect(snapshot.relatedEvents.map((e) => e.sequence)).toEqual([4, 5]);
  });
  test("preserves the actionable provider cause after the generic terminal failure", () => {
    const snapshot = projectLatestResponsePipeline([
      recognized,
      audit(2, "turn-requested", { causationId: "utterance_1", runtimeRunId: "run_1" }),
      audit(3, "runtime-provider-failed", {
        runtimeRunId: "run_1",
        failureCode: "harness-llm-context-window-missing",
        outcome: "failure",
      }),
      audit(4, "runtime-failed", {
        runtimeRunId: "run_1",
        failureCode: "configuration-error",
        outcome: "failure",
      }),
    ]);
    expect(snapshot.stages[1].failureCode).toBe("harness-llm-context-window-missing");
    expect(snapshot.stages[1].state).toBe("failure");
  });
  test("shows the delivery gap after ASR recognition", () => {
    const snapshot = projectLatestResponsePipeline([recognized]);

    expect(snapshot.diagnosis).toBe("delivery-waiting");
    expect(snapshot.stages.map((stage) => stage.state)).toEqual(["success", "waiting", "idle"]);
  });

  test("recognizes the current LFM response event names", () => {
    const snapshot = projectLatestResponsePipeline([
      recognized,
      audit(2, "lfm-utterance-received", { subjectId: "utterance_1" }),
      audit(3, "lfm-replied-without-reasoning-request", { subjectId: "utterance_1" }),
    ]);
    expect(snapshot.diagnosis).toBe("lfm-responded");
    expect(snapshot.stages.find((stage) => stage.key === "lfm")?.state).toBe("success");
  });

  test("treats an intentional LFM silence as a completed reception", () => {
    const snapshot = projectLatestResponsePipeline([
      recognized,
      audit(2, "lfm-utterance-received", { subjectId: "utterance_1" }),
      audit(3, "lfm-silent-without-reasoning-request", { subjectId: "utterance_1" }),
    ]);
    expect(snapshot.diagnosis).toBe("lfm-responded");
    expect(snapshot.stages.find((stage) => stage.key === "lfm")?.state).toBe("success");
  });

  test("surfaces a Role Routing frontend rejection at the LFM stage", () => {
    const snapshot = projectLatestResponsePipeline([
      recognized,
      audit(2, "lfm-utterance-rejected", {
        subjectId: "utterance_1",
        outcome: "failure",
        failureCode: "role-routing-frontend-binding-mismatch",
      }),
    ]);
    expect(snapshot.diagnosis).toBe("lfm-failed");
    expect(snapshot.stages.find((stage) => stage.key === "lfm")?.failureCode).toBe(
      "role-routing-frontend-binding-mismatch",
    );
  });

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
