import { readFileSync, writeFileSync } from "node:fs";
import { z } from "zod";
const source = z
  .object({
    id: z.string(),
    version: z.number().int().positive(),
    start: z.number().int().nonnegative(),
    end: z.number().int().positive(),
  })
  .strict();
const item = z
  .object({ kind: z.string(), target: z.string(), value: z.string(), status: z.string(), source })
  .strict();
const scenario = z
  .object({
    id: z.string(),
    category: z.string(),
    sources: z.array(
      z.object({ id: z.string(), version: z.number().int(), text: z.string() }).strict(),
    ),
    checkpoint: z.string(),
    gold: z.array(item),
    coverageRequired: z.array(z.string()),
    forbidden: z.array(z.string()),
    deadlineMs: z.number().positive(),
  })
  .strict();
export const corpusSchema = z
  .object({
    version: z.string(),
    humanReviewed: z.boolean(),
    reviewNote: z.string(),
    repetitions: z.literal(3),
    scenarios: z.array(scenario).min(60),
  })
  .strict();
export const resultSchema = z
  .object({
    scenarioId: z.string(),
    repetition: z.number().int().min(1).max(3),
    checkpoint: z.string(),
    items: z.array(item),
    coveredSources: z.array(z.string()),
    elapsedMs: z.number().nonnegative(),
    violations: z.array(z.string()),
  })
  .strict();
export function evaluate(corpusInput: unknown, resultsInput: unknown) {
  const corpus = corpusSchema.parse(corpusInput);
  const results = z.array(resultSchema).parse(resultsInput);
  const ids = new Set(corpus.scenarios.map((s) => s.id));
  if (ids.size !== corpus.scenarios.length) throw new Error("duplicate scenario");
  const categories = new Map<string, number>();
  for (const s of corpus.scenarios)
    categories.set(s.category, (categories.get(s.category) ?? 0) + 1);
  if (categories.size < 8 || [...categories.values()].some((n) => n < 5))
    throw new Error("category coverage incomplete");
  const seen = new Set<string>();
  for (const r of results) {
    const key = `${r.scenarioId}/${r.repetition}`;
    if (!ids.has(r.scenarioId) || seen.has(key)) throw new Error("unknown or duplicate result");
    seen.add(key);
  }
  const repetitions = [];
  for (let repetition = 1; repetition <= 3; repetition++) {
    let expected = 0,
      produced = 0,
      correct = 0,
      pending = 0,
      totalSources = 0,
      violations = 0,
      missing = 0;
    for (const s of corpus.scenarios) {
      expected += s.gold.length;
      totalSources += s.coverageRequired.length;
      const r = results.find(
        (r) =>
          r.scenarioId === s.id && r.repetition === repetition && r.checkpoint === s.checkpoint,
      );
      if (!r) {
        missing++;
        pending += s.coverageRequired.length;
        continue;
      }
      produced += r.items.length;
      violations += r.violations.length;
      const matched = new Set<number>();
      for (const actual of r.items) {
        const index = s.gold.findIndex(
          (gold, i) => !matched.has(i) && JSON.stringify(gold) === JSON.stringify(actual),
        );
        if (index >= 0) {
          matched.add(index);
          correct++;
        } else if (actual.kind === "decision" && actual.status === "active") {
          violations++;
        }
      }
      pending += s.coverageRequired.filter(
        (id) => !r.coveredSources.includes(id) || r.elapsedMs > s.deadlineMs,
      ).length;
    }
    const recall = expected ? correct / expected : 1,
      precision = produced ? correct / produced : expected ? 0 : 1,
      unprocessed = totalSources ? pending / totalSources : 0;
    repetitions.push({
      repetition,
      recall,
      precision,
      unprocessed,
      violations,
      missing,
      passed:
        recall >= 0.95 &&
        precision >= 0.95 &&
        unprocessed <= 0.05 &&
        violations === 0 &&
        missing === 0,
    });
  }
  return {
    version: corpus.version,
    humanReviewed: corpus.humanReviewed,
    repetitions,
    certified: corpus.humanReviewed && repetitions.every((r) => r.passed),
  };
}
if (import.meta.main) {
  const corpus = JSON.parse(readFileSync("tests/fixtures/personal-state/ja-corpus.json", "utf8"));
  const input = process.argv[2];
  if (!input)
    throw new Error("Usage: bun scripts/personal-state-eval.ts <results.json> [summary.json]");
  const result = evaluate(corpus, JSON.parse(readFileSync(input, "utf8")));
  const encoded = JSON.stringify(result, null, 2) + "\n";
  if (process.argv[3]) writeFileSync(process.argv[3], encoded);
  else process.stdout.write(encoded);
  if (!result.certified) process.exitCode = 1;
}
