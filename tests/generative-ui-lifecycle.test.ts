import { expect, test } from "bun:test";
import { UiQueryCache } from "../src/features/chat/ui/queryCache";
const pause = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));
test("ten dashboards share a query and unmount stops polling", async () => {
  const cache = new UiQueryCache(15);
  let calls = 0;
  const fetch = async () => {
    calls++;
    return { capturedAt: "1", rows: [] };
  };
  const unsubscribes = Array.from({ length: 10 }, () =>
    cache.subscribe("conversation:source", fetch, () => {}),
  );
  await pause(2);
  expect(calls).toBe(1);
  expect(cache.size).toBe(1);
  unsubscribes.slice(0, 9).forEach((fn) => fn());
  await pause(20);
  expect(calls).toBe(2);
  unsubscribes[9]();
  const stopped = calls;
  await pause(30);
  expect(calls).toBe(stopped);
  expect(cache.size).toBe(0);
});
test("late results cannot resurrect a disposed subscription", async () => {
  const cache = new UiQueryCache(10);
  let resolve!: (value: { capturedAt: string; rows: [] }) => void;
  const unsubscribe = cache.subscribe(
    "a",
    () =>
      new Promise((done) => {
        resolve = done;
      }),
    () => {},
  );
  await Promise.resolve();
  unsubscribe();
  resolve({ capturedAt: "1", rows: [] });
  await pause(2);
  expect(cache.size).toBe(0);
  expect(cache.snapshot("a").data).toBeUndefined();
});
test("failures retain prior values with explicit stale/error status", async () => {
  const cache = new UiQueryCache(100_000);
  let fail = false;
  const unsubscribe = cache.subscribe(
    "a",
    async () => {
      if (fail) throw Error("offline");
      return { capturedAt: "7", rows: [{ count: 3 }] };
    },
    () => {},
  );
  await pause(2);
  fail = true;
  await cache.refresh("a");
  expect(cache.snapshot("a").data?.capturedAt).toBe("7");
  expect(cache.snapshot("a").error).toBe("unavailable");
  unsubscribe();
});
test("synchronous fetch failures release in-flight state so retry can recover", async () => {
  const cache = new UiQueryCache(100_000);
  let fail = true;
  const unsubscribe = cache.subscribe(
    "sync",
    () => {
      if (fail) throw Error("invoke unavailable");
      return Promise.resolve({ capturedAt: "8", rows: [] });
    },
    () => {},
  );
  await cache.refresh("sync");
  expect(cache.snapshot("sync").error).toBe("unavailable");
  fail = false;
  await cache.refresh("sync");
  expect(cache.snapshot("sync").data?.capturedAt).toBe("8");
  unsubscribe();
});
test("unsubscribing before a scheduled fetch prevents starting that fetch", async () => {
  const cache = new UiQueryCache(100_000);
  let calls = 0;
  const off = cache.subscribe(
    "cancelled",
    async () => {
      calls++;
      return { capturedAt: "0", rows: [] };
    },
    () => {},
  );
  off();
  await Promise.resolve();
  expect(calls).toBe(0);
  expect(cache.size).toBe(0);
});
