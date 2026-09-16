import { expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { corpusSchema, evaluate } from "../scripts/personal-state-eval";
const corpus = corpusSchema.parse(
  JSON.parse(readFileSync("tests/fixtures/personal-state/ja-corpus.json", "utf8")),
);
test("all 64 scenarios have fixed ranges; unreviewed gold cannot certify", () => {
  expect(corpus.scenarios.length).toBe(64);
  for (const s of corpus.scenarios)
    for (const gold of s.gold)
      expect(gold.source.end).toBe(new TextEncoder().encode(s.sources[0].text).length);
  const results = corpus.scenarios.flatMap((s) =>
    [1, 2, 3].map((repetition) => ({
      scenarioId: s.id,
      repetition,
      checkpoint: s.checkpoint,
      items: s.gold,
      coveredSources: s.coverageRequired,
      elapsedMs: 100,
      violations: [],
    })),
  );
  const result = evaluate(corpus, results);
  expect(result.repetitions.every((r) => r.passed)).toBe(true);
  expect(result.certified).toBe(false);
  results[0].items = [
    { ...results[0].items[0], source: { ...results[0].items[0].source, version: 2 } },
  ];
  expect(evaluate(corpus, results).repetitions[0].precision).toBeLessThan(1);
});
test("missing repetitions and duplicate outputs never count as success", () => {
  expect(evaluate(corpus, []).repetitions.every((r) => !r.passed)).toBe(true);
  const s = corpus.scenarios[0];
  const r = {
    scenarioId: s.id,
    repetition: 1,
    checkpoint: s.checkpoint,
    items: s.gold,
    coveredSources: s.coverageRequired,
    elapsedMs: 100,
    violations: [],
  };
  expect(() => evaluate(corpus, [r, r])).toThrow("duplicate");
});
