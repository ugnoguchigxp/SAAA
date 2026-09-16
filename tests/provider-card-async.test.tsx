import { test, expect } from "bun:test";
import { act } from "react";
import { installJsdom } from "./jsdomGlobals";
import "../src/i18n";
import { ProviderCard } from "../src/features/settings/ProviderCard";
import type { OpenAiCompatibleProviderSettings } from "../src/lib/contracts";
import type { testModelProvider } from "../src/lib/runtime";
test("a late probe cannot mark changed provider settings as successful", async () => {
  const env = installJsdom();
  const { createRoot } = await import("react-dom/client");
  const root = createRoot(document.getElementById("root")!);
  let done!: (v: Awaited<ReturnType<typeof testModelProvider>>) => void;
  const pending = new Promise<Awaited<ReturnType<typeof testModelProvider>>>((resolve) => {
    done = resolve;
  });
  const provider: OpenAiCompatibleProviderSettings = {
    kind: "openai-compatible",
    id: "fixture",
    enabled: true,
    label: "Fixture",
    location: "local",
    endpoint: "http://localhost/first",
    model: "fixture",
    authentication: "none",
  };
  const render = (endpoint: string) =>
    root.render(
      <ProviderCard
        provider={{ ...provider, endpoint }}
        persisted
        onChange={() => {}}
        onRemove={() => {}}
        testProvider={() => pending}
      />,
    );
  try {
    await act(async () => render(provider.endpoint));
    await act(async () =>
      document.querySelector<HTMLButtonElement>(".provider-card-footer button")!.click(),
    );
    await act(async () => render("http://localhost/second"));
    await act(async () =>
      done({ ok: true, latencyMs: 5, message: "OK" } as Awaited<
        ReturnType<typeof testModelProvider>
      >),
    );
    expect(document.querySelector(".provider-test-result.success")).toBeNull();
    expect(
      document.querySelector<HTMLButtonElement>(".provider-card-footer button")!.disabled,
    ).toBe(false);
  } finally {
    await act(async () => root.unmount());
    env.restore();
  }
});
