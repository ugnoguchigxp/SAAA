import { afterEach, beforeEach, describe, expect, mock, test } from "bun:test";
import { act, createElement, useRef } from "react";
import type { Root } from "react-dom/client";
import { installJsdom } from "./jsdomGlobals";
import { invokeImpl, resetTauriCoreMock } from "./tauriCoreMock";

await import("./tauriCoreMock");

const closed: string[] = [];
const created: string[] = [];

mock.module("../src/features/chat/artifacts/artifactWebviewHost.ts", () => ({
  attachPreviewWebview: async (label: string) => {
    created.push(label);
    return {
      label,
      setPosition: async () => undefined,
      setSize: async () => undefined,
      show: async () => undefined,
      hide: async () => undefined,
      close: async () => {
        closed.push(label);
      },
    };
  },
  closePreviewWebview: async (label: string) => {
    closed.push(label);
  },
}));

const { useArtifactWebview } = await import("../src/features/chat/artifacts/useArtifactWebview");
await import("../src/i18n");

function Harness({
  artifactId = "artifact.fixture.interactive-html",
  active = true,
}: {
  artifactId?: string;
  active?: boolean;
}) {
  const hostRef = useRef<HTMLDivElement>(null);
  useArtifactWebview({
    active,
    artifactId,
    revisionId: "rev.fixture.interactive-html.v1",
    hostRef,
    retryNonce: 0,
  });
  return createElement("div", {
    ref: hostRef,
    className: "host",
    style: { width: "100px", height: "100px" },
  });
}

function installMeasurableHost() {
  globalThis.requestAnimationFrame = (callback: FrameRequestCallback) =>
    setTimeout(() => callback(0), 0) as unknown as number;
  globalThis.cancelAnimationFrame = (id: number) => clearTimeout(id);
  Object.defineProperty(window, "innerWidth", { configurable: true, value: 1200 });
  Object.defineProperty(window, "innerHeight", { configurable: true, value: 800 });
  Object.defineProperty(document, "hidden", { configurable: true, get: () => false });
  HTMLElement.prototype.getBoundingClientRect = () =>
    ({
      x: 12,
      y: 24,
      width: 320,
      height: 240,
      top: 24,
      left: 12,
      right: 332,
      bottom: 264,
      toJSON() {
        return {};
      },
    }) as DOMRect;
}

describe("artifact webview lifecycle", () => {
  let root: Root | null = null;
  let restore: (() => void) | null = null;

  beforeEach(() => {
    closed.length = 0;
    created.length = 0;
    resetTauriCoreMock();
    invokeImpl.handler = async (command) => {
      if (command === "prepare_artifact_preview") {
        return {
          artifactId: "artifact.fixture.interactive-html",
          revisionId: "rev.fixture.interactive-html.v1",
          title: "Interactive HTML",
          mediaType: "text/html",
          digest: "abc",
          previewToken: "a".repeat(64),
          expiresAt: "2026-01-01T00:00:00.000Z",
          webviewLabel: `artifact-preview-${created.length + 1}`,
          policy: {
            network: "none",
            navigation: "preview-only",
            popup: "deny",
            download: "deny",
            tauriIpc: "deny",
          },
        };
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

  test("creates one webview and cleans up on unmount", async () => {
    restore = installJsdom().restore;
    installMeasurableHost();
    const { createRoot } = await import("react-dom/client");
    root = createRoot(document.getElementById("root")!);
    await act(async () => root!.render(createElement(Harness)));
    for (let step = 0; step < 12; step += 1) {
      await act(async () => {
        await new Promise((resolve) => setTimeout(resolve, 0));
      });
    }
    expect(created.length).toBeGreaterThan(0);
    await act(async () => root?.unmount());
    await new Promise((resolve) => setTimeout(resolve, 0));
    root = null;
    expect(closed.length).toBeGreaterThan(0);
  });

  test("switches artifacts without leaving two live webviews", async () => {
    restore = installJsdom().restore;
    installMeasurableHost();
    const { createRoot } = await import("react-dom/client");
    root = createRoot(document.getElementById("root")!);
    await act(async () =>
      root!.render(createElement(Harness, { artifactId: "artifact.fixture.interactive-html" })),
    );
    for (let step = 0; step < 12; step += 1) {
      await act(async () => {
        await new Promise((resolve) => setTimeout(resolve, 0));
      });
    }
    await act(async () =>
      root!.render(createElement(Harness, { artifactId: "artifact.fixture.adversarial-ipc" })),
    );
    for (let step = 0; step < 12; step += 1) {
      await act(async () => {
        await new Promise((resolve) => setTimeout(resolve, 0));
      });
    }
    expect(created.length).toBeGreaterThanOrEqual(2);
    expect(closed.length).toBeGreaterThanOrEqual(1);
  });
});
