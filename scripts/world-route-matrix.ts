export type MatrixCase = {
  case_id: string;
  route: string;
  transition: string;
  pass: boolean;
  verification_level: string;
  omission_reason: string | null;
  [key: string]: unknown;
};

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

export function validateMatrix(cases: MatrixCase[]): string[] {
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
  const expected = new Set(
    routes.flatMap((route) => transitions.map((transition) => `${route}:${transition}`)),
  );
  const identities = cases.map((entry) => entry.case_id);
  const errors: string[] = [];
  if (new Set(identities).size !== identities.length)
    errors.push("duplicate route matrix identity");
  for (const identity of expected)
    if (!identities.includes(identity)) errors.push(`missing route matrix case ${identity}`);
  for (const entry of cases) {
    const identity = typeof entry.case_id === "string" ? entry.case_id : "<invalid>";
    if (!expected.has(entry.case_id)) errors.push(`unexpected route matrix case ${entry.case_id}`);
    if (entry.case_id !== `${entry.route}:${entry.transition}`)
      errors.push(`invalid route matrix identity ${identity}`);
    if (entry.pass !== true) errors.push(`failed route matrix case ${identity}`);
    if (entry.expected !== entry.actual || typeof entry.expected !== "string")
      errors.push(`route matrix outcome mismatch ${identity}`);
    const isContract = entry.verification_level === "offline-contract";
    if (!isContract && entry.verification_level !== "offline-wire")
      errors.push(`invalid verification level ${identity}`);
    if (isContract) {
      if (
        identity !== "reasoning-mcp:tool-continuation" ||
        entry.omission_reason !== "unsupported-capability" ||
        entry.expected !== "n/a-no-host-tool-capability" ||
        entry.request_count !== 0 ||
        entry.frame_digest !== null ||
        entry.wire_digest !== null
      )
        errors.push(`invalid N/A route matrix case ${identity}`);
      continue;
    }
    const denied = entry.transition === "scope-switch";
    if (denied) {
      if (
        entry.omission_reason !== "scope-changed" ||
        entry.request_count !== 0 ||
        entry.frame_digest !== null ||
        entry.wire_digest !== null ||
        !entry.expected.startsWith("denied-before-")
      )
        errors.push(`invalid denied route matrix case ${identity}`);
      continue;
    }
    const digest = (value: unknown) => typeof value === "string" && /^[0-9a-f]{64}$/.test(value);
    const sourceKinds = Array.isArray(entry.source_kinds)
      ? [...entry.source_kinds].sort().join(",")
      : "";
    const minimumRequests = ["tool-continuation", "fallback", "session-resume"].includes(
      entry.transition,
    )
      ? 2
      : 1;
    if (
      entry.omission_reason !== null ||
      entry.expected !== "current-frame-and-matching-receipt" ||
      typeof entry.request_count !== "number" ||
      entry.request_count < minimumRequests ||
      !digest(entry.frame_digest) ||
      !digest(entry.wire_digest) ||
      sourceKinds !== "coding,delegation,schedule,situation"
    )
      errors.push(`invalid wire route matrix case ${identity}`);
  }
  return errors;
}
