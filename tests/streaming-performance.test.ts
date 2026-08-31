import { describe, expect, test } from "bun:test";
import {
  beginRunPerformance,
  recordFirstDelta,
  recordFirstPlainPaint,
  recordMarkdownPaint,
  recordPlainCommit,
  recordResponseCompleted,
  runPerformanceSnapshot,
} from "../src/features/chat/streamingPerformance";

describe("streaming performance telemetry", () => {
  test("records stage timings and counts without response content", () => {
    beginRunPerformance("run_perf_test");
    recordFirstDelta("run_perf_test");
    expect(recordPlainCommit("run_perf_test")).toBeTrue();
    recordFirstPlainPaint("run_perf_test");
    expect(recordPlainCommit("run_perf_test")).toBeFalse();
    recordResponseCompleted("run_perf_test", "message_perf_test");
    recordMarkdownPaint("message_perf_test");

    const snapshot = runPerformanceSnapshot("run_perf_test");
    expect(snapshot?.streamingCommits).toBe(2);
    expect(snapshot?.terminal).toBe("completed");
    expect(snapshot?.firstDeltaAt).not.toBeNull();
    expect(snapshot?.markdownPaintAt).not.toBeNull();
    expect(JSON.stringify(snapshot)).not.toContain("response text");
  });
});
