import { expect, test } from "bun:test";
import { act } from "react";
import { installJsdom } from "./jsdomGlobals";
import { invokeCalls, invokeImpl, resetTauriCoreMock } from "./tauriCoreMock";

test("TTS result exposes playable audio and releases its preview URL", async () => {
  const environment = installJsdom();
  resetTauriCoreMock();
  const { ProviderUnitTestPage } =
    await import("../src/features/providerUnitTest/ProviderUnitTestPage");
  const { createRoot } = await import("react-dom/client");
  const root = createRoot(document.getElementById("root")!);
  const created: Blob[] = [];
  const revoked: string[] = [];
  const originalCreate = URL.createObjectURL;
  const originalRevoke = URL.revokeObjectURL;
  URL.createObjectURL = (blob) => {
    created.push(blob);
    return "blob:tts-fixture";
  };
  URL.revokeObjectURL = (url) => revoked.push(url);
  invokeImpl.handler = async (command) => {
    expect(command).toBe("run_provider_unit_test");
    return {
      capability: "tts",
      model: "voicevox-core",
      output: "音声を生成しました",
      latencyMs: 3,
      audioBase64: btoa("RIFFfixture"),
    };
  };
  try {
    await act(async () =>
      root.render(
        <ProviderUnitTestPage inputDeviceId="default" echoCancellation onOpenSettings={() => {}} />,
      ),
    );
    await act(async () => {
      [...document.querySelectorAll<HTMLButtonElement>(".provider-unit-list button")]
        .find((button) => button.querySelector("strong")?.textContent === "TTS")!
        .click();
    });
    await act(async () => document.querySelector<HTMLButtonElement>(".provider-unit-run")!.click());
    expect(invokeCalls[0]?.args).toMatchObject({
      input: {
        capability: "tts",
        text: "こんにちは。音声合成のテストです。",
        audioUploadId: undefined,
      },
    });
    expect(created).toHaveLength(1);
    expect(await created[0]!.text()).toBe("RIFFfixture");
    expect(document.querySelector<HTMLAudioElement>("audio")?.src).toBe("blob:tts-fixture");
  } finally {
    await act(async () => root.unmount());
    expect(revoked).toEqual(["blob:tts-fixture"]);
    URL.createObjectURL = originalCreate;
    URL.revokeObjectURL = originalRevoke;
    environment.restore();
    resetTauriCoreMock();
  }
});
