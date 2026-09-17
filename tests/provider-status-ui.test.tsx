import { expect, test } from "bun:test";
import { act } from "react";
import i18n from "../src/i18n";
import { ApiKeyControl } from "../src/features/settings/ApiKeyControl";
import { ProviderCard } from "../src/features/settings/ProviderCard";
import type { OpenAiCompatibleProviderSettings } from "../src/lib/contracts";
import { installJsdom } from "./jsdomGlobals";

test("renders an actionable authentication failure instead of a generic error", async () => {
  const env = installJsdom();
  const { createRoot } = await import("react-dom/client");
  const root = createRoot(document.getElementById("root")!);
  await i18n.changeLanguage("en");
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
  try {
    await act(async () =>
      root.render(
        <ProviderCard
          provider={provider}
          persisted
          onChange={() => {}}
          onRemove={() => {}}
          testProvider={async () => ({
            providerId: provider.id,
            ok: false,
            latencyMs: 2,
            message: "HTTP 401 unauthorized",
          })}
        />,
      ),
    );
    await act(async () =>
      document.querySelector<HTMLButtonElement>(".provider-card-footer button")!.click(),
    );
    expect(document.querySelector(".provider-test-result.error")?.textContent).toContain(
      "Authentication failed",
    );
    expect(document.querySelector(".provider-test-result.error")?.textContent).toContain(
      "Check the saved API key",
    );
  } finally {
    await act(async () => root.unmount());
    env.restore();
  }
});

test("does not offer credential storage on an unsupported operating system", async () => {
  const env = installJsdom();
  const { createRoot } = await import("react-dom/client");
  const root = createRoot(document.getElementById("root")!);
  Object.defineProperty(globalThis.navigator, "userAgent", {
    configurable: true,
    value: "Mozilla/5.0 (Windows NT 10.0; Win64; x64)",
  });
  await i18n.changeLanguage("en");
  const provider: OpenAiCompatibleProviderSettings = {
    kind: "openai-compatible",
    id: "unsupported-credential-store",
    enabled: true,
    label: "Unsupported credential store",
    location: "remote",
    endpoint: "https://example.invalid/v1",
    model: "fixture",
    authentication: "api-key",
  };
  try {
    await act(async () =>
      root.render(<ApiKeyControl provider={provider} persisted onCredentialChange={() => {}} />),
    );
    expect(document.querySelector('[role="status"]')?.textContent).toContain(
      "API-key storage is not supported",
    );
    expect(document.querySelector('input[type="password"]')).toBeNull();
  } finally {
    await act(async () => root.unmount());
    env.restore();
  }
});
