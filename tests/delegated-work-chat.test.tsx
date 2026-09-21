import { expect, test } from "bun:test";
import { MessageHistoryStore, type MessagePage } from "../src/features/chat/messageHistoryStore";

test("duplicate report events merge by message id", async () => {
  const pages: MessagePage[] = [];
  const store = new MessageHistoryStore(async () => {
    const page = pages[pages.length - 1] ?? {
      messages: [],
      hasMore: false,
      hasNewer: false,
      nextCursor: null,
      newerCursor: null,
    };
    return page;
  });
  store.reset("c1");
  const message = {
    id: "m1",
    conversationId: "c1",
    role: "assistant" as const,
    content: "done",
    createdAt: "1",
  };
  pages.push({
    messages: [message],
    hasMore: false,
    hasNewer: false,
    nextCursor: null,
    newerCursor: null,
  });
  await store.latest("c1");
  await store.latest("c1");
  expect(store.snapshot().messages.filter((entry) => entry.id === "m1")).toHaveLength(1);
});
