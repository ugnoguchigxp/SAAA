import { afterEach, expect, test } from "bun:test";
import { act } from "react";
import { MemoryPage } from "../src/features/memory/MemoryPage";
import { installJsdom } from "./jsdomGlobals";
import { invokeImpl, resetTauriCoreMock } from "./tauriCoreMock";

afterEach(resetTauriCoreMock);

test("World view shows persisted knowledge without mixing conversation state", async () => {
  const env = installJsdom();
  const { createRoot } = await import("react-dom/client");
  const root = createRoot(document.getElementById("root")!);
  invokeImpl.handler = async (command) => {
    if (command === "personal_source_page") return { sources: [], nextSequence: 0, hasMore: false };
    if (command !== "personal_state_snapshot") throw new Error(`unexpected ${command}`);
    return {
      enabled: true,
      revision: 1,
      inputEpoch: 1,
      pendingCount: 0,
      pendingBytes: 0,
      contractReady: true,
      contractReason: null,
      cleanup: [],
      items: [
        { id: "state", key: "state-key", status: "active", value: "continuity-only", source: [] },
      ],
      worldItems: [
        { id: "world", key: "world-key", status: "active", value: "causal-only", source: [] },
      ],
      maintenance: {
        reason: "connection-unavailable",
        failures: 1,
        nextAttemptAt: 30000,
        updatedAt: 0,
      },
    };
  };
  try {
    await act(async () => root.render(<MemoryPage onOpenRecord={() => {}} onCorrect={() => {}} />));
    expect(document.body.textContent).toContain("continuity-only");
    expect(document.body.textContent).not.toContain("causal-only");
    const buttons = [...document.querySelectorAll<HTMLButtonElement>("button[aria-pressed]")];
    await act(async () => buttons[1]!.click());
    expect(document.body.textContent).toContain("causal-only");
    expect(document.body.textContent).not.toContain("continuity-only");
    expect(buttons[1]!.getAttribute("aria-pressed")).toBe("true");
  } finally {
    await act(async () => root.unmount());
    env.restore();
  }
});

test("World-only data remains visible and questions reach the supplied conversation action", async () => {
  const env = installJsdom();
  const { createRoot } = await import("react-dom/client");
  const root = createRoot(document.getElementById("root")!);
  const asked: string[] = [];
  invokeImpl.handler = async (command) => {
    if (command === "personal_source_page") return { sources: [], nextSequence: 0, hasMore: false };
    if (command !== "personal_state_snapshot") throw new Error(command);
    return {
      enabled: false,
      revision: 0,
      inputEpoch: 0,
      pendingCount: 0,
      pendingBytes: 0,
      contractReady: false,
      contractReason: null,
      cleanup: [],
      items: [],
      worldItems: [
        { id: "world", key: "world-key", status: "active", value: "world-only", source: [] },
      ],
    };
  };
  try {
    await act(async () =>
      root.render(
        <MemoryPage
          onOpenRecord={() => {}}
          onCorrect={() => {}}
          onAsk={async (question) => {
            asked.push(question);
          }}
        />,
      ),
    );
    await act(async () =>
      document.querySelectorAll<HTMLButtonElement>("button[aria-pressed]")[1]!.click(),
    );
    expect(document.querySelector("table")?.textContent).toContain("world-only");
    const input = document.querySelector<HTMLInputElement>("form input")!;
    await act(async () => {
      Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, "value")!.set!.call(
        input,
        "cacheの前提は？",
      );
      input.dispatchEvent(new window.Event("input", { bubbles: true }));
      input.dispatchEvent(new window.Event("change", { bubbles: true }));
    });
    await act(async () =>
      document
        .querySelector("form")!
        .dispatchEvent(new window.Event("submit", { bubbles: true, cancelable: true })),
    );
    expect(asked).toEqual(["cacheの前提は？"]);
    expect(input.value).toBe("");
  } finally {
    await act(async () => root.unmount());
    env.restore();
  }
});
