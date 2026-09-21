export type MatrixCase = {
  case_id: string;
  route: string;
  transition: string;
  pass: boolean;
  verification_level: string;
  omission_reason: string | null;
  [key: string]: unknown;
};

export { parseMatrix, parseMatrixOutput } from "./world-route-matrix-parse";
export { validateMatrix } from "./world-route-matrix-validate";
