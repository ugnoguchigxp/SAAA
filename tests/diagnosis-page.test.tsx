import { afterEach, expect, test } from "bun:test";
import { act } from "react";
import type { Root } from "react-dom/client";
import i18n from "../src/i18n";
import type { DiagnosisReport, DiagnosisScope } from "../src/lib/generated/diagnosis";
import { installJsdom } from "./jsdomGlobals";
import { capability, evidence, mixed, report } from "./diagnosisFixtures";

const { DiagnosisPage } = await import("../src/features/diagnosis/DiagnosisPage");

let runCalls: DiagnosisScope[] = [];
let root: Root | null = null;
let restore: (() => void) | null = null;

afterEach(async () => {
  await act(async () => root?.unmount());
  root = null;
  restore?.();
  restore = null;
});

const justNow = (value: DiagnosisReport): DiagnosisReport => ({
  ...value,
  finishedAt: Date.now() - 1_000,
});

async function renderPage(
  initial: DiagnosisReport,
  onOpenSettings = () => undefined,
  language = "ja",
  bump = true,
) {
  runCalls = [];
  const backend = {
    listen: async () => () => undefined,
    get: async () => initial,
    run: async (scope: DiagnosisScope) => {
      runCalls.push(scope);
      return bump ? { ...initial, revision: initial.revision + 1 } : initial;
    },
  };
  restore = installJsdom().restore;
  const { createRoot } = await import("react-dom/client");
  const { createElement } = await import("react");
  await i18n.changeLanguage(language);
  root = createRoot(document.getElementById("root")!);
  await act(async () => root!.render(createElement(DiagnosisPage, { onOpenSettings, backend })));
  await act(async () => {
    await Promise.resolve();
  });
}

const buttons = (label: string) =>
  [...document.querySelectorAll("button")].filter((button) => button.textContent === label);
const card = (id: string) => document.querySelector(`[data-capability="${id}"]`)!;
const runs = () => runCalls;

test("page leads with a verdict that counts unproven capabilities and lists problems first", async () => {
  await renderPage(justNow(mixed));
  const banner = document.querySelector(".dx-banner")!;
  expect(banner.getAttribute("data-state")).toBe("unverified");
  expect(banner.textContent).toContain("実機で確認できていない機能が 1 件あります");
  const order = [...document.querySelectorAll("[data-capability]")].map((node) =>
    node.getAttribute("data-capability"),
  );
  expect(order.slice(0, 3)).toEqual(["voice-listen", "memory", "conversation"]);
  expect(order.at(-1)).toBe("coding");
  expect(card("voice-listen").textContent).toContain("利用不可");
  expect(card("voice-listen").textContent).toContain("接続できません");
  expect(card("voice-listen").textContent).toContain("connection refused");
  expect(card("conversation").textContent).toContain("実際の動作は未確認");
  expect(card("storage").textContent).toContain("最終確認 5 分前");
  expect(card("conversation").textContent).toContain("まだ確認されていません");
});

test("evidence rows name the subject and tier without exposing raw route ids", async () => {
  await renderPage(justNow(mixed));
  const text = card("conversation").textContent ?? "";
  expect(text).toContain("DeepSeek V4.1 Flash");
  expect(text).toContain("設定・構成");
  expect(text).not.toContain("provider:deepseek");
  expect(card("voice-listen").textContent).toContain("LARM 音声認識");
  expect(card("voice-listen").textContent).toContain("実機確認");
});

test("a card can be retested alone and offers settings only when settings can fix it", async () => {
  let opened = 0;
  await renderPage(justNow(mixed), () => {
    opened += 1;
  });
  expect(buttons("設定を開く")).toHaveLength(1);
  expect(card("voice-listen").textContent).toContain("設定を開く");
  expect(card("storage").textContent).not.toContain("設定を開く");
  await act(async () => {
    buttons("設定を開く")[0]!.click();
  });
  expect(opened).toBe(1);
  await act(async () => {
    card("voice-listen").querySelector("button")!.click();
    await Promise.resolve();
  });
  expect(runs().at(-1)).toEqual({ kind: "capability", capability: "voice-listen" });
});

test("quick and live checks send their scopes", async () => {
  await renderPage(justNow(mixed));
  await act(async () => {
    buttons("クイック診断")[0]!.click();
    await Promise.resolve();
  });
  expect(runs().at(-1)).toEqual({ kind: "quick" });
  await act(async () => {
    buttons("実機診断")[0]!.click();
    await Promise.resolve();
  });
  expect(runs().at(-1)).toEqual({ kind: "full" });
});

test("the live check is the primary action while something is unproven", async () => {
  await renderPage(justNow(mixed));
  expect(buttons("実機診断")[0]!.className).toContain("dx-primary");
  expect(buttons("クイック診断")[0]!.className).not.toContain("dx-primary");
  const allReady = justNow(report("ready", [capability("storage", "ready")]));
  await act(async () => root?.unmount());
  restore?.();
  await renderPage(allReady);
  expect(document.querySelector(".dx-banner")!.textContent).toContain(
    "主要な機能が使えることを確認しました",
  );
  expect(buttons("クイック診断")[0]!.className).toContain("dx-primary");
});

test("opening the page refreshes a stale report once, and leaves a fresh one alone", async () => {
  await renderPage({ ...mixed, finishedAt: Date.now() - 10 * 60_000 });
  expect(runs()).toHaveLength(1);
  expect(runs()[0]).toEqual({ kind: "quick" });
  await act(async () => root?.unmount());
  restore?.();
  await renderPage(justNow(mixed));
  expect(runs()).toHaveLength(0);
});

test("buttons are disabled and progress is announced while a run is active", async () => {
  await renderPage(justNow({ ...mixed, running: true }));
  expect(buttons("クイック診断")[0]!.hasAttribute("disabled")).toBe(true);
  expect(buttons("実機診断")[0]!.hasAttribute("disabled")).toBe(true);
  expect(document.querySelector("[role=status]")!.textContent).toBe("診断中…");
});

test("before the first run the page says so instead of showing a green state", async () => {
  await renderPage(
    report("unverified", [], { revision: 0, finishedAt: null }),
    () => undefined,
    "ja",
    false,
  );
  expect(document.querySelector(".dx-banner")!.textContent).toContain(
    "診断はまだ実行されていません",
  );
  expect(document.querySelectorAll("[data-capability]")).toHaveLength(0);
});

test("english wording follows plural rules", async () => {
  const two = justNow(
    report("unverified", [
      capability("conversation", "unverified"),
      capability("voice-speak", "unverified"),
    ]),
  );
  await renderPage(two, () => undefined, "en");
  expect(document.querySelector(".dx-banner")!.textContent).toContain(
    "2 capabilities are not yet confirmed on a live run",
  );
  await i18n.changeLanguage("ja");
});

test("numeric details are formatted by the UI and buttons name their capability", async () => {
  const storage = capability("storage", "degraded", {
    reason: "capacity-high",
    evidence: [
      evidence({
        source: "records.capacity",
        outcome: "degraded",
        reason: "capacity-high",
        detail: "85",
      }),
    ],
  });
  await renderPage(justNow(report("degraded", [storage])));
  expect(card("storage").textContent).toContain("使用率 85%");
  const retest = card("storage").querySelector("button")!;
  expect(retest.getAttribute("aria-label")).toBe("再試験: 保存領域");
  expect(document.querySelector("[role=toolbar]")).toBeNull();
});

test("error text is never forced into a numeric detail template", async () => {
  const storage = capability("storage", "unavailable", {
    reason: "internal",
    evidence: [
      evidence({
        source: "records.capacity",
        outcome: "unverified",
        reason: "internal",
        detail: "database error",
      }),
    ],
  });
  await renderPage(justNow(report("unavailable", [storage])));
  const text = card("storage").textContent ?? "";
  expect(text).toContain("database error");
  expect(text).not.toContain("使用率 database error");
});
