import { afterEach, describe, expect, test } from "bun:test";
import { JSDOM } from "jsdom";
import React, { act } from "react";
import type { Root } from "react-dom/client";
import "../src/i18n";
import { ServiceConnectionsSection } from "../src/features/settings/ServiceConnectionsSection";
import { defaultSettingsDraft } from "../src/features/settings/settingsDefaults";
import type { HarnessResolution, ModelProvidersSettings } from "../src/lib/contracts";

const dom = new JSDOM("<!doctype html><html><body><div id='root'></div></body></html>", { url: "http://localhost" });
Object.assign(globalThis, {
  window: dom.window,
  document: dom.window.document,
  navigator: dom.window.navigator,
  HTMLElement: dom.window.HTMLElement,
  Event: dom.window.Event,
  IS_REACT_ACT_ENVIRONMENT: true,
});

let root: Root | null = null;
afterEach(() => {
  act(() => root?.unmount());
  root = null;
  document.body.innerHTML = "<div id='root'></div>";
});

describe("ServiceConnectionsSection asynchronous state", () => {
  test("does not apply a resolution for an address that has since changed", async () => {
    let resolveRequest!: (value: HarnessResolution) => void;
    const pending = new Promise<HarnessResolution>((resolve) => { resolveRequest = resolve; });
    let providers = structuredClone(defaultSettingsDraft.providers);
    const render = () => React.createElement(ServiceConnectionsSection, {
      providers,
      routing: defaultSettingsDraft.routing,
      onProvidersChange: (next: ModelProvidersSettings) => { providers = next; render(); },
      onRoutingChange: () => undefined,
      onValidityChange: () => undefined,
      resolveHarnessAddress: () => pending,
    });
    const { createRoot } = await import("react-dom/client");
    root = createRoot(document.getElementById("root")!);
    await act(async () => root!.render(render()));
    const button = document.querySelector<HTMLButtonElement>(".provider-card-footer button");
    if (!button) throw new Error("resolve button missing");
    await act(async () => button.dispatchEvent(new dom.window.MouseEvent("click", { bubbles: true })));

    const input = document.querySelector<HTMLInputElement>('input[placeholder="http://provider.local:9810"]')!;
    await act(async () => {
      const setter = Object.getOwnPropertyDescriptor(dom.window.HTMLInputElement.prototype, "value")!.set!;
      setter.call(input, "http://changed.local:9810");
      input.dispatchEvent(new dom.window.Event("input", { bubbles: true }));
    });
    await act(async () => resolveRequest({ state: "ready", revision: "v1", services: [] }));

    expect(document.querySelector(".provider-test-result")?.classList.contains("success")).toBe(false);
    expect(document.querySelector(".provider-card-footer")?.textContent).not.toContain("resolved");
  });

  test("returns to idle when the address changes outside the input handler", async () => {
    let resolveRequest: ((value: HarnessResolution) => void) | undefined;
    const request = new Promise<HarnessResolution>((resolve) => {
      resolveRequest = resolve;
    });
    const container = document.getElementById("root")!;
    const { createRoot } = await import("react-dom/client");
    root = createRoot(container);
    const render = (address: string) => root!.render(
      <ServiceConnectionsSection
        providers={{ ...defaultSettingsDraft.providers, harness: { address } }}
        routing={defaultSettingsDraft.routing}
        onProvidersChange={() => undefined}
        onRoutingChange={() => undefined}
        onValidityChange={() => undefined}
        resolveHarnessAddress={() => request}
      />,
    );

    await act(async () => render("http://first.local:9810"));
    const resolveButton = container.querySelector<HTMLButtonElement>(".provider-card-footer button");
    expect(resolveButton).toBeDefined();
    await act(async () => resolveButton?.click());
    expect(resolveButton?.disabled).toBe(true);

    await act(async () => render("http://second.local:9810"));
    expect(container.querySelector<HTMLButtonElement>(".provider-card-footer button")?.disabled).toBe(false);
    expect(container.querySelector(".provider-test-result")?.classList.contains("success")).toBe(false);

    await act(async () => resolveRequest?.({ state: "ready", revision: "v1", services: [] }));
    expect(container.querySelector(".provider-test-result")?.classList.contains("success")).toBe(false);
  });
});
