import { afterEach, expect, mock, test } from "bun:test";
import { act } from "react";
import type { Root } from "react-dom/client";
import i18n from "../src/i18n";
import { invokeImpl, resetTauriCoreMock } from "./tauriCoreMock";
import { installJsdom } from "./jsdomGlobals";

mock.module("@tauri-apps/api/event", () => ({
  listen: async () => async () => undefined,
}));

const { DiagnosisPage } = await import("../src/features/diagnosis/DiagnosisPage");

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
    {
      id: "harness.reachability",
      group: "harness",
      label: "Harness reachability",
      status: "ok",
      severity: "degraded",
      message: "Agent connection and model readiness probe succeeded",
      latencyMs: 12,
    },
    {
      id: "harness.llm",
      group: "harness",
      label: "Harness LLM",
      status: "skipped",
      severity: "info",
      message: "llm is not advertised",
      latencyMs: null,
    },
    {
      id: "harness.embedding",
      group: "harness",
      label: "Harness embedding",
      status: "ok",
      severity: "degraded",
      message: "Embedding request returned a vector",
      latencyMs: 20,
    },
    {
      id: "harness.tts",
      group: "harness",
      label: "Harness TTS",
      status: "fail",
      severity: "degraded",
      message: "Capability is not advertised by this Harness",
      latencyMs: null,
    },
    {
      id: "provider.system-tts",
      group: "voice",
      label: "System Voice",
      status: "ok",
      severity: "degraded",
      message: "System text-to-speech is available",
      latencyMs: 4,
    },
    {
      id: "provider.deepseek",
      group: "llm",
      label: "DeepSeek V4.1 Flash",
      status: "fail",
      severity: "degraded",
      message: "DeepSeek V4.1 Flash: Provider request failed",
      latencyMs: null,
    },
    {
      id: "provider.codex-sdk",
      group: "llm",
      label: "Codex SDK",
      status: "ok",
      severity: "degraded",
      message: "Codex SDK: gpt-5.6-luna is ready",
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

async function renderPage(report: typeof ready | null) {
  resetTauriCoreMock();
  invokeImpl.handler = async (command) => {
    if (command === "run_diagnosis") return report;
    return null;
  };
  restore = installJsdom().restore;
  await i18n.changeLanguage("ja");
  const { createRoot } = await import("react-dom/client");
  const { createElement } = await import("react");
  root = createRoot(document.getElementById("root")!);
  await act(async () => root!.render(createElement(DiagnosisPage)));
  await act(async () => {
    await Promise.resolve();
  });
}

test("diagnosis page shows stages, item list, and blocks rerun while running", async () => {
  await renderPage(ready);
  expect(document.body.textContent).toContain("診断開始");
  expect(document.body.textContent).not.toContain("データベース");
  const start = [...document.querySelectorAll("button")].find((button) =>
    button.textContent?.includes("診断開始"),
  );
  await act(async () => {
    start?.click();
    await Promise.resolve();
  });
  expect(document.querySelector('[role="dialog"]')).toBeNull();
  const services = document.querySelector(".diagnosis-services")?.textContent ?? "";
  const response = [...document.querySelectorAll(".diagnosis-services article")].find(
    (card) => card.querySelector("strong")?.textContent === "応答",
  );
  expect(response?.textContent).toContain("正常");
  expect(response?.textContent).not.toContain("対象外");
  expect(services).toContain("応答");
  expect(services).not.toContain("音声認識");
  const speech = [...document.querySelectorAll(".diagnosis-services article")].find(
    (card) => card.querySelector("strong")?.textContent === "音声合成",
  );
  expect(speech?.textContent).toContain("正常");
  expect(speech?.textContent).not.toContain("失敗");
  expect(services).toContain("音声合成");
  const embedding = [...document.querySelectorAll(".diagnosis-services article")].find(
    (card) => card.querySelector("strong")?.textContent === "埋め込み",
  );
  expect(embedding?.textContent).toContain("正常");
  expect(embedding?.textContent).not.toContain("対象外");
  expect(services).toContain("埋め込み");
  expect(services).not.toContain("保存");
  expect(services).not.toContain("DeepSeek");
  expect(services).not.toContain("Codex SDK");
  expect(document.querySelector(".diagnosis-foundation")).toBeNull();
  const state = document.querySelector(".diagnosis-state")?.textContent ?? "";
  expect(state).toContain("データベース");
  expect(state).not.toContain("個人状態");
  expect(state).not.toContain("ワールドモデル");
  expect(state).not.toContain("ToolChain");
  const reasoning = document.querySelector(".diagnosis-reasoning")?.textContent ?? "";
  expect(reasoning).toContain("DeepSeek");
  expect(reasoning).toContain("Codex SDK");
  expect(document.body.textContent).toContain("一部に注意があります");
  expect(document.body.textContent).toContain("データベース");
  expect(document.body.textContent).toContain("モデルプロバイダ設定");
  const rerun = [...document.querySelectorAll("button")].find((button) =>
    button.textContent?.includes("再診断"),
  );
  expect(rerun?.hasAttribute("disabled")).toBe(false);

  await act(async () => root?.unmount());
  resetTauriCoreMock();
  invokeImpl.handler = async () => new Promise(() => undefined);
  restore = installJsdom().restore;
  const { createRoot } = await import("react-dom/client");
  const { createElement } = await import("react");
  root = createRoot(document.getElementById("root")!);
  await act(async () => root!.render(createElement(DiagnosisPage)));
  const pendingStart = [...document.querySelectorAll("button")].find((button) =>
    button.textContent?.includes("診断開始"),
  );
  await act(async () => {
    pendingStart?.click();
    await Promise.resolve();
  });
  const busy = [...document.querySelectorAll("button")].find((button) =>
    button.textContent?.includes("診断中"),
  );
  expect(busy?.hasAttribute("disabled")).toBe(true);
});
