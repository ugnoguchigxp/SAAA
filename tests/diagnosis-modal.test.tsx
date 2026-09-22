import { afterEach, expect, mock, test } from "bun:test";
import { act } from "react";
import type { Root } from "react-dom/client";
import i18n from "../src/i18n";
import { invokeImpl, resetTauriCoreMock } from "./tauriCoreMock";
import { installJsdom } from "./jsdomGlobals";

mock.module("@tauri-apps/api/event", () => ({
  listen: async () => async () => undefined,
}));

const { DiagnosisModal } = await import("../src/features/diagnosis/DiagnosisModal");

const ready = {
  revision: 1,
  startedAt: "2026-09-22T00:00:00Z",
  finishedAt: "2026-09-22T00:00:01Z",
  running: false,
  overall: "warn",
  items: [
    {
      id: "sqlite",
      group: "storage",
      label: "SQLite",
      status: "ok",
      severity: "fatal",
      message: "",
      latencyMs: 3,
    },
    {
      id: "settings.providers",
      group: "settings",
      label: "Model providers",
      status: "warn",
      severity: "fatal",
      message: "No model providers are configured",
      latencyMs: null,
    },
  ],
};

let root: Root | null = null;
let restore: (() => void) | null = null;

afterEach(async () => {
  await act(async () => root?.unmount());
  root = null;
  restore?.();
  restore = null;
});

async function renderModal(report: typeof ready, onClose = () => undefined) {
  resetTauriCoreMock();
  invokeImpl.handler = async () => report;
  restore = installJsdom().restore;
  await i18n.changeLanguage("ja");
  const { createRoot } = await import("react-dom/client");
  const { createElement } = await import("react");
  root = createRoot(document.getElementById("root")!);
  await act(async () => root!.render(createElement(DiagnosisModal, { onClose })));
  await act(async () => {
    await Promise.resolve();
  });
}

test("diagnosis modal shows grouped items, blocks rerun while running, and closes on escape", async () => {
  let closed = 0;
  await renderModal(ready, () => {
    closed += 1;
  });
  expect(document.querySelector('[role="dialog"]')).toBeTruthy();
  expect(document.querySelectorAll(".diagnosis-modal-group")).toHaveLength(2);
  expect(document.body.textContent).toContain("データベース");
  expect(document.body.textContent).toContain("モデルプロバイダ設定");
  const rerun = [...document.querySelectorAll("button")].find((button) =>
    button.textContent?.includes("再診断"),
  );
  expect(rerun?.hasAttribute("disabled")).toBe(false);
  window.dispatchEvent(new window.KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
  expect(closed).toBe(1);

  await act(async () => root?.unmount());
  await renderModal({ ...ready, running: true, overall: "running" });
  const busy = [...document.querySelectorAll("button")].find((button) =>
    button.textContent?.includes("診断中"),
  );
  expect(busy?.hasAttribute("disabled")).toBe(true);
});
