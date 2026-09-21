import { mkdir, writeFile } from "node:fs/promises";
import { resolve } from "node:path";
export type TestCase = { name: string; result: "ok" | "FAILED" | "ignored"; manifest: string };
export type MatrixCase = {
  case_id: string;
  route: string;
  transition: string;
  pass: boolean;
  verification_level: string;
  omission_reason: string | null;
  [key: string]: unknown;
};
export function parseTests(output: string, manifest: string): TestCase[] {
  return [...output.matchAll(/^test ([^\s]+) \.\.\. (ok|FAILED|ignored)\b/gm)].map((m) => ({
    name: m[1],
    result: m[2] as TestCase["result"],
    manifest,
  }));
}
export function validateTests(cases: TestCase[], required: string[]): string[] {
  const identities = cases.map((c) => `${c.manifest}:${c.name}`);
  const errors: string[] = [];
  if (new Set(identities).size !== identities.length) errors.push("duplicate test identity");
  if (!cases.length) errors.push("zero tests");
  for (const prefix of required)
    if (!cases.some((c) => c.name.includes(prefix) && c.result === "ok"))
      errors.push(`missing passing card ${prefix}`);
  for (const c of cases) if (c.result === "FAILED") errors.push(`failed ${c.name}`);
  return errors;
}
export function parseMatrix(output: string): MatrixCase[] {
  return output.split("\n").flatMap((line) => {
    const marker = "WORLD_MATRIX_CASE=";
    const start = line.indexOf(marker);
    if (start < 0) return [];
    try { return [JSON.parse(line.slice(start + marker.length)) as MatrixCase]; }
    catch { return []; }
  });
}
export function validateMatrix(cases: MatrixCase[]): string[] {
  const routes = ["openai-compatible", "dynamic-lan", "shared-larm", "agent-session", "codex", "reasoning-mcp"];
  const transitions = ["initial", "tool-continuation", "fallback", "scope-switch", "correction", "forget", "session-resume"];
  const expected = new Set(routes.flatMap((route) => transitions.map((transition) => `${route}:${transition}`)));
  const identities = cases.map((entry) => entry.case_id);
  const errors: string[] = [];
  if (new Set(identities).size !== identities.length) errors.push("duplicate route matrix identity");
  for (const identity of expected) if (!identities.includes(identity)) errors.push(`missing route matrix case ${identity}`);
  for (const entry of cases) {
    if (!expected.has(entry.case_id)) errors.push(`unexpected route matrix case ${entry.case_id}`);
    if (entry.case_id !== `${entry.route}:${entry.transition}`) errors.push(`invalid route matrix identity ${entry.case_id}`);
    if (!entry.pass) errors.push(`failed route matrix case ${entry.case_id}`);
    if (entry.verification_level === "offline-contract" && entry.omission_reason !== "unsupported-capability")
      errors.push(`invalid N/A route matrix case ${entry.case_id}`);
  }
  return errors;
}
export async function runRemaining() {
  const directory = resolve("spec/evidence/world-delivery");
  await mkdir(directory, { recursive: true });
  const path = resolve(directory, "remaining-report.json");
  const startedAt = new Date().toISOString();
  await writeFile(
    path,
    JSON.stringify(
      { suite: "world-remaining", startedAt, complete: false, phase: "running" },
      null,
      2,
    ) + "\n",
  );
  const cases: TestCase[] = [];
  const matrix: MatrixCase[] = [];
  const exits: { manifest: string; exitCode: number }[] = [];
  for (const manifest of ["crates/personal-state-core/Cargo.toml", "src-tauri/Cargo.toml"]) {
    const child = Bun.spawn(
      [
        "cargo",
        "test",
        "--locked",
        "--manifest-path",
        manifest,
        "--lib",
        "wr_",
        "--",
        "--nocapture",
        "--test-threads=4",
      ],
      { stdout: "pipe", stderr: "inherit" },
    );
    const output = await new Response(child.stdout).text();
    const exitCode = await child.exited;
    process.stdout.write(output);
    exits.push({ manifest, exitCode });
    cases.push(...parseTests(output, manifest));
    matrix.push(...parseMatrix(output));
  }
  const required = Array.from({ length: 21 }, (_, i) => `wr_t${String(i + 1).padStart(2, "0")}_`);
  const errors = validateTests(cases, required);
  errors.push(...validateMatrix(matrix));
  for (const result of exits)
    if (result.exitCode !== 0) errors.push(`child failed: ${result.manifest}`);
  const report = {
    suite: "world-remaining",
    schemaVersion: 1,
    startedAt,
    completedAt: new Date().toISOString(),
    complete: errors.length === 0,
    liveVerified: false,
    scope:
      "T01-T21 card regression; separate route matrix, performance and product gates are required",
    cases,
    matrix,
    exits,
    errors,
  };
  await writeFile(path, JSON.stringify(report, null, 2) + "\n");
  if (errors.length) throw new Error(errors.join("\n"));
  console.log(
    `World remaining regression: ${cases.filter((c) => c.result === "ok").length} passed`,
  );
}
if (import.meta.main) await runRemaining();
