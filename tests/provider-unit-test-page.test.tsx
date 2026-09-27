import { expect, test } from "bun:test";
import { act } from "react";
import { installJsdom } from "./jsdomGlobals";
import { invokeCalls, invokeImpl, resetTauriCoreMock } from "./tauriCoreMock";

test("each text provider runs only its selected capability and shows its own result", async () => {
  const environment = installJsdom();
  resetTauriCoreMock();
  const { ProviderUnitTestPage } =
    await import("../src/features/providerUnitTest/ProviderUnitTestPage");
  const { createRoot } = await import("react-dom/client");
  const root = createRoot(document.getElementById("root")!);
  invokeImpl.handler = async (_command, args) => {
    const capability = (args as { input: { capability: string } }).input.capability;
    return {
      capability,
      model: `model-${capability}`,
      output: `result-${capability}`,
      latencyMs: 4,
      audioBase64: null,
    };
  };
  try {
    await act(async () =>
      root.render(
        <ProviderUnitTestPage inputDeviceId="default" echoCancellation onOpenSettings={() => {}} />,
      ),
    );
    for (const [capability, label] of [
      ["llm", "Ornith1.5"],
      ["embedding", "Embedding"],
      ["tts", "TTS"],
    ]) {
      await act(async () => {
        [...document.querySelectorAll<HTMLButtonElement>(".provider-unit-list button")]
          .find((button) => button.querySelector("strong")?.textContent === label)!
          .click();
      });
      await act(async () =>
        document.querySelector<HTMLButtonElement>(".provider-unit-run")!.click(),
      );
      if (!document.querySelector(".provider-unit-result pre")) {
        throw new Error(document.querySelector('[role="alert"]')?.textContent ?? "result missing");
      }
      expect(document.querySelector(".provider-unit-result pre")?.textContent).toBe(
        `result-${capability}`,
      );
    }
    expect(invokeCalls.map(({ command }) => command)).toEqual(
      Array(3).fill("run_provider_unit_test"),
    );
    expect(
      invokeCalls.map(({ args }) => (args as { input: { capability: string } }).input.capability),
    ).toEqual(["llm", "embedding", "tts"]);
  } finally {
    await act(async () => root.unmount());
    environment.restore();
    resetTauriCoreMock();
  }
});
