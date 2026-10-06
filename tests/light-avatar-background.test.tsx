import { expect, mock, test } from "bun:test";
import { act } from "react";
import { installJsdom } from "./jsdomGlobals";
let created = 0,
  disposed = 0,
  draws = 0,
  disconnected = 0;
mock.module("../src/features/chat/avatar/model.js", () => ({
  createLightAvatar: (host: HTMLElement) => {
    created++;
    const canvas = document.createElement("canvas");
    host.append(canvas);
    return {
      canvas,
      resize() {},
      beginMotion() {},
      render() {
        draws++;
      },
      dispose() {
        disposed++;
        canvas.remove();
      },
    };
  },
}));
test("avatar releases hidden surfaces, respects reduced motion and finishes each cue", async () => {
  const env = installJsdom();
  const globals = ["requestAnimationFrame", "cancelAnimationFrame", "ResizeObserver"] as const;
  const previous = globals.map((k) => Object.getOwnPropertyDescriptor(globalThis, k));
  let id = 0;
  const frames = new Map<number, FrameRequestCallback>();
  Object.defineProperty(window, "WebGL2RenderingContext", { value: class {} });
  Object.assign(globalThis, {
    requestAnimationFrame: (fn: FrameRequestCallback) => {
      frames.set(++id, fn);
      return id;
    },
    cancelAnimationFrame: (key: number) => frames.delete(key),
    ResizeObserver: class {
      observe() {}
      disconnect() {
        disconnected++;
      }
    },
  });
  const { LightAvatarBackground } =
    await import("../src/features/chat/avatar/LightAvatarBackground");
  const { createRoot } = await import("react-dom/client");
  const root = createRoot(document.getElementById("root")!);
  try {
    await act(async () => {
      root.render(<LightAvatarBackground active reduced={false} />);
    });
    expect(document.querySelectorAll("canvas").length).toBe(1);
    expect(frames.size).toBe(0);
    await act(async () =>
      root.render(
        <LightAvatarBackground active reduced={false} cue={{ id: "one", motion: "joyful" }} />,
      ),
    );
    expect(frames.size).toBe(1);
    const start = performance.now();
    for (let elapsed = 0; elapsed <= 9_000; elapsed += 50) {
      const callbacks = [...frames.values()];
      frames.clear();
      callbacks.forEach((fn) => fn(start + elapsed));
    }
    expect(frames.size).toBe(0);
    expect(draws).toBeGreaterThan(10);
    await act(async () => root.render(<LightAvatarBackground active={false} reduced={false} />));
    expect(document.querySelectorAll("canvas").length).toBe(0);
    expect(disposed).toBe(1);
    expect(disconnected).toBe(1);
    await act(async () =>
      root.render(<LightAvatarBackground active reduced cue={{ id: "two", motion: "greeting" }} />),
    );
    expect(document.querySelectorAll("canvas").length).toBe(1);
    expect(frames.size).toBe(0);
    await act(async () => root.unmount());
    expect(created).toBe(disposed);
    expect(disconnected).toBe(2);
  } finally {
    for (let i = 0; i < globals.length; i++) {
      const descriptor = previous[i];
      if (descriptor) Object.defineProperty(globalThis, globals[i], descriptor);
      else Reflect.deleteProperty(globalThis, globals[i]);
    }
    env.restore();
  }
});
