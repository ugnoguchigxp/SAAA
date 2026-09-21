import { mkdir, writeFile } from "node:fs/promises";
import { resolve } from "node:path";

// Offline only: the Rust suites bind loopback servers and use synthetic databases.
const child = Bun.spawn(
  ["cargo", "test", "--locked", "--manifest-path", "src-tauri/Cargo.toml", "--lib", "world_m4a_", "--", "--nocapture"],
  { stdout: "pipe", stderr: "inherit" },
);
const output = await new Response(child.stdout).text();
process.stdout.write(output);
const code = await child.exited;
type Case = { case_id: string; pass: boolean; [key: string]: unknown };
const cases: Case[] = [];
for (const line of output.split("\n")) {
  const prefix = "WORLD_EVAL_REPORT=";
  const at = line.indexOf(prefix);
  if (at < 0) continue;
  const report = JSON.parse(line.slice(at + prefix.length));
  cases.push(...report.cases);
}
const required = [
  "W01", "W02", "W03", "W04", "W05", "W06", "W07", "W08", "W09", "W10",
  "W11-agent-initial-followup", "W12-agent-expired", "W13", "W14", "W15", "W16",
  "W17a", "W17b", "W18a", "W18b", "W20", "G1-five-elements", "G1-unknown",
  "G1-goal-revoked", "G1-relation-changed", "G1-followup",
];
const ids = new Set(cases.map((c) => c.case_id));
const missing = required.filter((id) => !ids.has(id));
const passed = cases.filter((c) => c.pass).length;
const ok = code === 0 && missing.length === 0 && ids.size === cases.length && passed === cases.length;
const report = {
  schema_version: 1, suite: "world-m4a", contract: "WD: AgentSession initial-only; G1 graph enabled",
  summary: { total: cases.length, passed, failed: cases.length - passed, skipped: missing.length, complete: ok },
  missing, test_exit_code: code,
  cases: cases.sort((a, b) => a.case_id.localeCompare(b.case_id)),
};
const directory = resolve("spec/evidence/world-model");
await mkdir(directory, { recursive: true });
await writeFile(resolve(directory, "m4a-report.json"), `${JSON.stringify(report, null, 2)}\n`);
if (!ok) {
  console.error(`World evaluation incomplete: exit=${code}, missing=${missing.join(",")}`);
  process.exit(1);
}
console.log(`World evaluation: ${passed} cases passed`);
