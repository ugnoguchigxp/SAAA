import { afterEach, beforeEach, describe, expect, test } from "bun:test";
import { act, type MutableRefObject } from "react";
import type { Root } from "react-dom/client";
import { currentLarmVoice } from "../src/lib/larmVoiceRuntime";
import { resetTauriCoreMock } from "./tauriCoreMock";
import { installJsdom } from "./jsdomGlobals";

const { useLarmVoiceLifetime } = await import("../src/features/voice/useLarmVoiceLifetime");

function Harness({
  enabled,
  conversationId,
  apiRef,
}: {
  enabled: boolean;
  conversationId: string | null;
  apiRef: MutableRefObject<((next: boolean) => void) | null>;
}) {
  apiRef.current = useLarmVoiceLifetime(enabled, conversationId, () => false, () => undefined);
  return null;
}

describe("LARM voice lifetime hook", () => {
  let root: Root | null = null;
  let restore: (() => void) | null = null;

  beforeEach(() => {
    resetTauriCoreMock();
  });

  afterEach(async () => {
    await act(async () => root?.unmount());
    root = null;
    restore?.();
    restore = null;
  });

  test("owns a connection while listening and releases it when disabled", async () => {
    restore = installJsdom().restore;
    const { createRoot } = await import("react-dom/client");
    const { createElement } = await import("react");
    const apiRef: MutableRefObject<((next: boolean) => void) | null> = { current: null };
    root = createRoot(document.getElementById("root")!);
    await act(async () => root!.render(createElement(Harness, { enabled: true, conversationId: "c1", apiRef })));
    expect(currentLarmVoice()?.conversationId).toBe("c1");
    await act(async () => apiRef.current?.(false));
    await act(async () => { await Promise.resolve(); });
    await act(async () => root!.render(createElement(Harness, { enabled: false, conversationId: "c1", apiRef })));
  });
});
