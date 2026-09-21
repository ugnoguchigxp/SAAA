import { expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { join } from "node:path";

test("dw_r21_tool_intake_matrix", () => {
  const corpus = JSON.parse(
    readFileSync(join(import.meta.dir, "fixtures/delegated-work/requests.json"), "utf8"),
  ) as { cases: Array<{ intent: string; expect: string; lane: string }> };
  const requests = corpus.cases.filter((entry) => entry.intent === "request");
  const rejected = corpus.cases.filter((entry) =>
    ["reject", "clarify"].includes(entry.intent),
  );
  expect(requests.length).toBeGreaterThanOrEqual(20);
  expect(rejected.length).toBeGreaterThanOrEqual(10);
  expect(corpus.cases.some((entry) => entry.expect === "chat")).toBeTrue();
});
