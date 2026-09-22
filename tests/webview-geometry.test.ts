import { describe, expect, test } from "bun:test";
import {
  isStaleGeometry,
  logicalRectFromDom,
  nextGeometryGeneration,
} from "../src/features/chat/artifacts/webviewGeometry";

describe("webview geometry", () => {
  test("keeps CSS pixels and hides empty, offscreen, or hidden rects", () => {
    const visible = logicalRectFromDom(
      { x: 10.5, y: 20.25, width: 300.5, height: 400 },
      { viewportWidth: 1200, viewportHeight: 800 },
    );
    expect(visible).toEqual({
      x: 10.5,
      y: 20.25,
      width: 300.5,
      height: 400,
      visible: true,
    });
    expect(
      logicalRectFromDom(
        { x: 0, y: 0, width: 0, height: 10 },
        { viewportWidth: 800, viewportHeight: 600 },
      ).visible,
    ).toBe(false);
    expect(
      logicalRectFromDom(
        { x: 900, y: 0, width: 10, height: 10 },
        { viewportWidth: 800, viewportHeight: 600 },
      ).visible,
    ).toBe(false);
    expect(
      logicalRectFromDom(
        { x: 10, y: 10, width: 10, height: 10 },
        { viewportWidth: 800, viewportHeight: 600, hidden: true },
      ).visible,
    ).toBe(false);
  });

  test("treats older resize generations as stale", () => {
    const first = nextGeometryGeneration(0);
    const second = nextGeometryGeneration(first);
    expect(isStaleGeometry(first, second)).toBe(true);
    expect(isStaleGeometry(second, second)).toBe(false);
  });
});
