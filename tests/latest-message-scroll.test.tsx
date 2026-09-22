import { afterEach, expect, test } from "bun:test";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { installJsdom } from "./jsdomGlobals";
import { useLatestMessageScroll } from "../src/features/chat/useLatestMessageScroll";

let root: Root | undefined;
let restore: (() => void) | undefined;
const originalObserver = globalThis.ResizeObserver;
afterEach(async () => {
  await act(async () => root?.unmount());
  restore?.();
  globalThis.ResizeObserver = originalObserver;
});

async function setup() {
  restore = installJsdom().restore;
  let notifyResize = () => {};
  let disconnected = false;
  globalThis.ResizeObserver = class {
    constructor(callback: () => void) {
      notifyResize = callback;
    }
    observe() {}
    disconnect() {
      disconnected = true;
    }
  } as unknown as typeof ResizeObserver;
  let height = 1000;
  let viewport = 400;
  let top = 0;
  Object.defineProperties(HTMLElement.prototype, {
    scrollHeight: { configurable: true, get: () => height },
    clientHeight: { configurable: true, get: () => viewport },
    scrollTop: {
      configurable: true,
      get: () => top,
      set: (value: number) => {
        top = Math.max(0, Math.min(value, height - viewport));
      },
    },
  });
  function Harness({ id, newer }: { id: string; newer: boolean }) {
    const scroll = useLatestMessageScroll(id, newer);
    return (
      <div ref={scroll.messageAreaRef} onScroll={scroll.updateFollowLatest}>
        <div ref={scroll.messageContentRef} />
        {scroll.showLatestButton && <button>latest</button>}
      </div>
    );
  }
  root = createRoot(document.getElementById("root")!);
  const render = async (id = "c1", newer = false) => {
    await act(async () => root!.render(<Harness id={id} newer={newer} />));
  };
  await render();
  return {
    render,
    top: () => top,
    disconnected: () => disconnected,
    async resize(nextHeight: number, nextViewport = viewport) {
      height = nextHeight;
      viewport = nextViewport;
      await act(async () => notifyResize());
    },
    async scroll(value: number) {
      top = value;
      await act(async () =>
        document.getElementById("root")!.firstElementChild!.dispatchEvent(new Event("scroll")),
      );
    },
  };
}

test("entry and remount default to the latest edge, even after browsing older messages", async () => {
  const ui = await setup();
  expect(ui.top()).toBe(600);
  await ui.scroll(200);
  await act(async () => root!.render(null));
  expect(ui.disconnected()).toBe(true);
  await ui.render();
  expect(ui.top()).toBe(600);
  await ui.scroll(200);
  await ui.render("c2");
  expect(ui.top()).toBe(600);
});

test("ASR details and delayed row measurements keep the full tail visible", async () => {
  const ui = await setup();
  await ui.resize(1100); // AI thinking details
  expect(ui.top()).toBe(700);
  await ui.resize(1200); // runtime details
  expect(ui.top()).toBe(800);
  await ui.resize(1800); // virtual row / markdown measurement
  expect(ui.top()).toBe(1400);
  await ui.resize(1800, 300); // viewport shrinks
  expect(ui.top()).toBe(1500);
});

test("reading older messages stays in place until the user returns to the bottom", async () => {
  const ui = await setup();
  await ui.scroll(200);
  await ui.resize(1400);
  await ui.render();
  expect(ui.top()).toBe(200);
  expect(document.querySelector("button")).not.toBeNull();
  await ui.scroll(1000);
  await ui.resize(1500);
  expect(ui.top()).toBe(1100);
});

test("an older paginated window waits for latest history before following", async () => {
  const ui = await setup();
  await ui.render("c2", true);
  await ui.resize(1500);
  expect(ui.top()).toBe(600);
  await ui.render("c2", false);
  expect(ui.top()).toBe(1100);
});
