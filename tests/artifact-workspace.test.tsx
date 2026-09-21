import { afterEach, beforeEach, describe, expect, test } from "bun:test";
import { act, createElement } from "react";
import type { Root } from "react-dom/client";
import type { UiInstance } from "../src/lib/generated/generativeUi";
import { installJsdom } from "./jsdomGlobals";
import { invokeImpl, resetTauriCoreMock } from "./tauriCoreMock";

await import("../src/i18n");
await import("../src/features/chat/artifacts/ArtifactPanel");
const { ArtifactWorkspaceProvider, useArtifactWorkspace } =
  await import("../src/features/chat/artifacts/ArtifactDrawer");

const instance: UiInstance = {
  id: "ui-1",
  viewId: "view-1",
  revision: 1,
  summary: "Article",
  definition: "",
  libraryVersion: 1,
  mode: "snapshot",
  node: {
    id: "root",
    kind: "Text",
    args: ["artifact"],
    span: 12,
    children: [],
  },
  state: {},
  snapshots: {},
  stateVersion: 0,
  name: null,
  publishedRevision: null,
};

function OpenArtifact() {
  const workspace = useArtifactWorkspace();
  return createElement(
    "button",
    {
      type: "button",
      onClick: () => workspace?.open(instance, "conversation-1"),
    },
    "open artifact",
  );
}

function OpenManyArtifacts() {
  const workspace = useArtifactWorkspace();
  return createElement(
    "div",
    null,
    ...Array.from({ length: 9 }, (_, index) => {
      const number = index + 1;
      const next = {
        ...instance,
        id: `ui-${number}`,
        viewId: `view-${number}`,
        summary: `Article ${number}`,
      };
      return createElement(
        "button",
        {
          key: number,
          type: "button",
          className: "open-many",
          onClick: () => workspace?.open(next, "conversation-1"),
        },
        `open ${number}`,
      );
    }),
  );
}

describe("artifact workspace", () => {
  let root: Root | null = null;
  let restore: (() => void) | null = null;

  beforeEach(() => {
    resetTauriCoreMock();
    invokeImpl.handler = async (command) => {
      if (command === "get_ui_enabled") return true;
      if (command === "get_ui_instance") return instance;
      if (command === "list_ui_view_revisions") {
        return [{ revision: 1, summary: "Article", createdAt: "1" }];
      }
      return undefined;
    };
  });

  afterEach(async () => {
    await act(async () => root?.unmount());
    await new Promise((resolve) => setTimeout(resolve, 0));
    root = null;
    restore?.();
    restore = null;
  });

  test("shrinks chat into a 50/50 split instead of overlaying it", async () => {
    const environment = installJsdom();
    restore = environment.restore;
    const { createRoot } = await import("react-dom/client");
    root = createRoot(document.getElementById("root")!);
    await act(async () =>
      root!.render(
        createElement(
          ArtifactWorkspaceProvider,
          null,
          createElement(
            "section",
            { className: "chat-panel" },
            createElement(OpenArtifact),
          ),
        ),
      ),
    );
    const opener = document.querySelector<HTMLButtonElement>("button")!;
    opener.focus();
    await act(async () => {
      opener.dispatchEvent(new Event("click", { bubbles: true }));
      await Promise.resolve();
    });
    const workspace = document.querySelector<HTMLElement>(
      ".artifact-workspace",
    )!;
    expect(workspace.classList.contains("artifact-workspace-open")).toBe(true);
    expect(workspace.style.getPropertyValue("--artifact-width")).toBe("50%");
    expect(
      workspace.querySelector(".artifact-chat-region .chat-panel"),
    ).not.toBeNull();
    expect(workspace.querySelector(".artifact-panel")).not.toBeNull();
    expect(document.activeElement).toBe(
      workspace.querySelector(".artifact-panel"),
    );

    await act(async () => {
      window.dispatchEvent(
        new environment.dom.window.KeyboardEvent("keydown", {
          key: "Escape",
          bubbles: true,
        }),
      );
      await Promise.resolve();
    });
    expect(workspace.querySelector(".artifact-panel")).toBeNull();
    expect(document.activeElement).toBe(opener);
  });

  test("keeps the current artifact visible when revision IPC fails", async () => {
    invokeImpl.handler = async (command) => {
      if (command === "get_ui_enabled") return true;
      if (
        command === "get_ui_instance" ||
        command === "list_ui_view_revisions"
      ) {
        throw new Error("offline");
      }
      return undefined;
    };
    restore = installJsdom().restore;
    const { createRoot } = await import("react-dom/client");
    root = createRoot(document.getElementById("root")!);
    await act(async () =>
      root!.render(
        createElement(
          ArtifactWorkspaceProvider,
          null,
          createElement(
            "section",
            { className: "chat-panel" },
            createElement(OpenArtifact),
          ),
        ),
      ),
    );
    await act(async () => {
      document
        .querySelector("button")!
        .dispatchEvent(new Event("click", { bubbles: true }));
      await Promise.resolve();
      await Promise.resolve();
    });
    expect(document.querySelector(".artifact-panel")?.textContent).toContain(
      "artifact",
    );
    expect(document.querySelector(".artifact-load-error")).not.toBeNull();
  });

  test("deduplicates views and keeps at most eight tabs", async () => {
    const environment = installJsdom();
    restore = environment.restore;
    const { createRoot } = await import("react-dom/client");
    root = createRoot(document.getElementById("root")!);
    await act(async () =>
      root!.render(
        createElement(
          ArtifactWorkspaceProvider,
          null,
          createElement(OpenManyArtifacts),
        ),
      ),
    );
    const openers = Array.from(
      document.querySelectorAll<HTMLButtonElement>(".open-many"),
    );
    for (const opener of openers) {
      await act(async () =>
        opener.dispatchEvent(new Event("click", { bubbles: true })),
      );
    }
    expect(document.querySelectorAll(".artifact-tab")).toHaveLength(8);
    expect(document.querySelector(".artifact-tabs")?.textContent).not.toContain(
      "Article 1",
    );
    await act(async () =>
      openers[8].dispatchEvent(new Event("click", { bubbles: true })),
    );
    expect(document.querySelectorAll(".artifact-tab")).toHaveLength(8);

    const tabButtons = Array.from(
      document.querySelectorAll<HTMLButtonElement>('[role="tab"]'),
    );
    expect(tabButtons[7].getAttribute("aria-selected")).toBe("true");
    expect(tabButtons[7].tabIndex).toBe(0);
    tabButtons[7].focus();
    await act(async () => {
      tabButtons[7].dispatchEvent(
        new environment.dom.window.KeyboardEvent("keydown", {
          key: "ArrowLeft",
          bubbles: true,
        }),
      );
    });
    expect(tabButtons[6].getAttribute("aria-selected")).toBe("true");
    expect(document.activeElement).toBe(tabButtons[6]);
    expect(tabButtons[6].getAttribute("aria-controls")).toBe(
      "artifact-panel-content",
    );
    expect(
      document
        .querySelector('[role="tabpanel"]')
        ?.getAttribute("aria-labelledby"),
    ).toBe(tabButtons[6].id);
  });
});
