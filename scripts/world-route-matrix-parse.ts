import type { MatrixCase } from "./world-route-matrix";

export function parseMatrix(output: string): MatrixCase[] {
  return parseMatrixOutput(output).cases;
}

export function parseMatrixOutput(output: string): { cases: MatrixCase[]; errors: string[] } {
  const cases: MatrixCase[] = [];
  const errors: string[] = [];
  for (const [index, line] of output.split("\n").entries()) {
    const marker = "WORLD_MATRIX_CASE=";
    const start = line.indexOf(marker);
    if (start < 0) continue;
    try {
      const parsed: unknown = JSON.parse(line.slice(start + marker.length));
      if (!parsed || typeof parsed !== "object" || Array.isArray(parsed))
        errors.push(`invalid route matrix object at output line ${index + 1}`);
      else cases.push(parsed as MatrixCase);
    } catch {
      errors.push(`invalid route matrix JSON at output line ${index + 1}`);
    }
  }
  return { cases, errors };
}
