import { expect, test } from "bun:test";
import corpus from "./fixtures/personal-state/ja-corpus.json";
import { runLive, executeHarness } from "../scripts/personal-state-live";
const deployment = {
  model: "fixture",
  release: "fixture",
  runtime: "fixture",
  policyDigest: "a".repeat(64),
  promptDigest: "b".repeat(64),
  tokenizerDigest: "c".repeat(64),
  snapshotMode: "off",
  maxInputTokens: 100,
  maxInputBytes: 10000,
  maxOutputTokens: 100,
  command: ["fixture"],
};
test("finite live collector withholds gold, binds results and preserves failures", async () => {
  let count = 0;
  const result = await runLive(corpus, deployment, async (_, raw) => {
    const input = raw as { scenarioId: string; repetition: number; checkpoint: string };
    expect(raw).not.toHaveProperty("gold");
    expect(raw).not.toHaveProperty("forbidden");
    count++;
    if (count === 1) throw new Error("secret from harness");
    return {
      scenarioId: input.scenarioId,
      repetition: input.repetition,
      checkpoint: input.checkpoint,
      items: [],
      coveredSources: [],
      elapsedMs: 0,
      violations: [],
    };
  });
  expect(count).toBe(192);
  expect(result.failures).toEqual([
    { scenarioId: "negation-01", repetition: 1, code: "harness-result-invalid" },
  ]);
  expect(result.sampleCount).toBe(191);
  expect(result.evaluation.certified).toBe(false);
});
test("harness child has bounded lifetime and accepts JSON over stdin", async () => {
  await expect(
    executeHarness([process.execPath, "-e", "setTimeout(()=>{},10000)"], {}, 30),
  ).rejects.toThrow("harness-timeout");
  expect(
    await executeHarness(
      [process.execPath, "-e", "process.stdin.on('data',d=>process.stdout.write(d))"],
      { ok: true },
      3000,
    ),
  ).toEqual({ ok: true });
});
