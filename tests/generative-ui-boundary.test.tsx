import { expect, test } from "bun:test";
import { JSDOM } from "jsdom";
import { act, lazy, Suspense } from "react";
import { createRoot } from "react-dom/client";
import { UiBoundary } from "../src/features/chat/ui/UiBoundary";
test("failed lazy UI import preserves the surrounding conversation and summary", async () => {
  const dom = new JSDOM('<div id="root"></div>');
  const names = ["window", "document", "navigator", "IS_REACT_ACT_ENVIRONMENT"] as const;
  const before = names.map((name) => Object.getOwnPropertyDescriptor(globalThis, name));
  const values = [dom.window, dom.window.document, dom.window.navigator, true];
  names.forEach((name, i) =>
    Object.defineProperty(globalThis, name, {
      configurable: true,
      writable: true,
      value: values[i],
    }),
  );
  const root = createRoot(dom.window.document.getElementById("root")!, { onCaughtError: () => {} });
  try {
    const Broken = lazy(async () => {
      throw Error("chunk unavailable");
    });
    await act(async () => {
      root.render(
        <>
          <p>既存の会話</p>
          <UiBoundary fallback={<p>保存済みの要約</p>}>
            <Suspense fallback="loading">
              <Broken />
            </Suspense>
          </UiBoundary>
        </>,
      );
    });
    expect(dom.window.document.body.textContent).toContain("既存の会話");
    expect(dom.window.document.body.textContent).toContain("保存済みの要約");
  } finally {
    await act(async () => root.unmount());
    dom.window.close();
    names.forEach((name, i) => {
      const descriptor = before[i];
      if (descriptor) Object.defineProperty(globalThis, name, descriptor);
      else Reflect.deleteProperty(globalThis, name);
    });
  }
});
