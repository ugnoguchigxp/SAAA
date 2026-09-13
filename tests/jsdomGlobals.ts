import { JSDOM } from "jsdom";

const names = ["window", "document", "navigator", "HTMLElement", "Event", "IS_REACT_ACT_ENVIRONMENT"] as const;

export function installJsdom(html = "<!doctype html><html><body><div id='root'></div></body></html>") {
  const dom = new JSDOM(html, { url: "http://localhost" });
  const previous = names.map((name) => Object.getOwnPropertyDescriptor(globalThis, name));
  const values: unknown[] = [
    dom.window,
    dom.window.document,
    dom.window.navigator,
    dom.window.HTMLElement,
    dom.window.Event,
    true,
  ];
  names.forEach((name, index) => {
    Object.defineProperty(globalThis, name, { configurable: true, writable: true, value: values[index] });
  });
  return {
    dom,
    restore() {
      names.forEach((name, index) => {
        const descriptor = previous[index];
        if (descriptor) Object.defineProperty(globalThis, name, descriptor);
        else Reflect.deleteProperty(globalThis, name);
      });
    },
  };
}
