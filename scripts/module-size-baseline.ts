import { existsSync, readFileSync, writeFileSync } from "node:fs";
import { collectSizes, evaluate, type BaselineFile, type SizeRecord } from "./module-size";

export function supplementBaseline(records: SizeRecord[], baseline: BaselineFile): BaselineFile {
  // Validate against the old values before registering anything: adding a new file
  // must not grandfather a hard-limit violation or reset another file's ratchet.
  const failures = evaluate(records, baseline, false);
  if (failures.length) throw new Error(failures.join("\n"));
  return {
    ...baseline,
    files: Object.fromEntries([
      ...Object.entries(baseline.files),
      ...records.filter((record) => !baseline.files[record.path]).map((record) => [
        record.path, { total: record.total, production: record.production },
      ] as const),
    ].sort(([left], [right]) => left.localeCompare(right))),
  };
}

export function runSizeCommand(command: string | undefined, baselinePath: string): void {
  if (command !== undefined && command !== "check" && command !== "add-missing") {
    throw new Error("usage: bun scripts/module-size.ts check|add-missing (existing baselines must be reviewed explicitly)");
  }
  if (!existsSync(baselinePath)) throw new Error(`missing baseline: ${baselinePath}`);
  const records = collectSizes();
  const baseline = JSON.parse(readFileSync(baselinePath, "utf8")) as BaselineFile;
  if (command === "add-missing") {
    const next = supplementBaseline(records, baseline);
    writeFileSync(baselinePath, `${JSON.stringify(next, null, 2)}\n`);
    console.log(`registered ${Object.keys(next.files).length - Object.keys(baseline.files).length} files; existing values preserved`);
    return;
  }
  const failures = evaluate(records, baseline);
  if (failures.length) throw new Error(failures.join("\n"));
  console.log(`module-size ok (${records.length} files)`);
}
