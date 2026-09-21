import { expect, test } from "bun:test";
import { mergeMessageWindow } from "../src/features/chat/messageWindow";
import type { ConversationMessage } from "../src/lib/contracts";
function rows(start: number, count: number): ConversationMessage[] {
  return Array.from({ length: count }, (_, i) => ({
    id: `m${String(start + i).padStart(6, "0")}`,
    createdAt: String(start + i),
    conversationId: "c",
    role: "user",
    content: "text",
  }));
}
test("window stays bounded across forward/backward pagination and deduplicates overlaps", () => {
  let window = rows(1000, 30);
  for (let page = 0; page < 20; page++)
    window = mergeMessageWindow(window, rows(970 - page * 30, 30), "before");
  expect(window).toHaveLength(150);
  expect(new Set(window.map((m) => m.id)).size).toBe(150);
  const first = window[0].id;
  window = mergeMessageWindow(
    window,
    rows(Number(window[window.length - 1].createdAt) + 1, 30),
    "after",
  );
  expect(window).toHaveLength(150);
  expect(window[0].id).not.toBe(first);
  expect(mergeMessageWindow(window, window, "after")).toHaveLength(150);
});

import { MessageHistoryStore, type MessagePage } from "../src/features/chat/messageHistoryStore";
const page = (
  start: number,
  count: number,
  hasMore = start > 0,
  hasNewer = false,
): MessagePage => ({
  messages: rows(start, count),
  hasMore,
  hasNewer,
  nextCursor: null,
  newerCursor: null,
});
function queuedHistory() {
  const pending: {
    cursor: string | null;
    resolve: (value: MessagePage) => void;
    reject: (error: Error) => void;
  }[] = [];
  const store = new MessageHistoryStore(
    (_conversation, cursor) =>
      new Promise((resolve, reject) => pending.push({ cursor, resolve, reject })),
  );
  store.reset("c");
  return { store, pending };
}
test("prepending without eviction stays at the newest edge and preserves the beginning on refresh", async () => {
  const { store, pending } = queuedHistory();
  const first = store.latest("c");
  pending.shift()!.resolve(page(30, 30));
  await first;
  const older = store.load("before");
  pending.shift()!.resolve(page(0, 30, false, true));
  await older;
  expect(store.snapshot().hasNewerMessages).toBe(false);
  const refresh = store.latest("c");
  pending.shift()!.resolve(page(30, 30));
  await refresh;
  expect(store.snapshot().hasMoreMessages).toBe(false);
  expect(store.snapshot().messages).toHaveLength(60);
});
test("failed refresh does not strand an in-flight pagination request", async () => {
  const { store, pending } = queuedHistory();
  const first = store.latest("c");
  pending.shift()!.resolve(page(60, 30));
  await first;
  const older = store.load("before");
  const oldRequest = pending.shift()!;
  const refresh = store.latest("c");
  pending.shift()!.reject(Error("offline"));
  await expect(refresh).rejects.toThrow("offline");
  oldRequest.resolve(page(30, 30, true, true));
  await older;
  expect(store.snapshot().loadingOlderMessages).toBe(false);
  const next = store.load("before");
  expect(pending).toHaveLength(1);
  pending.shift()!.resolve(page(0, 30, false, true));
  await next;
});
test("refresh after a long gap replaces the window instead of creating an unreachable hole", async () => {
  const { store, pending } = queuedHistory();
  const first = store.latest("c");
  pending.shift()!.resolve(page(0, 30, false));
  await first;
  const refresh = store.latest("c");
  pending.shift()!.resolve(page(100, 30));
  await refresh;
  expect(store.snapshot().messages).toEqual(rows(100, 30));
  expect(store.snapshot().hasMoreMessages).toBe(true);
});
test("conversation switch ignores late pages and resets edge flags synchronously", async () => {
  const { store, pending } = queuedHistory();
  const old = store.latest("c");
  const oldRequest = pending.shift()!;
  store.reset("other");
  const current = store.latest("other");
  pending.shift()!.resolve(page(200, 30));
  await current;
  oldRequest.resolve(page(0, 30));
  await old;
  expect(store.snapshot().messages[0].createdAt).toBe("200");
  expect(store.isBrowsingOlder()).toBe(false);
});
test("new completion overlap cannot bridge an unobserved history gap", async () => {
  const { store, pending } = queuedHistory();
  const first = store.latest("c");
  pending.shift()!.resolve(page(0, 30, false));
  await first;
  store.setMessages((current) => [...current, ...rows(129, 1)]);
  const refresh = store.latest("c");
  pending.shift()!.resolve(page(100, 30));
  await refresh;
  expect(store.snapshot().messages).toEqual(rows(100, 30));
});
test("a newly appended message exposes the evicted oldest row through pagination", () => {
  const { store } = queuedHistory();
  store.setMessages(rows(0, 150));
  store.setMessages((current) => [...current, ...rows(150, 1)]);
  expect(store.snapshot().messages).toHaveLength(150);
  expect(store.snapshot().hasMoreMessages).toBe(true);
});
test("older eviction sets a real newer edge and refresh preserves the browsing window", async () => {
  const { store, pending } = queuedHistory();
  store.setMessages(rows(100, 150));
  const old = store.load("before");
  pending.shift()!.resolve(page(70, 30, true, true));
  await old;
  const before = store.snapshot().messages;
  expect(store.isBrowsingOlder()).toBe(true);
  const refresh = store.latest("c");
  pending.shift()!.resolve(page(230, 30));
  await refresh;
  expect(store.snapshot().messages).toEqual(before);
  const newer = store.load("after");
  pending.shift()!.resolve(page(220, 30, true, false));
  await newer;
  expect(store.isBrowsingOlder()).toBe(false);
  expect(store.snapshot().hasMoreMessages).toBe(true);
});

test("returning to latest replaces an older browsing window in one request", async () => {
  const { store, pending } = queuedHistory();
  store.setMessages(rows(100, 150));
  const older = store.load("before");
  pending.shift()!.resolve(page(70, 30, true, true));
  await older;
  expect(store.isBrowsingOlder()).toBe(true);

  const latest = store.returnLatest();
  expect(pending[0]?.cursor).toBeNull();
  pending.shift()!.resolve(page(230, 30, true, false));
  await latest;

  expect(store.snapshot().messages).toEqual(rows(230, 30));
  expect(store.snapshot().hasNewerMessages).toBe(false);
  expect(store.snapshot().loadingNewerMessages).toBe(false);
});
