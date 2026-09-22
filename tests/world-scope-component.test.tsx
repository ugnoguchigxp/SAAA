import { expect, test } from "bun:test";
import { act } from "react";
import { installJsdom } from "./jsdomGlobals";
import { WorldScopeSelector } from "../src/features/chat/WorldScopeSelector";
import { CompletedMessage } from "../src/features/chat/ChatMessages";
import type { WorldContextStatus } from "../src/lib/generated/runtimeEvent";

test("wr_t20_select_B_keeps_A_answer_attached_to_A_and_deleted_selection_visible", async () => {
  const env = installJsdom();
  const { createRoot } = await import("react-dom/client");
  const root = createRoot(document.getElementById("root")!);
  const status: WorldContextStatus = {
    choices: ["a", "b"].map((id) => ({
      key: `project:${id}`,
      label: `同名 (${id})`,
      refs: [{ kind: "project", id, relation: "focus" }],
    })),
    messageScopes: { answerA: ["project:a"] },
    latestScopeKeys: ["project:a"],
    latestProvider: "fixture",
    delivery: "sent",
    omissionReason: null,
  };
  let selected = "project:a";
  const render = (value: WorldContextStatus) => (
    <>
      <WorldScopeSelector
        status={value}
        value={selected}
        onChange={(key) => {
          selected = key;
        }}
      />
      <CompletedMessage
        message={{
          id: "answerA",
          conversationId: "c",
          role: "assistant",
          content: "Aの状態",
          createdAt: "1",
          parts: null,
        }}
      />
    </>
  );
  try {
    await act(async () => root.render(render(status)));
    const select = document.querySelector("select")!;
    await act(async () => {
      select.value = "project:b";
      select.dispatchEvent(new Event("change", { bubbles: true }));
    });
    expect(selected).toBe("project:b");
    await act(async () => root.render(render({ ...status, latestScopeKeys: ["project:b"] })));
    expect(document.body.textContent).not.toContain("project:a");
    expect(document.body.textContent).toContain("Aの状態");
    await act(async () => root.render(render({ ...status, choices: [] })));
    expect(document.querySelector("select")?.textContent).toContain("対象の再確認が必要");
  } finally {
    await act(async () => root.unmount());
    env.restore();
  }
});
