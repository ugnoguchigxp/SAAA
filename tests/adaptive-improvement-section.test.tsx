import { afterEach, expect, test } from "bun:test";
import { act } from "react";
import { AdaptiveImprovementSection } from "../src/features/settings/AdaptiveImprovementSection";
import { installJsdom } from "./jsdomGlobals";
import { invokeImpl, resetTauriCoreMock } from "./tauriCoreMock";

afterEach(resetTauriCoreMock);

test("adaptive improvement section shows missing evaluation reason", async () => {
  const env = installJsdom();
  const { createRoot } = await import("react-dom/client");
  const root = createRoot(document.getElementById("root")!);
  invokeImpl.handler = async (command) => {
    if (command === "list_adaptive_evaluations") {
      return [
        {
          artifactId: "aia-1",
          domain: "plan",
          scopeKey: "goal",
          state: "evaluated",
          stopReason: "examples",
          examples: 12,
          groups: 4,
          successLower: -0.1,
          policyRevision: null,
        },
      ];
    }
    throw new Error(`unexpected ${command}`);
  };
  try {
    await act(async () => {
      root.render(<AdaptiveImprovementSection />);
    });
    await act(async () => {
      await Promise.resolve();
    });
    expect(document.body.textContent).toContain("実測が不足しているか");
    const buttons = [...document.querySelectorAll("button")];
    expect(buttons.find((button) => button.textContent === "有効化")?.disabled).toBe(true);
  } finally {
    await act(async () => root.unmount());
    env.restore();
  }
});

test("eligible artifact can be activated without a prior policy revision", async () => {
  const env = installJsdom();
  const { createRoot } = await import("react-dom/client");
  const root = createRoot(document.getElementById("root")!);
  invokeImpl.handler = async (command, args) => {
    if (command === "list_adaptive_evaluations") {
      return [
        {
          artifactId: "aia-2",
          domain: "plan",
          scopeKey: "goal",
          state: "eligible",
          stopReason: null,
          examples: 30,
          groups: 6,
          successLower: 0.2,
          policyRevision: null,
        },
      ];
    }
    if (command === "activate_adaptive_artifact") {
      expect(args).toEqual({
        input: { artifactId: "aia-2", expectedRevision: 1 },
      });
      return;
    }
    throw new Error(`unexpected ${command}`);
  };
  try {
    await act(async () => {
      root.render(<AdaptiveImprovementSection />);
    });
    await act(async () => {
      await Promise.resolve();
    });
    const activate = [...document.querySelectorAll("button")].find(
      (button) => button.textContent === "有効化",
    );
    expect(activate?.disabled).toBe(false);
    await act(async () => {
      (activate as HTMLButtonElement | undefined)?.click();
    });
  } finally {
    await act(async () => root.unmount());
    env.restore();
  }
});
