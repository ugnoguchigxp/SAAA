import { createHash } from "node:crypto";
import type { QualityScenario } from "./scenarios";

export const SCORE_MAXIMA = {
  correctness: 35,
  relevance: 15,
  language: 15,
  spoken: 15,
  ambiguity: 10,
  tools: 10,
} as const;

export const HARD_VIOLATIONS = [
  "fabricated_source",
  "inferred_user_name",
  "false_local_claim",
  "false_asr_unavailable",
  "duplicated_answer",
  "assertion_after_tool_failure",
] as const;

const REQUIRED_CATEGORY_COUNTS: Record<QualityScenario["category"], number> = {
  ja: 10,
  en: 10,
  "ambiguous-asr": 10,
  long: 8,
  current: 8,
  "tool-failure": 7,
  continuation: 7,
};

export type Scores = Record<keyof typeof SCORE_MAXIMA, number>;
export type Evaluation = { scores: Scores; violations: string[] };
export type ScoredQualityResult = {
  scenarioId: string;
  round: number;
  score: number;
  scores: Scores;
  violationCodes: string[];
};
export type QualityRunSummary = {
  round: number;
  resultCount: number;
  average: number;
  categoryPercentages: Record<keyof Scores, number>;
  hardViolationCount: number;
  passed: boolean;
};
export type QualityGateSummary = {
  average: number;
  categoryPercentages: Record<keyof Scores, number>;
  hardViolationCount: number;
  runSummaries: QualityRunSummary[];
  passingRunCount: number;
  medianRunAverage: number;
  medianRunCategoryPercentages: Record<keyof Scores, number>;
  passed: boolean;
};

export function hashText(value: string): string {
  return createHash("sha256").update(value).digest("hex");
}

export function validateScenarios(scenarios: QualityScenario[]): string[] {
  const failures: string[] = [];
  const ids = new Set<string>();
  if (scenarios.length !== 60) failures.push(`expected 60 scenarios, received ${scenarios.length}`);
  for (const scenario of scenarios) {
    if (!/^[a-z][a-z0-9-]{0,79}$/.test(scenario.id)) failures.push(`invalid scenario id: ${scenario.id}`);
    if (ids.has(scenario.id)) failures.push(`duplicate scenario id: ${scenario.id}`);
    ids.add(scenario.id);
    if (!scenario.input.trim() || !scenario.expected.trim()) failures.push(`${scenario.id}: empty contract text`);
    if (scenario.toolMode !== "none" && !scenario.toolResult) failures.push(`${scenario.id}: missing tool result`);
    const expectedToolMode = scenario.category === "current"
      ? "success"
      : scenario.category === "tool-failure" ? "failure" : "none";
    if (scenario.toolMode !== expectedToolMode) failures.push(`${scenario.id}: invalid tool mode for ${scenario.category}`);
  }
  for (const [category, expectedCount] of Object.entries(REQUIRED_CATEGORY_COUNTS)) {
    const count = scenarios.filter((scenario) => scenario.category === category).length;
    if (count !== expectedCount) failures.push(`${category}: expected ${expectedCount} scenarios, received ${count}`);
  }
  return failures;
}

export function parseEvaluation(value: unknown): Evaluation {
  if (!value || typeof value !== "object") throw new Error("judge result must be an object");
  const record = value as Record<string, unknown>;
  if (Object.keys(record).sort().join(",") !== "scores,violations") {
    throw new Error("judge result must contain only scores and violations");
  }
  if (!record.scores || typeof record.scores !== "object") throw new Error("judge scores are missing");
  const rawScores = record.scores as Record<string, unknown>;
  if (Object.keys(rawScores).sort().join(",") !== Object.keys(SCORE_MAXIMA).sort().join(",")) {
    throw new Error("judge scores must contain the fixed rubric categories");
  }
  const scores = {} as Scores;
  for (const [key, maximum] of Object.entries(SCORE_MAXIMA) as Array<[keyof Scores, number]>) {
    const score = rawScores[key];
    if (typeof score !== "number" || !Number.isFinite(score) || score < 0 || score > maximum) {
      throw new Error(`invalid ${key} score`);
    }
    scores[key] = score;
  }
  const violations = record.violations;
  const allowed = new Set<string>(HARD_VIOLATIONS);
  if (!Array.isArray(violations) || violations.some((item) => typeof item !== "string" || !allowed.has(item))) {
    throw new Error("judge violations must contain only fixed violation codes");
  }
  return { scores, violations: violations as string[] };
}

export function totalScore(scores: Scores): number {
  return Object.values(scores).reduce((sum, score) => sum + score, 0);
}

function median(values: number[]): number {
  if (!values.length) throw new Error("cannot calculate a median without values");
  const sorted = [...values].sort((left, right) => left - right);
  const middle = Math.floor(sorted.length / 2);
  return sorted.length % 2 === 0
    ? ((sorted[middle - 1] ?? 0) + (sorted[middle] ?? 0)) / 2
    : (sorted[middle] ?? 0);
}

function summarizeResultSet(results: ScoredQualityResult[]): Omit<QualityRunSummary, "round" | "passed"> {
  if (!results.length) throw new Error("quality result set must not be empty");
  const average = results.reduce((sum, result) => sum + result.score, 0) / results.length;
  const categoryPercentages = Object.fromEntries(
    (Object.entries(SCORE_MAXIMA) as Array<[keyof Scores, number]>).map(([key, maximum]) => [
      key,
      results.reduce((sum, result) => sum + result.scores[key], 0) / results.length / maximum * 100,
    ]),
  ) as Record<keyof Scores, number>;
  return {
    average,
    categoryPercentages,
    hardViolationCount: results.reduce((sum, result) => sum + result.violationCodes.length, 0),
    resultCount: results.length,
  };
}

export function summarizeQualityGate(
  results: ScoredQualityResult[],
  expectedScenarioIds: string[],
  expectedRounds = 3,
): QualityGateSummary {
  if (expectedRounds !== 3) throw new Error("quality release gate requires exactly 3 rounds");
  const expectedIds = new Set(expectedScenarioIds);
  if (!expectedScenarioIds.length
    || expectedIds.size !== expectedScenarioIds.length
    || expectedScenarioIds.some((id) => !id.trim())) {
    throw new Error("quality release gate requires unique expected scenario ids");
  }
  if (results.length !== expectedScenarioIds.length * expectedRounds) {
    throw new Error("quality release gate received an incomplete result matrix");
  }
  const observed = new Set<string>();
  for (const result of results) {
    if (!Number.isInteger(result.round) || result.round < 1 || result.round > expectedRounds) {
      throw new Error(`quality result has an invalid round: ${result.round}`);
    }
    if (!expectedIds.has(result.scenarioId)) {
      throw new Error(`quality result has an unknown scenario id: ${result.scenarioId}`);
    }
    const resultKey = `${result.round}:${result.scenarioId}`;
    if (observed.has(resultKey)) throw new Error(`duplicate quality result: ${resultKey}`);
    observed.add(resultKey);
    const parsed = parseEvaluation({ scores: result.scores, violations: result.violationCodes });
    if (!Number.isFinite(result.score) || Math.abs(result.score - totalScore(parsed.scores)) > Number.EPSILON) {
      throw new Error(`quality result score does not match its rubric: ${resultKey}`);
    }
  }
  for (let round = 1; round <= expectedRounds; round += 1) {
    for (const scenarioId of expectedIds) {
      if (!observed.has(`${round}:${scenarioId}`)) {
        throw new Error(`missing quality result: ${round}:${scenarioId}`);
      }
    }
  }
  const runSummaries = Array.from({ length: expectedRounds }, (_, index) => {
    const round = index + 1;
    const run = summarizeResultSet(results.filter((result) => result.round === round));
    return {
      round,
      ...run,
      passed: run.average >= 90
        && Object.values(run.categoryPercentages).every((value) => value >= 85)
        && run.hardViolationCount === 0,
    };
  });
  const aggregate = summarizeResultSet(results);
  const medianRunCategoryPercentages = Object.fromEntries(
    (Object.keys(SCORE_MAXIMA) as Array<keyof Scores>).map((key) => [
      key,
      median(runSummaries.map((run) => run.categoryPercentages[key])),
    ]),
  ) as Record<keyof Scores, number>;
  const passingRunCount = runSummaries.filter((run) => run.passed).length;
  const medianRunAverage = median(runSummaries.map((run) => run.average));
  return {
    average: aggregate.average,
    categoryPercentages: aggregate.categoryPercentages,
    hardViolationCount: aggregate.hardViolationCount,
    runSummaries,
    passingRunCount,
    medianRunAverage,
    medianRunCategoryPercentages,
    passed: passingRunCount >= 2
      && medianRunAverage >= 90
      && Object.values(medianRunCategoryPercentages).every((value) => value >= 85)
      && aggregate.hardViolationCount === 0,
  };
}
