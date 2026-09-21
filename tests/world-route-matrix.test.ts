import { expect, test } from "bun:test";
import { parseMatrixOutput, validateMatrix } from "../scripts/world-route-matrix";
import { validMatrixOutput } from "./support/world-route-matrix-fixture";

test("wr_t22_route_matrix_is_strict", () => {
  const { cases: matrix, errors } = parseMatrixOutput(validMatrixOutput());
  expect(errors).toEqual([]);
  expect(validateMatrix(matrix)).toEqual([]);
  expect(validateMatrix(matrix.slice(1))).toContain(
    "missing route matrix case openai-compatible:initial",
  );
  expect(validateMatrix([...matrix, matrix[0]])).toContain("duplicate route matrix identity");
  expect(parseMatrixOutput("WORLD_MATRIX_CASE={broken").errors).toHaveLength(1);
  expect(
    validateMatrix(matrix.map((entry, i) => (i ? entry : { ...entry, pass: "true" })) as never),
  ).toContain("failed route matrix case openai-compatible:initial");
});
