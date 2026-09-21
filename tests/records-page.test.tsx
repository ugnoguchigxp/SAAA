import { afterEach, expect, test } from "bun:test";
import { act } from "react";
import { RecordsPage } from "../src/features/records/RecordsPage";
import { installJsdom } from "./jsdomGlobals";
import { invokeImpl, resetTauriCoreMock } from "./tauriCoreMock";

afterEach(resetTauriCoreMock);

test("records use a date index and dense conversation table", async () => {
  const env = installJsdom();
  const { createRoot } = await import("react-dom/client");
  const root = createRoot(document.getElementById("root")!);
  invokeImpl.handler = async (command) => {
    if (command !== "list_messages") throw new Error(`unexpected command: ${command}`);
    return {
      messages: [
        {
          id: "m1",
          conversationId: "c1",
          role: "user",
          content: "最初の相談",
          createdAt: new Date(2026, 8, 20, 9, 10).getTime().toString(),
          parts: null,
        },
        {
          id: "m2",
          conversationId: "c1",
          role: "assistant",
          content: "最初の回答",
          createdAt: new Date(2026, 8, 20, 9, 11).getTime().toString(),
          parts: null,
        },
        {
          id: "m3",
          conversationId: "c1",
          role: "user",
          content: "翌日の相談",
          createdAt: new Date(2026, 8, 21, 10, 20).getTime().toString(),
          parts: null,
        },
      ],
      hasMore: false,
      nextCursor: null,
    };
  };
  try {
    await act(async () => {
      root.render(
        <RecordsPage
          conversations={[
            {
              id: "c1",
              title: "会話",
              taskMode: "conversation",
              createdAt: "1",
              updatedAt: "2",
            },
          ]}
          initialConversationId="c1"
          targetMessageId={null}
          onTargetHandled={() => {}}
        />,
      );
      await new Promise((resolve) => setTimeout(resolve, 0));
    });
    expect(document.querySelectorAll(".records-date-list button")).toHaveLength(2);
    expect(document.querySelector(".records-table")).not.toBeNull();
    expect(document.querySelectorAll(".records-table tbody tr")).toHaveLength(1);
    expect(document.body.textContent).toContain("翌日の相談");
  } finally {
    await act(async () => root.unmount());
    env.restore();
  }
});
