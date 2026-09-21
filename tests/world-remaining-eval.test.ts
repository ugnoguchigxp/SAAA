import { expect, test } from "bun:test";
import { parseMatrix, parseTests, validateMatrix, validateTests } from "../scripts/world-remaining-eval";
test("wr_t22_missing_duplicate_failed_and_zero_tests_fail", () => {
  expect(validateTests([], ["wr_t01_"])).toContain("zero tests");
  const cases = parseTests(
    "test module::wr_t01_contract ... ok\ntest module::wr_t02_scope ... FAILED\n",
    "fixture",
  );
  expect(validateTests(cases, ["wr_t03_"]).length).toBe(2);
  expect(validateTests([cases[0], cases[0]], [])).toContain("duplicate test identity");
  expect(validateTests([cases[0]], ["wr_t01_"])).toEqual([]);
});

test("wr_t22_route_matrix_requires_each_route_transition_once", () => {
  const routes = ["openai-compatible", "dynamic-lan", "shared-larm", "agent-session", "codex", "reasoning-mcp"];
  const transitions = ["initial", "tool-continuation", "fallback", "scope-switch", "correction", "forget", "session-resume"];
  const lines = routes.flatMap((route) => transitions.map((transition) =>
    `WORLD_MATRIX_CASE=${JSON.stringify({case_id:`${route}:${transition}`,route,transition,pass:true,verification_level:"offline-wire",omission_reason:null})}`));
  const matrix = parseMatrix(lines.join("\n"));
  expect(validateMatrix(matrix)).toEqual([]);
  expect(validateMatrix(matrix.slice(1))).toContain("missing route matrix case openai-compatible:initial");
  expect(validateMatrix([...matrix, matrix[0]])).toContain("duplicate route matrix identity");
  expect(validateMatrix(matrix.map((entry, index) => index === 0 ? {...entry, verification_level:"offline-contract"} : entry)))
    .toContain("invalid N/A route matrix case openai-compatible:initial");
});
