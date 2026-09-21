import type { RoutingEventRecord } from "../../lib/generated/runtimeEvent";

export type RoutingEventCursors = Map<string, bigint>;

/** Merges replay and live-triggered reads by the durable `(rootId, seq)` identity. */
export function mergeRoutingEventRecords(
  current: readonly RoutingEventRecord[],
  incoming: readonly RoutingEventRecord[],
): RoutingEventRecord[] {
  const records = new Map<string, RoutingEventRecord>();
  for (const event of [...current, ...incoming]) {
    records.set(`${event.rootId}\u0000${event.seq}`, event);
  }
  return [...records.values()].sort((left, right) => {
    if (left.rootId !== right.rootId) return left.rootId.localeCompare(right.rootId);
    return left.seq < right.seq ? -1 : left.seq > right.seq ? 1 : 0;
  });
}

export function advanceRoutingEventCursors(
  cursors: RoutingEventCursors,
  events: readonly RoutingEventRecord[],
): RoutingEventCursors {
  const next = new Map(cursors);
  for (const event of events) {
    const previous = next.get(event.rootId) ?? 0n;
    if (event.seq > previous) next.set(event.rootId, event.seq);
  }
  return next;
}
