import { expect, test } from "bun:test";

const corpus = await Bun.file(
  new URL("../spec/evidence/delegated-work/natural-language-cases.md", import.meta.url),
).text();

function entries(heading: string) {
  const start = corpus.indexOf(`## ${heading}`);
  const next = corpus.indexOf("\n## ", start + 1);
  return corpus.slice(start, next === -1 ? undefined : next).match(/^\d+\. .+$/gm) ?? [];
}

test("delegated-work acceptance corpus keeps explicit and non-adoption coverage", () => {
  const explicit = entries("Explicit delegated-work requests (20)");
  const rejected = entries("Non-adoption / confirmation cases (10)");
  expect(explicit).toHaveLength(20);
  expect(rejected).toHaveLength(10);
  expect(explicit.slice(0, 17).every((entry) => entry.includes("`work_propose`"))).toBeTrue();
  expect(explicit[17]).toContain("対象確認");
  expect(explicit[18]).toContain("`work_withdraw`");
  expect(explicit[19]).toContain("`work_status`");
});
