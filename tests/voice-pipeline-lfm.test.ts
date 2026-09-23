import { describe, expect, test } from "bun:test";
import { projectLatestResponsePipeline } from "../src/features/audit/voicePipelineMonitor";
import { audit, recognized } from "./voice-pipeline-monitor-fixture";

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

});
