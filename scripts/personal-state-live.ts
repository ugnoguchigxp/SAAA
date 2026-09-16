import { spawn } from "node:child_process";
import { createHash } from "node:crypto";
import { readFileSync, writeFileSync } from "node:fs";
import { z } from "zod";
import { corpusSchema, resultSchema, evaluate } from "./personal-state-eval";

const deploymentSchema = z
  .object({
    model: z.string().min(1),
    release: z.string().min(1),
    runtime: z.string().min(1),
    policyDigest: z.string().regex(/^[a-f0-9]{64}$/),
    promptDigest: z.string().regex(/^[a-f0-9]{64}$/),
    tokenizerDigest: z.string().regex(/^[a-f0-9]{64}$/),
    snapshotMode: z.enum(["off", "on"]),
    maxInputTokens: z.number().int().positive(),
    maxInputBytes: z.number().int().positive(),
    maxOutputTokens: z.number().int().positive().max(2000),
    command: z.array(z.string().min(1)).min(1),
  })
  .strict();
type Execute = (command: string[], input: unknown, timeoutMs: number) => Promise<unknown>;

/** Explicit operator-supplied harness, never a guessed LARM endpoint or shell command.
 * Each child must run the SAAA lifecycle and return resultSchema JSON, not model self-grading.
 */
export const executeHarness: Execute = (command, input, timeoutMs) =>
  new Promise((resolve, reject) => {
    const child = spawn(command[0], command.slice(1), {
      stdio: ["pipe", "pipe", "pipe"],
      detached: process.platform !== "win32",
    });
    let output: Buffer[] = [],
      bytes = 0,
      ended = false;
    const stop = () => {
      if (child.pid && process.platform !== "win32") {
        try {
          process.kill(-child.pid, "SIGKILL");
        } catch {
          /* Already exited. */
        }
      } else child.kill("SIGKILL");
    };
    const fail = (code: string) => {
      if (ended) return;
      ended = true;
      clearTimeout(timer);
      stop();
      reject(new Error(code));
    };
    const timer = setTimeout(() => fail("harness-timeout"), timeoutMs);
    child.once("error", () => fail("harness-unavailable"));
    child.stdin.on("error", () => fail("harness-input-failed"));
    child.stdout.on("data", (part: Buffer) => {
      bytes += part.length;
      if (bytes > 1_048_576) {
        fail("harness-output-budget");
        return;
      }
      output.push(part);
    });
    // Never persist stderr: the operator's harness may log credentials/source content.
    child.stderr.resume();
    child.once("close", (code) => {
      if (ended) return;
      if (code !== 0) {
        fail("harness-failed");
        return;
      }
      ended = true;
      clearTimeout(timer);
      stop();
      try {
        resolve(JSON.parse(Buffer.concat(output).toString("utf8")));
      } catch {
        reject(new Error("harness-output-schema"));
      }
    });
    child.stdin.end(JSON.stringify(input));
  });

export async function runLive(
  corpusInput: unknown,
  deploymentInput: unknown,
  execute: Execute = executeHarness,
) {
  const corpus = corpusSchema.parse(corpusInput),
    deployment = deploymentSchema.parse(deploymentInput);
  const results: z.infer<typeof resultSchema>[] = [];
  const failures: { scenarioId: string; repetition: number; code: string }[] = [];
  for (let repetition = 1; repetition <= corpus.repetitions; repetition++) {
    for (const scenario of corpus.scenarios) {
      const start = performance.now();
      try {
        const input = {
          schemaVersion: 1,
          scenarioId: scenario.id,
          repetition,
          checkpoint: scenario.checkpoint,
          sources: scenario.sources,
          deployment: { ...deployment, command: undefined },
          resetState: true,
        };
        if (Buffer.byteLength(JSON.stringify(input)) > deployment.maxInputBytes)
          throw new Error("harness-input-budget");
        const raw = await execute(deployment.command, input, Math.min(scenario.deadlineMs, 30000));
        const result = resultSchema.parse(raw);
        if (
          result.scenarioId !== scenario.id ||
          result.repetition !== repetition ||
          result.checkpoint !== scenario.checkpoint
        )
          throw new Error("harness-result-binding");
        const ids = new Set(scenario.sources.map((s) => s.id));
        if (result.coveredSources.some((id) => !ids.has(id)))
          throw new Error("harness-result-binding");
        results.push({ ...result, elapsedMs: performance.now() - start });
      } catch (error) {
        const code =
          error instanceof Error && /^harness-[a-z-]+$/.test(error.message)
            ? error.message
            : "harness-result-invalid";
        failures.push({ scenarioId: scenario.id, repetition, code });
      }
    }
  }
  return {
    schemaVersion: 1,
    corpusVersion: corpus.version,
    corpusDigest: createHash("sha256").update(JSON.stringify(corpus)).digest("hex"),
    deployment: { ...deployment, command: undefined },
    sampleCount: results.length,
    expectedSamples: corpus.scenarios.length * corpus.repetitions,
    failures,
    results,
    evaluation: evaluate(corpus, results),
  };
}
if (import.meta.main) {
  const [deploymentFile, outputFile] = process.argv.slice(2);
  if (!deploymentFile || !outputFile)
    throw new Error("Usage: bun scripts/personal-state-live.ts <deployment.json> <evidence.json>");
  const corpus = JSON.parse(readFileSync("tests/fixtures/personal-state/ja-corpus.json", "utf8"));
  const evidence = await runLive(corpus, JSON.parse(readFileSync(deploymentFile, "utf8")));
  writeFileSync(outputFile, JSON.stringify(evidence, null, 2) + "\n", { mode: 0o600 });
  if (evidence.failures.length || !evidence.evaluation.certified) process.exitCode = 1;
}
