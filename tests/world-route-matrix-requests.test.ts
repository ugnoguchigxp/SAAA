import { expect, test } from "bun:test";
import { parseMatrix, validateMatrix } from "../scripts/world-route-matrix";
import { validMatrixOutput } from "./support/world-route-matrix-fixture";

test("wr_t22_session_resume_requires_two_requests", () => {
  const matrix = parseMatrix(validMatrixOutput()).map((entry) =>
    entry.case_id === "openai-compatible:session-resume" ? { ...entry, request_count: 1 } : entry,
  );
  expect(validateMatrix(matrix)).toContain(
    "invalid wire route matrix case openai-compatible:session-resume",
  );
});
