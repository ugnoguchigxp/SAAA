import { mkdir, writeFile } from "node:fs/promises";
import { resolve } from "node:path";
export type TestCase = { name: string; result: "ok" | "FAILED" | "ignored"; manifest: string };
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
        "--test-threads=4",
      ],
      { stdout: "pipe", stderr: "inherit" },
    );
    const output = await new Response(child.stdout).text();
    const exitCode = await child.exited;
    process.stdout.write(output);
    exits.push({ manifest, exitCode });
    cases.push(...parseTests(output, manifest));
  }
  const required = Array.from({ length: 21 }, (_, i) => `wr_t${String(i + 1).padStart(2, "0")}_`);
  const errors = validateTests(cases, required);
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
