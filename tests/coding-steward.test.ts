import { afterEach, expect, test } from "bun:test";
import { invokeCalls, invokeImpl, resetTauriCoreMock } from "./tauriCoreMock";
import { stewardApi, stewardErrorMessage } from "../src/features/coding/stewardApi";

afterEach(resetTauriCoreMock);

test("the multiple-Goal limit is explained without a new IPC", () => {
  expect(stewardErrorMessage("active_goal_limit")).toContain("最大 8 件");
  expect(stewardErrorMessage("steward_register_invalid")).toContain("成功条件");
});

test("notification-only amendment targets exactly one Goal", async () => {
  invokeImpl.handler = async () => ({ goalId: "goal-a", status: "active", revisioned: true });
  await stewardApi.amendNotification("conversation-a", "goal-a", "silent");
  expect(invokeCalls).toEqual([
    {
      command: "work_amend",
      args: { conversationId: "conversation-a", goalId: "goal-a", notify: "silent" },
      options: undefined,
    },
  ]);
});
