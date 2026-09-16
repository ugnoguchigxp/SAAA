import { expect, test } from "bun:test";
import { performanceReport } from "../scripts/personal-state-performance";
test("finite performance gate keeps failures and cold/warm populations separate", () => {
  const samples = ["cold", "warm"].flatMap((condition) =>
    ["baseline", "personal-state"].flatMap((mode) =>
      Array.from({ length: 100 }, () => ({
        condition,
        mode,
        firstPlayableMs: 100,
        ttftMs: 200,
        finalMs: 500,
        workerWaitMs: 30000,
        cancelSendMs: 50,
        cpuPercent: 20,
        tokensPerSecond: 40,
        sourceTokens: 10,
        sourceBytes: 50,
        snapshotBytes: 50,
        asrRtf: 0.3,
        ttsFirstPlayableMs: 100,
        ramBytes: 100,
        vramBytes: 100,
        concurrentGenerations: 1,
        success: true,
        intentionalFault: false,
      })),
    ),
  );
  const input = {
    release: "fixture",
    limitsFixedBeforeRun: true,
    maxCpuPercent: 100,
    maxSourceTokens: 1000,
    maxSourceBytes: 1000,
    maxSnapshotBytes: 1000,
    maxRamBytes: 200,
    maxVramBytes: 200,
    samples,
  };
  expect(performanceReport(input).passed).toBe(true);
  samples[0].success = false;
  expect(performanceReport(input).passed).toBe(false);
  samples[0].success = true;
  samples[100].ttftMs = 10000;
  samples[101].ttftMs = 10000;
  samples[102].ttftMs = 10000;
  samples[103].ttftMs = 10000;
  samples[104].ttftMs = 10000;
  samples[105].ttftMs = 10000;
  expect(performanceReport(input).passed).toBe(false);
  expect(performanceReport({ ...input, samples: samples.slice(0, 100) }).passed).toBe(false);
});
