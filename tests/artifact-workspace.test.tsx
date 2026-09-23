import { afterEach, beforeEach, describe, expect, mock, test } from "bun:test";
import { act, createElement } from "react";
import type { Root } from "react-dom/client";
import type { UiInstance } from "../src/lib/generated/generativeUi";
import { INTERACTIVE_HTML_FIXTURE } from "../src/features/chat/artifacts/artifactPreviewApi";
import { installJsdom } from "./jsdomGlobals";
import { invokeCalls, invokeImpl, resetTauriCoreMock } from "./tauriCoreMock";

await import("./tauriCoreMock");
mock.module("../src/features/chat/artifacts/artifactWebviewHost.ts", () => ({
  attachPreviewWebview: async (label: string) => ({
    label,
    setPosition: async () => undefined,
    setSize: async () => undefined,
    show: async () => undefined,
    hide: async () => undefined,
    close: async () => undefined,
    scroll: async () => undefined,
  }),
  closePreviewWebview: async () => undefined,
  setActiveSourceWebview: () => undefined,
  scrollActiveSourceWebview: async () => undefined,
}));

await import("../src/i18n");
await import("../src/features/chat/artifacts/ArtifactPanel");
const { ArtifactWorkspaceProvider, useArtifactWorkspace } =
  await import("../src/features/chat/artifacts/ArtifactDrawer");
const { CompletedMessage } = await import("../src/features/chat/ChatMessages");

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

function OpenPreview() {
  const workspace = useArtifactWorkspace();
  return createElement(
    "button",
    {
      type: "button",
      className: "open-preview",
      onClick: () =>
        workspace?.openInteractivePreview({
          artifactId: INTERACTIVE_HTML_FIXTURE.artifactId,
          revisionId: INTERACTIVE_HTML_FIXTURE.revisionId,
          title: "Interactive HTML",
        }),
    },
    "open preview",
  );
}

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

function OpenSource() {
  const workspace = useArtifactWorkspace();
  return createElement(
    "button",
    {
      type: "button",
      className: "open-source",
      onClick: () =>
        workspace?.openSource({
          conversationId: "conversation-1",
          url: "https://example.com/weather",
          title: "Weather source",
        }),
    },
    "open source",
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
  let restoreRect: (() => void) | null = null;

  beforeEach(() => {
    resetTauriCoreMock();
    invokeImpl.handler = async (command) => {
      if (command === "get_ui_enabled") return true;
      if (command === "get_ui_instance") return instance;
      if (command === "list_ui_view_revisions") {
        return [{ revision: 1, summary: "Article", createdAt: "1" }];
      }
      if (command === "prepare_artifact_preview") {
        return {
          artifactId: INTERACTIVE_HTML_FIXTURE.artifactId,
          revisionId: INTERACTIVE_HTML_FIXTURE.revisionId,
          title: "Interactive HTML",
          mediaType: "text/html",
          digest: "abc",
          previewToken: "a".repeat(64),
          expiresAt: "2026-01-01T00:00:00.000Z",
          webviewLabel: "artifact-preview-test",
          policy: {
            network: "none",
            navigation: "preview-only",
            popup: "deny",
            download: "deny",
            tauriIpc: "deny",
          },
        };
      }
      if (command === "read_source_artifact") {
        return {
          url: "https://example.com/weather",
          text: "Saved <script>text</script>",
          sourceKind: "search-excerpt",
          observedAt: 1_790_000_000_000,
          truncated: false,
        };
      }
      if (command === "mount_source_website") return "source-website-test";
      return undefined;
    };
  });

  afterEach(async () => {
    await act(async () => root?.unmount());
    await new Promise((resolve) => setTimeout(resolve, 0));
    root = null;
    restoreRect?.();
    restoreRect = null;
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
          createElement("section", { className: "chat-panel" }, createElement(OpenArtifact)),
        ),
      ),
    );
    const opener = document.querySelector<HTMLButtonElement>("button")!;
    opener.focus();
    await act(async () => {
      opener.dispatchEvent(new Event("click", { bubbles: true }));
      await Promise.resolve();
    });
    const workspace = document.querySelector<HTMLElement>(".artifact-workspace")!;
    expect(workspace.classList.contains("artifact-workspace-open")).toBe(true);
    expect(workspace.style.getPropertyValue("--artifact-width")).toBe("50%");
    expect(workspace.querySelector(".artifact-chat-region .chat-panel")).not.toBeNull();
    expect(workspace.querySelector(".artifact-panel")).not.toBeNull();
    expect(document.activeElement).toBe(workspace.querySelector(".artifact-panel"));

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
      if (command === "get_ui_instance" || command === "list_ui_view_revisions") {
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
          createElement("section", { className: "chat-panel" }, createElement(OpenArtifact)),
        ),
      ),
    );
    await act(async () => {
      document.querySelector("button")!.dispatchEvent(new Event("click", { bubbles: true }));
      await Promise.resolve();
      await Promise.resolve();
    });
    expect(document.querySelector(".artifact-panel")?.textContent).toContain("artifact");
    expect(document.querySelector(".artifact-load-error")).not.toBeNull();
  });

  test("deduplicates views and keeps at most eight tabs", async () => {
    const environment = installJsdom();
    restore = environment.restore;
    const { createRoot } = await import("react-dom/client");
    root = createRoot(document.getElementById("root")!);
    await act(async () =>
      root!.render(
        createElement(ArtifactWorkspaceProvider, null, createElement(OpenManyArtifacts)),
      ),
    );
    const openers = Array.from(document.querySelectorAll<HTMLButtonElement>(".open-many"));
    for (const opener of openers) {
      await act(async () => opener.dispatchEvent(new Event("click", { bubbles: true })));
    }
    expect(document.querySelectorAll(".artifact-tab")).toHaveLength(8);
    expect(document.querySelector(".artifact-tabs")?.textContent).not.toContain("Article 1");
    await act(async () => openers[8].dispatchEvent(new Event("click", { bubbles: true })));
    expect(document.querySelectorAll(".artifact-tab")).toHaveLength(8);

    const tabButtons = Array.from(document.querySelectorAll<HTMLButtonElement>('[role="tab"]'));
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
    expect(tabButtons[6].getAttribute("aria-controls")).toBe("artifact-panel-content");
    expect(document.querySelector('[role="tabpanel"]')?.getAttribute("aria-labelledby")).toBe(
      tabButtons[6].id,
    );
  });

  test("switches between a semantic tab and an interactive preview tab", async () => {
    restore = installJsdom().restore;
    const { createRoot } = await import("react-dom/client");
    root = createRoot(document.getElementById("root")!);
    await act(async () =>
      root!.render(
        createElement(
          ArtifactWorkspaceProvider,
          null,
          createElement("div", null, createElement(OpenArtifact), createElement(OpenPreview)),
        ),
      ),
    );
    const click = (selector: string) =>
      document
        .querySelector<HTMLButtonElement>(selector)!
        .dispatchEvent(new Event("click", { bubbles: true }));
    await act(async () => click("button"));
    expect(document.querySelector(".artifact-tabs")?.textContent).toContain("Article");
    await act(async () => click(".open-preview"));
    for (let step = 0; step < 8; step += 1)
      await act(async () => {
        await new Promise((resolve) => setTimeout(resolve, 0));
      });
    const tabs = Array.from(document.querySelectorAll<HTMLButtonElement>('[role="tab"]'));
    expect(tabs).toHaveLength(2);
    expect(tabs[1].getAttribute("aria-selected")).toBe("true");
    expect(document.querySelector(".artifact-webview-host")).not.toBeNull();
    await act(async () => tabs[0].dispatchEvent(new Event("click", { bubbles: true })));
    expect(document.querySelector(".artifact-webview-host")).toBeNull();
  });

  test("opens the source website in the artifact panel", async () => {
    restore = installJsdom().restore;
    Object.defineProperty(document, "hidden", { configurable: true, value: false });
    const originalRect = HTMLElement.prototype.getBoundingClientRect;
    HTMLElement.prototype.getBoundingClientRect = () => ({
      x: 400, y: 100, width: 500, height: 400,
      top: 100, left: 400, right: 900, bottom: 500, toJSON: () => ({}),
    });
    restoreRect = () => { HTMLElement.prototype.getBoundingClientRect = originalRect; };
    const { createRoot } = await import("react-dom/client");
    root = createRoot(document.getElementById("root")!);
    await act(async () =>
      root!.render(createElement(ArtifactWorkspaceProvider, null, createElement(OpenSource))),
    );
    await act(async () => {
      document.querySelector<HTMLButtonElement>(".open-source")!.click();
    });
    for (let step = 0; step < 8; step += 1) {
      await act(async () => {
        await new Promise((resolve) => setTimeout(resolve, 0));
      });
    }
    expect(invokeCalls).toContainEqual(expect.objectContaining({
      command: "mount_source_website",
      args: expect.objectContaining({ url: "https://example.com/weather" }),
    }));
    expect(document.querySelector(".artifact-source-text")).toBeNull();
    await act(async () => {
      document.querySelector<HTMLButtonElement>(".artifact-source-browser")!.click();
      await Promise.resolve();
    });
    expect(invokeCalls.some((call) => call.command === "open_source_website_in_browser")).toBe(true);
  });

  test("assistant source links open the artifact panel", async () => {
    restore = installJsdom().restore;
    Object.defineProperty(document, "hidden", { configurable: true, value: false });
    const originalRect = HTMLElement.prototype.getBoundingClientRect;
    HTMLElement.prototype.getBoundingClientRect = () => ({
      x: 400, y: 100, width: 500, height: 400,
      top: 100, left: 400, right: 900, bottom: 500, toJSON: () => ({}),
    });
    restoreRect = () => { HTMLElement.prototype.getBoundingClientRect = originalRect; };
    const { createRoot } = await import("react-dom/client");
    root = createRoot(document.getElementById("root")!);
    await act(async () =>
      root!.render(
        createElement(
          ArtifactWorkspaceProvider,
          null,
          createElement(CompletedMessage, {
            message: {
              id: "assistant-1",
              conversationId: "conversation-1",
              role: "assistant",
              content: "[Source](https://example.com/weather)",
              createdAt: "2026-09-23T00:00:00Z",
            },
          }),
        ),
      ),
    );
    for (let step = 0; step < 4; step += 1)
      await act(async () => await new Promise((resolve) => setTimeout(resolve, 0)));
    await act(async () => {
      document.querySelector<HTMLAnchorElement>(".message a")!.click();
      await new Promise((resolve) => setTimeout(resolve, 0));
    });
    expect(document.querySelector(".artifact-panel")?.textContent).toContain("Source");
    expect(invokeCalls.some((call) => call.command === "mount_source_website")).toBe(true);
  });
});
