import { afterEach, expect, test } from "bun:test";
import { act } from "react";
import { StewardPanel } from "../src/features/coding/StewardPanel";
import { installJsdom } from "./jsdomGlobals";
import { invokeCalls, invokeImpl, resetTauriCoreMock } from "./tauriCoreMock";

afterEach(resetTauriCoreMock);

test("multiple Goals remain separately visible and one can be withdrawn", async () => {
  const env = installJsdom();
  const { createRoot } = await import("react-dom/client");
  const root = createRoot(document.getElementById("root")!);
  invokeImpl.handler = async (command) => {
    if (command === "list_steward_tasks") {
      return [
        {
          taskId: "task-a",
          loopState: "queued",
          dedupeKey: "a",
          codingJobId: null,
          goalStatus: "active",
          goalId: "goal-a",
          summary: "A の調査",
          verifier: "test_report_obtained",
          workspaceId: "workspace-a",
          operations: "read",
          budgetRuns: 1,
          budgetMs: 60_000,
          notify: "silent",
          deliveryState: null,
          speechState: null,
          artifactRefs: [],
        },
        {
          taskId: "task-b",
          loopState: "awaiting_dependency",
          dedupeKey: "b",
          codingJobId: null,
          goalStatus: "active",
          goalId: "goal-b",
          summary: "B の調査",
          verifier: "tests_pass",
          workspaceId: "workspace-b",
          operations: "read_test",
          budgetRuns: 2,
          budgetMs: 120_000,
          notify: "both",
          deliveryState: "pending",
          speechState: "pending",
          artifactRefs: ["job-b"],
        },
      ];
    }
    if (command === "work_withdraw") return { goalId: "goal-b", status: "withdrawn" };
    throw new Error(`unexpected command: ${command}`);
  };
  try {
    await act(async () => {
      root.render(
        <StewardPanel
          conversationId="conversation-a"
          workspaceId="workspace-a"
          onError={() => {}}
        />,
      );
    });
    await act(async () => {
      await Promise.resolve();
    });
    expect(document.body.textContent).toContain("A の調査");
    expect(document.body.textContent).toContain("B の調査");
    const withdrawButtons = [...document.querySelectorAll("button")].filter(
      (button) => button.textContent === "この Goal を撤回",
    );
    expect(withdrawButtons).toHaveLength(2);
    await act(async () => withdrawButtons[1]?.click());
    expect(invokeCalls).toContainEqual({
      command: "work_withdraw",
      args: { conversationId: "conversation-a", goalId: "goal-b" },
      options: undefined,
    });
  } finally {
    await act(async () => root.unmount());
    env.restore();
  }
});

test("confirmed registration sends only the selected bounded Goal scope", async () => {
  const env = installJsdom();
  const { createRoot } = await import("react-dom/client");
  const root = createRoot(document.getElementById("root")!);
  invokeImpl.handler = async (command) => {
    if (command === "list_steward_tasks") return [];
    if (command === "register_steward_goal") {
      return { goalId: "goal-c", delegationId: "delegation-c", status: "active" };
    }
    throw new Error(`unexpected command: ${command}`);
  };
  try {
    await act(async () => {
      root.render(
        <StewardPanel
          conversationId="conversation-c"
          workspaceId="workspace-c"
          onError={() => {}}
        />,
      );
    });
    await act(async () => {
      await Promise.resolve();
    });
    const inputs = document.querySelectorAll<HTMLInputElement>("input");
    const summary = inputs[0]!;
    const confirmation = document.querySelector<HTMLInputElement>('input[type="checkbox"]')!;
    await act(async () => {
      Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, "value")?.set?.call(
        summary,
        "対象テストを読む",
      );
      summary.dispatchEvent(new Event("input", { bubbles: true }));
      confirmation.click();
    });
    const register = [...document.querySelectorAll("button")].find(
      (button) => button.textContent === "Goal を登録",
    )!;
    expect(register.disabled).toBeFalse();
    await act(async () => register.click());
    expect(invokeCalls).toContainEqual({
      command: "register_steward_goal",
      args: {
        conversationId: "conversation-c",
        workspaceId: "workspace-c",
        successCondition: "tests pass",
        summary: "対象テストを読む",
        verifier: "test_report_obtained",
        operations: "read_test",
        budgetRuns: 3,
        budgetMs: 60_000,
        notify: "both",
      },
      options: undefined,
    });
  } finally {
    await act(async () => root.unmount());
    env.restore();
  }
});
