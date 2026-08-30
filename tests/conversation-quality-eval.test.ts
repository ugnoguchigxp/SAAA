import { describe, expect, test } from "bun:test";
import { normalizeEndpointBaseUrl, parseWebSearchToolCall } from "../scripts/conversation-quality/protocol";
import { QUALITY_SCENARIOS } from "../scripts/conversation-quality/scenarios";
import { parseEvaluation, summarizeQualityGate, totalScore, validateScenarios, type Scores } from "../scripts/conversation-quality/schema";

const scenarioIds = QUALITY_SCENARIOS.map((scenario) => scenario.id);
const scaledScores = (value: number): Scores => ({
  correctness: 35 * value,
  relevance: 15 * value,
  language: 15 * value,
  spoken: 15 * value,
  ambiguity: 10 * value,
  tools: 10 * value,
});
const resultMatrix = (scoreForRound: (round: number) => number) => [1, 2, 3].flatMap((round) =>
  scenarioIds.map((scenarioId) => {
    const scores = scaledScores(scoreForRound(round) / 100);
    return { scenarioId, round, score: totalScore(scores), scores, violationCodes: [] as string[] };
  }));

describe("conversation quality evaluation contract", () => {
  test("fixes 60 unique scenarios across every required category", () => {
    expect(validateScenarios(QUALITY_SCENARIOS)).toEqual([]);
    expect(new Set(QUALITY_SCENARIOS.map((scenario) => scenario.id)).size).toBe(60);
  });

  test("rejects scores outside the rubric and totals valid scores", () => {
    const valid = parseEvaluation({ scores: { correctness: 35, relevance: 15, language: 15, spoken: 15, ambiguity: 10, tools: 10 }, violations: [] });
    expect(totalScore(valid.scores)).toBe(100);
    expect(() => parseEvaluation({ scores: { ...valid.scores, tools: 11 }, violations: [] })).toThrow("invalid tools score");
    expect(() => parseEvaluation({ scores: valid.scores, violations: ["raw response content"] })).toThrow("fixed violation codes");
  });

  test("requires tool fixtures for current-information cases", () => {
    const broken = QUALITY_SCENARIOS.map((scenario) => scenario.id === "current-01" ? { ...scenario, toolResult: undefined } : scenario);
    expect(validateScenarios(broken)).toContain("current-01: missing tool result");
    const wrongMode = QUALITY_SCENARIOS.map((scenario) => scenario.id === "ja-01" ? { ...scenario, toolMode: "success" as const, toolResult: "fixture" } : scenario);
    expect(validateScenarios(wrongMode)).toContain("ja-01: invalid tool mode for ja");
    const wrongCoverage = QUALITY_SCENARIOS.map((scenario) => scenario.id === "en-01" ? { ...scenario, category: "ja" as const } : scenario);
    expect(validateScenarios(wrongCoverage)).toContain("ja: expected 10 scenarios, received 11");
  });

  test("release gate requires two independently passing runs and passing medians", () => {
    const gate = summarizeQualityGate(resultMatrix((round) => round === 3 ? 80 : 95), scenarioIds);
    expect(gate.passingRunCount).toBe(2);
    expect(gate.medianRunAverage).toBe(95);
    expect(gate.passed).toBe(true);
  });

  test("one hard violation fails the aggregate gate", () => {
    const results = resultMatrix(() => 100);
    const violated = results.find((result) => result.round === 2);
    if (violated) violated.violationCodes = ["fabricated_source"];
    const gate = summarizeQualityGate(results, scenarioIds);
    expect(gate.passingRunCount).toBe(2);
    expect(gate.hardViolationCount).toBe(1);
    expect(gate.passed).toBe(false);
  });

  test("rejects incomplete, duplicate, and internally inconsistent result matrices", () => {
    const complete = resultMatrix(() => 100);
    expect(() => summarizeQualityGate(complete.slice(1), scenarioIds)).toThrow("incomplete result matrix");
    expect(() => summarizeQualityGate([...complete.slice(0, -1), complete[0]!], scenarioIds)).toThrow("duplicate quality result");
    expect(() => summarizeQualityGate([{ ...complete[0]!, score: 99 }, ...complete.slice(1)], scenarioIds)).toThrow("does not match its rubric");
  });

  test("validates endpoint and web-search tool protocol", () => {
    expect(normalizeEndpointBaseUrl("https://example.test/v1/")).toBe("https://example.test/v1");
    expect(() => normalizeEndpointBaseUrl("file:///tmp/eval")).toThrow("HTTP or HTTPS");
    expect(() => normalizeEndpointBaseUrl("https://user:secret@example.test/v1")).toThrow("must not contain credentials");
    expect(parseWebSearchToolCall([{ id: "call_1", type: "function", function: { name: "web_search", arguments: '{"query":"current weather"}' } }]).id).toBe("call_1");
    expect(() => parseWebSearchToolCall([{ id: "call_1", type: "function", function: { name: "other", arguments: "{}" } }])).toThrow("must invoke web_search");
    expect(() => parseWebSearchToolCall([{ id: "call_1", type: "function", function: { name: "web_search", arguments: '{"query":"weather","extra":true}' } }])).toThrow("contain only query");
  });
});
