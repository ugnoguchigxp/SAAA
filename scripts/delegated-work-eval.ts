import { readFileSync } from "node:fs";
import { join } from "node:path";

const corpus = JSON.parse(
  readFileSync(join(import.meta.dir, "../tests/fixtures/delegated-work/requests.json"), "utf8"),
) as {
  cases: Array<{ id: string; lane: string; expect: string }>;
};

const live = process.env.SAAA_DELEGATED_LIVE === "1";
const selected = corpus.cases.filter((entry) =>
  live ? entry.lane === "live" : entry.lane === "scripted",
);
if (selected.length < 20 && !live) {
  throw new Error("scripted corpus must include 20+ explicit work cases");
}
console.log(
  JSON.stringify(
    {
      lane: live ? "live" : "scripted",
      count: selected.length,
      ids: selected.map((entry) => entry.id),
    },
    null,
    2,
  ),
);
