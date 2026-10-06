import { expect, test } from "bun:test";
import {
  age,
  isStale,
  sortCapabilities,
  sourceKey,
  STALE_REPORT_MS,
  summarize,
} from "../src/features/diagnosis/diagnosisModel";
import { capability, mixed, NOW, report } from "./diagnosisFixtures";

test("age buckets time without ever reporting a missing time as recent", () => {
  expect(age(NOW, null)).toEqual({ unit: "never", count: 0 });
  expect(age(NOW, NOW - 10_000).unit).toBe("now");
  expect(age(NOW, NOW - 5 * 60_000)).toEqual({ unit: "minutes", count: 5 });
  expect(age(NOW, NOW - 3 * 3_600_000)).toEqual({ unit: "hours", count: 3 });
  expect(age(NOW, NOW - 50 * 3_600_000)).toEqual({ unit: "days", count: 2 });
  expect(age(NOW, NOW + 5_000).unit).toBe("now");
});

test("summary leaves out disabled and unproven optional capabilities", () => {
  expect(summarize(mixed)).toEqual({
    counted: 6,
    ready: 3,
    degraded: 1,
    unavailable: 1,
    unverified: 1,
  });
});

test("problems sort before unproven and healthy, optional ones last", () => {
  expect(sortCapabilities(mixed.capabilities).map((item) => item.capability)).toEqual([
    "voice-listen",
    "memory",
    "conversation",
    "storage",
    "voice-speak",
    "voice-echo",
    "coding",
  ]);
  const same = [capability("storage", "ready"), capability("memory", "ready")];
  expect(sortCapabilities(same).map((item) => item.capability)).toEqual(["storage", "memory"]);
});

test("source ids become flat i18n keys", () => {
  expect(sourceKey("context-still.recall")).toBe("context_still_recall");
  expect(sourceKey("harness.session.deadline")).toBe("harness_session_deadline");
});

test("a report is stale when never run, unfinished or old", () => {
  expect(isStale(null, NOW)).toBe(true);
  expect(isStale(report("ready", [], { revision: 0 }), NOW)).toBe(true);
  expect(isStale(report("ready", [], { finishedAt: null }), NOW)).toBe(true);
  expect(isStale(report("ready", [], { finishedAt: NOW - STALE_REPORT_MS - 1 }), NOW)).toBe(true);
  expect(isStale(report("ready", [], { finishedAt: NOW - 1_000 }), NOW)).toBe(false);
});
