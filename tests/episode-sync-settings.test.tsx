import { afterEach, beforeEach, expect, test } from "bun:test";
import { act, createElement } from "react";
import type { Root } from "react-dom/client";
import { installJsdom } from "./jsdomGlobals";
import { invokeCalls, invokeImpl, resetTauriCoreMock } from "./tauriCoreMock";
const { EpisodeSyncSection } = await import("../src/features/settings/EpisodeSyncSection");
let root: Root | null = null;
let restore: (() => void) | null = null;
let enabled = false;
let fail = false;
beforeEach(() => {
  resetTauriCoreMock();
  enabled = false;
  fail = false;
  invokeImpl.handler = async (command, args) => {
    if (command === "set_episode_sync_scope") {
      if (fail) throw new Error("同期許可を保存できませんでした");
      enabled = (args as { enabled: boolean }).enabled;
    }
    return {
      consolidationEnabled: false,
      scopes: [{ scope: "user:fixture", name: "fixture", enabled }],
    };
  };
});
afterEach(async () => {
  await act(async () => root?.unmount());
  root = null;
  restore?.();
  restore = null;
});
async function render() {
  restore = installJsdom().restore;
  const { createRoot } = await import("react-dom/client");
  root = createRoot(document.getElementById("root")!);
  await act(async () => {
    root!.render(createElement(EpisodeSyncSection));
  });
}
test("grant is initially off and mounting only reads state", async () => {
  await render();
  expect((document.querySelector("input") as HTMLInputElement).checked).toBe(false);
  expect(invokeCalls.map((c) => c.command)).toEqual(["episode_sync_status"]);
  await act(async () => {
    (document.querySelector("input") as HTMLElement).click();
  });
  expect(invokeCalls[1]).toEqual({
    command: "set_episode_sync_scope",
    args: { scope: "user:fixture", enabled: true },
    options: undefined,
  });
  expect((document.querySelector("input") as HTMLInputElement).checked).toBe(true);
});
test("failed consent write preserves the old selection", async () => {
  fail = true;
  await render();
  await act(async () => {
    (document.querySelector("input") as HTMLElement).click();
  });
  expect((document.querySelector("input") as HTMLInputElement).checked).toBe(false);
  expect(document.querySelector("[role=alert]")?.textContent).toContain("保存できません");
});
