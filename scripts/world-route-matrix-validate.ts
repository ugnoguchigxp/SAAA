import type { MatrixCase } from "./world-route-matrix";

const ROUTES = [
  "openai-compatible",
  "dynamic-lan",
  "shared-larm",
  "agent-session",
  "codex",
  "reasoning-mcp",
];
const TRANSITIONS = [
  "initial",
  "tool-continuation",
  "fallback",
  "scope-switch",
  "correction",
  "forget",
  "session-resume",
];

export function validateMatrix(cases: MatrixCase[]): string[] {
  const expected = new Set(
    ROUTES.flatMap((route) => TRANSITIONS.map((item) => `${route}:${item}`)),
  );
  const identities = cases.map((entry) => entry.case_id);
  const errors: string[] = [];
  if (new Set(identities).size !== identities.length)
    errors.push("duplicate route matrix identity");
  for (const identity of expected)
    if (!identities.includes(identity)) errors.push(`missing route matrix case ${identity}`);
  for (const entry of cases) validateEntry(entry, expected, errors);
  return errors;
}

function validateEntry(entry: MatrixCase, expected: Set<string>, errors: string[]): void {
  const identity = typeof entry.case_id === "string" ? entry.case_id : "<invalid>";
  if (!expected.has(entry.case_id)) errors.push(`unexpected route matrix case ${entry.case_id}`);
  if (entry.case_id !== `${entry.route}:${entry.transition}`)
    errors.push(`invalid route matrix identity ${identity}`);
  if (entry.pass !== true) errors.push(`failed route matrix case ${identity}`);
  if (entry.expected !== entry.actual || typeof entry.expected !== "string")
    errors.push(`route matrix outcome mismatch ${identity}`);
  const contract = entry.verification_level === "offline-contract";
  if (!contract && entry.verification_level !== "offline-wire")
    errors.push(`invalid verification level ${identity}`);
  if (contract) return validateContract(entry, identity, errors);
  if (entry.transition === "scope-switch") return validateDenied(entry, identity, errors);
  validateWire(entry, identity, errors);
}

function validateContract(entry: MatrixCase, identity: string, errors: string[]): void {
  if (
    identity !== "reasoning-mcp:tool-continuation" ||
    entry.omission_reason !== "unsupported-capability" ||
    entry.expected !== "n/a-no-host-tool-capability" ||
    entry.request_count !== 0 ||
    entry.frame_digest !== null ||
    entry.wire_digest !== null ||
    !Array.isArray(entry.source_kinds) ||
    entry.source_kinds.length !== 0
  )
    errors.push(`invalid N/A route matrix case ${identity}`);
}

function validateDenied(entry: MatrixCase, identity: string, errors: string[]): void {
  if (
    entry.omission_reason !== "scope-changed" ||
    entry.request_count !== 0 ||
    entry.frame_digest !== null ||
    entry.wire_digest !== null ||
    !Array.isArray(entry.source_kinds) ||
    entry.source_kinds.length !== 0 ||
    typeof entry.expected !== "string" ||
    !entry.expected.startsWith("denied-before-")
  )
    errors.push(`invalid denied route matrix case ${identity}`);
}

function validateWire(entry: MatrixCase, identity: string, errors: string[]): void {
  const digest = (value: unknown) => typeof value === "string" && /^[0-9a-f]{64}$/.test(value);
  const kinds = Array.isArray(entry.source_kinds) ? [...entry.source_kinds].sort().join(",") : "";
  const multiAttemptFallback =
    entry.transition === "fallback" &&
    ["openai-compatible", "dynamic-lan", "shared-larm"].includes(entry.route);
  const expectedRequests = multiAttemptFallback
    ? 4
    : ["tool-continuation", "fallback", "session-resume"].includes(entry.transition)
      ? 2
      : 1;
  if (
    entry.omission_reason !== null ||
    entry.expected !== "current-frame-and-matching-receipt" ||
    !Number.isInteger(entry.request_count) ||
    entry.request_count !== expectedRequests ||
    !digest(entry.frame_digest) ||
    !digest(entry.wire_digest) ||
    kinds !== "coding,delegation,schedule,situation"
  )
    errors.push(`invalid wire route matrix case ${identity}`);
}
