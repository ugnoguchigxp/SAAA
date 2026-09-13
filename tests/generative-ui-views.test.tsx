import { afterEach, beforeEach, describe, expect, test } from "bun:test";
import { act } from "react";
import type { Root } from "react-dom/client";
import type { UiInstance, UiNode } from "../src/lib/generated/generativeUi";
import { invokeImpl, resetTauriCoreMock } from "./tauriCoreMock";
import { installJsdom } from "./jsdomGlobals";

await import("../src/i18n");
const { SemanticRenderer } = await import("../src/features/chat/ui/SemanticRenderer");
const { ActionsView, MetricView } = await import("../src/features/chat/ui/components");
const { UiContext } = await import("../src/features/chat/ui/context");
const { UiBoundary } = await import("../src/features/chat/ui/UiBoundary");
const { uiStates } = await import("../src/features/chat/ui/instanceState");

function node(kind: string, args: string[] = [], children: UiNode[] = [], id = kind): UiNode {
  return { id, kind, args, span: 6, children };
}

function instance(mode: "live" | "snapshot" = "snapshot"): UiInstance {
  return {
    id: "ui-1",
    viewId: "view-1",
    revision: 1,
    summary: "summary",
    definition: "",
    libraryVersion: 1,
    mode,
    node: node("Stack"),
    state: {},
    snapshots: {
      "runtime.summary": {
        capturedAt: "0",
        rows: [{ running: 1, completed: 2, failed: 0, total: 3 }],
      },
      "runtime.history": {
        capturedAt: "1",
        rows: [
          { time: 0, count: 1 },
          { time: 1_000, count: 4 },
        ],
      },
      "runtime.runs": {
        capturedAt: "2",
        rows: [{ id: "run-1", status: "running", provider: "local", startedAt: 0 }],
      },
      "larm.status": {
        capturedAt: "3",
        rows: [{ provider: "larm", runtime: "qwen", status: "ready", updatedAt: 0 }],
      },
    },
    stateVersion: 0,
    name: null,
    publishedRevision: null,
  };
}

describe("generative UI views", () => {
  let root: Root | null = null;
  let restore: (() => void) | null = null;
  let dom: ReturnType<typeof installJsdom>["dom"] | null = null;

  beforeEach(() => {
    resetTauriCoreMock();
    invokeImpl.handler = async (command) => {
      if (command === "query_ui_source") {
        return { capturedAt: "0", rows: [{ running: 1, completed: 2, failed: 0, total: 3 }] };
      }
      return command;
    };
  });

  afterEach(async () => {
    await act(async () => root?.unmount());
    root = null;
    restore?.();
    restore = null;
    dom = null;
  });

  test("renders semantic nodes and interactive snapshot views", async () => {
    const env = installJsdom();
    restore = env.restore;
    dom = env.dom;
    const { createRoot } = await import("react-dom/client");
    const { createElement } = await import("react");
    const value = { instance: instance(), conversationId: "c1", active: true, enabled: true };
    uiStates.initialize(value.instance);
    const tree = createElement(
      UiContext.Provider,
      { value },
      createElement(SemanticRenderer, {
        node: node(
          "Grid",
          [],
          [
            node("Cell", [], [node("Text", ["hello"])]),
            node("Metric", ["runtime.summary", "running", "Running"]),
            node("Status", ["runtime.summary", "failed", "Failed"]),
            node("Table", ["runtime.runs", "provider,status,startedAt"], [], "table"),
            node("ModelStatus", ["runtime.summary"], [], "model"),
            node("Chart", ["runtime.history"]),
            node("Actions", ["refresh"], [], "refresh"),
            node("Actions", ["cancel_run"], [], "cancel"),
          ],
        ),
      }),
    );
    root = createRoot(document.getElementById("root")!);
    await act(async () => root!.render(tree));
    expect(document.body.textContent).toContain("hello");
    const filter = document.querySelector("input");
    if (filter) {
      await act(async () => {
        const setter = Object.getOwnPropertyDescriptor(
          dom!.window.HTMLInputElement.prototype,
          "value",
        )!.set!;
        setter.call(filter, "local");
        filter.dispatchEvent(new dom!.window.Event("input", { bubbles: true }));
      });
    }
    const sort = document.querySelector("th button");
    await act(async () =>
      sort?.dispatchEvent(new dom!.window.MouseEvent("click", { bubbles: true })),
    );
    await act(async () =>
      sort?.dispatchEvent(new dom!.window.MouseEvent("click", { bubbles: true })),
    );
    expect(document.querySelector(".ui-chart")).not.toBeNull();
  });

  test("uses live context hooks and snapshot-only actions", async () => {
    restore = installJsdom().restore;
    const { createRoot } = await import("react-dom/client");
    const { createElement } = await import("react");
    const live = { instance: instance("live"), conversationId: "c1", active: true, enabled: true };
    uiStates.initialize(live.instance);
    root = createRoot(document.getElementById("root")!);
    await act(async () =>
      root!.render(
        createElement(
          UiContext.Provider,
          { value: live },
          createElement(MetricView, {
            source: "runtime.summary",
            field: "running",
            label: "Running",
          }),
        ),
      ),
    );
    await act(async () => {
      await Promise.resolve();
    });
    const snapshot = {
      instance: instance("snapshot"),
      conversationId: "c1",
      active: false,
      enabled: false,
    };
    await act(async () =>
      root!.render(
        createElement(
          UiContext.Provider,
          { value: snapshot },
          createElement(ActionsView, { action: "refresh" }),
        ),
      ),
    );
    await act(async () =>
      root!.render(
        createElement(UiBoundary, {
          fallback: createElement("p", null, "fallback"),
          children: createElement(SemanticRenderer, { node: node("Unknown") }),
        }),
      ),
    );
    expect(document.body.textContent).toContain("fallback");
  });
});
