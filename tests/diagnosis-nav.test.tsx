import { expect, test } from "bun:test";
import { act } from "react";
import { createRoot } from "react-dom/client";
import i18n from "../src/i18n";
import { resetTauriCoreMock } from "./tauriCoreMock";
import { installJsdom } from "./jsdomGlobals";
import { TopNavigation } from "../src/shell/TopNavigation";

test("top navigation selects the diagnosis screen", async () => {
  resetTauriCoreMock();
  const env = installJsdom();
  const root = createRoot(document.getElementById("root")!);
  const selected: string[] = [];
  await i18n.changeLanguage("ja");
  try {
    await act(async () => {
      root.render(
        <TopNavigation
          active="conversation"
          onChange={(route) => {
            selected.push(route);
          }}
        />,
      );
    });
    const button = [...document.querySelectorAll("button")].find(
      (candidate) => candidate.textContent === "自己診断",
    );
    expect(button).toBeTruthy();
    await act(async () => button!.click());
    expect(selected).toEqual(["diagnosis"]);
    expect(document.body.querySelector("[role=dialog]")).toBeNull();
  } finally {
    await act(() => root.unmount());
    env.dom.window.close();
    env.restore();
  }
});
