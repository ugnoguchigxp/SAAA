import { afterEach, describe, expect, test } from "bun:test";
import { act, createElement, type MutableRefObject } from "react";
import type { Root } from "react-dom/client";
import { installJsdom } from "./jsdomGlobals";

const { useStreamingTextProjection } =
  await import("../src/features/chat/useStreamingTextProjection");

type Projection = ReturnType<typeof useStreamingTextProjection>;

function Harness({ apiRef }: { apiRef: MutableRefObject<Projection | null> }) {
  apiRef.current = useStreamingTextProjection();
  return null;
}

describe("streaming text projection hook", () => {
  let root: Root | null = null;
  let restore: (() => void) | null = null;

  afterEach(async () => {
    await act(async () => root?.unmount());
    root = null;
    restore?.();
    restore = null;
  });

  test("keeps the reset callback stable across renders", async () => {
    restore = installJsdom().restore;
    const { createRoot } = await import("react-dom/client");
    const apiRef: MutableRefObject<Projection | null> = { current: null };
    root = createRoot(document.getElementById("root")!);

    await act(async () => root!.render(createElement(Harness, { apiRef })));
    const initialReset = apiRef.current!.resetStreamingText;

    await act(async () => root!.render(createElement(Harness, { apiRef })));

    expect(apiRef.current!.resetStreamingText).toBe(initialReset);
  });
});
