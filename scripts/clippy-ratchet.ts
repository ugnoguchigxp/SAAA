import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

const ROOT = fileURLToPath(new URL("..", import.meta.url));
const BASELINE = fileURLToPath(new URL("./clippy-warning-baseline.json", import.meta.url));

export type WarningCounts = Record<string, number>;

export function warningCounts(output: string): WarningCounts {
  const counts: WarningCounts = {};
  for (const line of output.split("\n")) {
    if (!line.startsWith("{")) continue;
    let item: {
      reason?: string;
      package_id?: string;
      message?: {
        level?: string;
        code?: { code?: string } | null;
        message?: string;
        spans?: Array<{ file_name?: string; is_primary?: boolean }>;
      };
    };
    try {
      item = JSON.parse(line);
    } catch {
      continue;
    }
    if (
      item.reason !== "compiler-message" ||
      !item.package_id?.includes("#saaa@") ||
      item.message?.level !== "warning"
    ) continue;
    const primary = item.message.spans?.find((span) => span.is_primary);
    if (!primary?.file_name || !item.message.message) continue;
    const key = [primary.file_name, item.message.code?.code ?? "unknown", item.message.message]
      .join(" | ");
    counts[key] = (counts[key] ?? 0) + 1;
  }
  return counts;
}

export function newWarnings(actual: WarningCounts, baseline: WarningCounts): string[] {
  return Object.entries(actual)
    .filter(([key, count]) => count > (baseline[key] ?? 0))
    .map(([key, count]) => `${key} (${count}, baseline ${baseline[key] ?? 0})`)
    .sort();
}

if (import.meta.main) {
  const child = Bun.spawn(
    ["cargo", "clippy", "--manifest-path", "src-tauri/Cargo.toml", "--all-targets", "--message-format=json"],
    { cwd: ROOT, stdout: "pipe", stderr: "inherit" },
  );
  const output = await new Response(child.stdout).text();
  const status = await child.exited;
  if (status !== 0) throw new Error(`Clippy failed with exit status ${status}`);
  const baseline = JSON.parse(readFileSync(BASELINE, "utf8")) as {
    warnings: WarningCounts;
  };
  const actual = warningCounts(output);
  const added = newWarnings(actual, baseline.warnings);
  if (added.length) throw new Error(`New Clippy warnings:\n${added.join("\n")}`);
  console.log(`Clippy ratchet passed (${Object.values(actual).reduce((a, b) => a + b, 0)} existing warnings, no new warnings).`);
}
