import { expect, test } from "bun:test";
import type { RoutingEventRecord } from "../src/lib/generated/runtimeEvent";
import {
  advanceRoutingEventCursors,
  mergeRoutingEventRecords,
} from "../src/features/chat/routingEventReplay";

function event(rootId: string, seq: bigint, kind = "candidate_ready"): RoutingEventRecord {
  return { rootId, seq, kind, dataJson: "{}", createdAtMs: Number(seq) };
}

test("rr_14_subscribe_replay_race deduplicates live/replay overlap and advances each root", () => {
  const live = [event("root-a", 2n), event("root-b", 1n)];
  const replay = [event("root-a", 1n, "root_started"), event("root-a", 2n)];
  const merged = mergeRoutingEventRecords(live, replay);

  expect(merged.map(({ rootId, seq }) => `${rootId}:${seq}`)).toEqual([
    "root-a:1",
    "root-a:2",
    "root-b:1",
  ]);
  const cursors = advanceRoutingEventCursors(new Map([["root-a", 1n]]), merged);
  expect(cursors.get("root-a")).toBe(2n);
  expect(cursors.get("root-b")).toBe(1n);
});

test("rr_14_subscribe_replay_race keeps distinct roots with the same sequence", () => {
  expect(mergeRoutingEventRecords([event("root-a", 1n)], [event("root-b", 1n)])).toHaveLength(2);
});
