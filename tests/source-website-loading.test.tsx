import { afterEach, beforeEach, expect, mock, test } from "bun:test";
import { act, createElement, StrictMode } from "react";
import type { Root } from "react-dom/client";
import { installJsdom } from "./jsdomGlobals";
import { channels, invokeCalls, invokeImpl, resetTauriCoreMock } from "./tauriCoreMock";

await import("./tauriCoreMock");
const shown: string[] = [];
const closed: string[] = [];
const ready: boolean[] = [];
const onReady = (value: boolean) => ready.push(value);
mock.module("../src/features/chat/artifacts/artifactWebviewHost", () => ({
  attachPreviewWebview: async (label: string) => ({
    label,
    setPosition: async () => undefined,
    setSize: async () => undefined,
    show: async () => {
      shown.push(label);
    },
    hide: async () => undefined,
    close: async () => {
      closed.push(label);
    },
  }),
  closePreviewWebview: async (label: string) => {
    closed.push(label);
  },
  setActiveSourceWebview: () => undefined,
  scrollActiveSourceWebview: async () => undefined,
}));
await import("../src/i18n");
const { SOURCE_SLOW_MS, SOURCE_TIMEOUT_MS } =
  await import("../src/features/chat/artifacts/useSourceWebsite");
const { default: SourceWebsite } = await import("../src/features/chat/artifacts/SourceWebsite");
let root: Root;
let restore: () => void;
const originalTimeout = globalThis.setTimeout;
const originalClearTimeout = globalThis.clearTimeout;
const timers = new Map<number, { callback: () => void; duration: number }>();
let timerId = 90000;
let mountNumber = 0;

beforeEach(async () => {
  resetTauriCoreMock();
  shown.length = 0;
  closed.length = 0;
  ready.length = 0;
  timers.clear();
  mountNumber = 0;
  restore = installJsdom().restore;
  Object.defineProperty(document, "hidden", { configurable: true, value: false });
  HTMLElement.prototype.getBoundingClientRect = () => ({
    x: 20,
    y: 20,
    width: 400,
    height: 300,
    top: 20,
    left: 20,
    right: 420,
    bottom: 320,
    toJSON: () => ({}),
  });
  globalThis.setTimeout = ((callback: () => void, duration: number, ...args: unknown[]) => {
    if (duration === SOURCE_SLOW_MS || duration === SOURCE_TIMEOUT_MS) {
      timers.set(++timerId, { callback, duration });
      return timerId;
    }
    return originalTimeout(callback, duration, ...args);
  }) as typeof setTimeout;
  globalThis.clearTimeout = ((id: number) => {
    if (!timers.delete(id)) originalClearTimeout(id);
  }) as typeof clearTimeout;
  invokeImpl.handler = async () => `source-website-${++mountNumber}`;
  const { createRoot } = await import("react-dom/client");
  root = createRoot(document.getElementById("root")!);
});
afterEach(async () => {
  await act(async () => root.unmount());
  globalThis.setTimeout = originalTimeout;
  globalThis.clearTimeout = originalClearTimeout;
  restore();
});
async function render(url = "https://example.com/one", strict = false) {
  await act(async () => {
    const element = createElement(SourceWebsite, {
      conversationId: "test",
      url,
      title: "Website",
      onReady,
    });
    root.render(strict ? createElement(StrictMode, null, element) : element);
  });
}
async function event(phase: string, navigation = 1, index = channels.length - 1) {
  await act(async () => {
    channels[index].onmessage?.({ phase, navigation });
  });
}
async function advance(duration: number) {
  await act(async () => {
    for (const [id, timer] of [...timers])
      if (timer.duration === duration) {
        timers.delete(id);
        timer.callback();
      }
  });
}

test("mount alone leaves the loading dialog visible; native load/layout completion reveals it", async () => {
  await render();
  expect(invokeCalls[0].command).toBe("mount_source_website");
  expect(document.querySelector('[role="dialog"]')).not.toBeNull();
  expect(shown).toEqual([]);
  expect(ready).not.toContain(true);
  await event("loading");
  expect(shown).toEqual([]);
  await event("preparing");
  expect(document.querySelector('[role="dialog"]')).not.toBeNull();
  expect(shown).toEqual([]);
  await event("ready");
  expect(shown).toEqual(["source-website-1"]);
  expect(ready.at(-1)).toBe(true);
  expect(document.querySelector('[role="dialog"]')).toBeNull();
  expect(timers.size).toBe(0);
});

test("slow loads remain pending, timeout offers retry, late completion cannot reveal them", async () => {
  await render();
  await advance(SOURCE_SLOW_MS);
  expect(document.querySelector("button")).not.toBeNull();
  expect(document.querySelector('[role="progressbar"], progress')).not.toBeNull();
  await advance(SOURCE_TIMEOUT_MS);
  expect(document.querySelector('[role="alert"]')).not.toBeNull();
  expect(closed).toContain("source-website-1");
  await event("ready");
  expect(shown).toEqual([]);
  await act(async () => {
    document.querySelector<HTMLButtonElement>("button")!.click();
  });
  expect(channels.length).toBe(2);
  await event("ready", 1, 0);
  expect(shown).toEqual([]);
  await event("ready", 1, 1);
  expect(shown).toEqual(["source-website-2"]);
});

test("changing tabs ignores stale channels and earlier navigation completion", async () => {
  await render();
  await render("https://example.com/two");
  await event("ready", 1, 0);
  expect(shown).toEqual([]);
  await event("loading", 2);
  await event("ready", 1);
  expect(shown).toEqual([]);
  await event("ready", 2);
  expect(shown).toEqual(["source-website-2"]);
});

test("completion arriving before mount resolves is retained without revealing an absent view", async () => {
  let resolveMount!: (label: string) => void;
  invokeImpl.handler = () =>
    new Promise<string>((resolve) => {
      resolveMount = resolve;
    });
  await render();
  await event("ready");
  expect(shown).toEqual([]);
  await act(async () => {
    resolveMount("source-website-delayed");
  });
  expect(shown).toEqual(["source-website-delayed"]);
});

test("strict-mode cancelled mount is closed and cannot dismiss the active dialog", async () => {
  await render("https://example.com/one", true);
  expect(channels.length).toBe(2);
  expect(closed).toContain("source-website-1");
  await event("ready", 1, 0);
  expect(shown).toEqual([]);
  await event("ready", 1, 1);
  expect(shown).toEqual(["source-website-2"]);
});

test("native layout failure keeps an error dialog and closes the surface", async () => {
  await render();
  await event("error");
  expect(shown).toEqual([]);
  expect(document.querySelector('[role="alert"]')).not.toBeNull();
  expect(closed).toContain("source-website-1");
});

test("a page completed in the background waits for visibility without timing out", async () => {
  await render();
  Object.defineProperty(document, "hidden", { configurable: true, value: true });
  await event("ready");
  expect(shown).toEqual([]);
  expect(timers.size).toBe(0);
  await advance(SOURCE_TIMEOUT_MS);
  expect(closed).toEqual([]);
  Object.defineProperty(document, "hidden", { configurable: true, value: false });
  await act(async () => {
    document.dispatchEvent(new Event("visibilitychange"));
    await new Promise((resolve) => originalTimeout(resolve, 30));
  });
  expect(shown).toEqual(["source-website-1"]);
});
