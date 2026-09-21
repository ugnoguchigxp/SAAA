import { expect, test } from "bun:test";
import { parseMatrixOutput, validateMatrix } from "../scripts/world-route-matrix";

test("wr_t22_route_matrix_requires_each_route_transition_once", () => {
  const routes = [
    "openai-compatible",
    "dynamic-lan",
    "shared-larm",
    "agent-session",
    "codex",
    "reasoning-mcp",
  ];
  const transitions = [
    "initial",
    "tool-continuation",
    "fallback",
    "scope-switch",
    "correction",
    "forget",
    "session-resume",
  ];
  const lines = routes.flatMap((route) =>
    transitions.map((transition) => {
      const contract = route === "reasoning-mcp" && transition === "tool-continuation";
      const denied = transition === "scope-switch";
      return `WORLD_MATRIX_CASE=${JSON.stringify({
        case_id: `${route}:${transition}`,
        route,
        transition,
        pass: true,
        verification_level: contract ? "offline-contract" : "offline-wire",
        omission_reason: contract ? "unsupported-capability" : denied ? "scope-changed" : null,
        expected: contract
          ? "n/a-no-host-tool-capability"
          : denied
            ? "denied-before-model-wire"
            : "current-frame-and-matching-receipt",
        actual: contract
          ? "n/a-no-host-tool-capability"
          : denied
            ? "denied-before-model-wire"
            : "current-frame-and-matching-receipt",
        request_count:
          contract || denied
            ? 0
            : ["tool-continuation", "fallback", "session-resume"].includes(transition)
              ? 2
              : 1,
        frame_digest: contract || denied ? null : "a".repeat(64),
        wire_digest: contract || denied ? null : "b".repeat(64),
        source_kinds: contract || denied ? [] : ["coding", "delegation", "schedule", "situation"],
      })}`;
    }),
  );
  const parsed = parseMatrixOutput(lines.join("\n"));
  expect(parsed.errors).toEqual([]);
  const matrix = parsed.cases;
  expect(validateMatrix(matrix)).toEqual([]);
  expect(validateMatrix(matrix.slice(1))).toContain(
    "missing route matrix case openai-compatible:initial",
  );
  expect(validateMatrix([...matrix, matrix[0]])).toContain("duplicate route matrix identity");
  expect(
    validateMatrix(
      matrix.map((entry, index) =>
        index === 0 ? { ...entry, verification_level: "offline-contract" } : entry,
      ),
    ),
  ).toContain("invalid N/A route matrix case openai-compatible:initial");
  expect(parseMatrixOutput(`${lines[0]}\nWORLD_MATRIX_CASE={broken`).errors).toEqual([
    "invalid route matrix JSON at output line 2",
  ]);
  expect(
    validateMatrix(
      matrix.map((entry, index) => (index === 0 ? { ...entry, pass: "true" } : entry)) as never,
    ),
  ).toContain("failed route matrix case openai-compatible:initial");
});
