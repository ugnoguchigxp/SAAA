import { afterEach, expect, test } from "bun:test";
import { act } from "react";
import { DelegationConfirmation } from "../src/features/coding/DelegationConfirmation";
import { TestRecipeForm } from "../src/features/coding/TestRecipeForm";
import { installJsdom } from "./jsdomGlobals";
import { invokeCalls, invokeImpl, resetTauriCoreMock } from "./tauriCoreMock";

afterEach(resetTauriCoreMock);

test("confirmation and recipe forms send host contracts", async () => {
  const env = installJsdom();
  const { createRoot } = await import("react-dom/client");
  const root = createRoot(document.getElementById("root")!);
  invokeImpl.handler = async (command) => {
    if (command === "work_confirm") return { decision: "accepted" };
    if (command === "register_steward_recipe") return { recipeId: "recipe-1", revision: 1 };
    throw new Error(command);
  };
  try {
    await act(async () => {
      root.render(
        <>
          <DelegationConfirmation
            conversationId="c1"
            proposalId="p1"
            digest="abc"
            revision={1}
            onDone={() => {}}
          />
          <TestRecipeForm onRegistered={() => {}} />
        </>,
      );
    });
    const start = [...document.querySelectorAll("button")].find(
      (button) => button.textContent === "登録して開始",
    );
    await act(async () => start?.click());
    expect(invokeCalls.some((call) => call.command === "work_confirm")).toBeTrue();
    await act(async () => {
      document
        .querySelector("form")
        ?.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
    });
  } finally {
    await act(async () => root.unmount());
    env.restore();
  }
});
