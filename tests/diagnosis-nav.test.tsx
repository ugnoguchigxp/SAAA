import { expect, mock, test } from "bun:test";
import { act } from "react";
import { createRoot } from "react-dom/client";
import i18n from "../src/i18n";
import { invokeImpl, resetTauriCoreMock } from "./tauriCoreMock";
import { installJsdom } from "./jsdomGlobals";

mock.module("@tauri-apps/api/event", () => ({
  listen: async () => async () => undefined,
}));

const { TopNavigation } = await import("../src/shell/TopNavigation");

test("top navigation opens diagnosis", async () => {
  resetTauriCoreMock();
  invokeImpl.handler = async () => ({
    revision: 1,
    startedAt: "2026-09-22T00:00:00Z",
    finishedAt: "2026-09-22T00:00:01Z",
    running: false,
    overall: "ok",
    items: [],
  });
  const env = installJsdom();
  const root = createRoot(document.getElementById("root")!);
  await i18n.changeLanguage("ja");
  try {
    await act(async () => {
      root.render(<TopNavigation active="conversation" onChange={() => undefined} />);
    });
    const button = [...document.querySelectorAll("button")].find(
      (candidate) => candidate.textContent === "自己診断",
    );
    expect(button).toBeTruthy();
    await act(async () => button!.click());
    expect(document.body.querySelector("[role=dialog]")).toBeTruthy();
  } finally {
    await act(() => root.unmount());
    env.dom.window.close();
    env.restore();
  }
});
