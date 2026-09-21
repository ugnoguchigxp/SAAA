import { afterEach, beforeEach, describe, expect, test } from "bun:test";
import { act, createElement } from "react";
import type { Root } from "react-dom/client";
import type { UiInstance } from "../src/lib/generated/generativeUi";
import { installJsdom } from "./jsdomGlobals";
import { invokeImpl, resetTauriCoreMock } from "./tauriCoreMock";

await import("../src/i18n");
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
  node: { id: "root", kind: "Text", args: ["artifact"], span: 12, children: [] },
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
    { type: "button", onClick: () => workspace?.open(instance, "conversation-1") },
    "open artifact",
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
    root = null;
    restore?.();
    restore = null;
  });

  test("shrinks chat into a 50/50 split instead of overlaying it", async () => {
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
    });
    const workspace = document.querySelector<HTMLElement>(".artifact-workspace")!;
    expect(workspace.classList.contains("artifact-workspace-open")).toBe(true);
    expect(workspace.style.getPropertyValue("--artifact-width")).toBe("50%");
    expect(workspace.querySelector(".artifact-chat-region .chat-panel")).not.toBeNull();
    expect(workspace.querySelector(".artifact-panel")).not.toBeNull();
  });
});
