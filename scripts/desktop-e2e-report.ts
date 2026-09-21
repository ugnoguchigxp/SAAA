import { readFileSync } from "node:fs";

export function readDesktopE2EChecks(
  marker: string,
  requiredChecks: string[],
): Record<string, boolean> {
  let report: unknown;
  try {
    report = JSON.parse(readFileSync(marker, "utf8"));
  } catch (cause) {
    throw new Error(
      `Desktop E2E marker is not valid JSON: ${cause instanceof Error ? cause.message : String(cause)}`,
    );
  }
  const candidate =
    report && typeof report === "object" && "checks" in report
      ? (report as { checks?: unknown }).checks
      : undefined;
  if (!candidate || typeof candidate !== "object")
    throw new Error("Desktop E2E marker has no checks object");
  const checks = Object.fromEntries(
    Object.entries(candidate).filter(
      (entry): entry is [string, boolean] => entry[1] === true || entry[1] === false,
    ),
  );
  const failed = requiredChecks.filter((name) => checks[name] !== true);
  if (failed.length) throw new Error(`Desktop E2E checks failed: ${failed.join(", ")}`);
  return checks;
}
