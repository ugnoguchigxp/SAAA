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
      ["laya", "Laya"],
      ["laya-speech", "Laya（発話表現）"],
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
      Array(5).fill("run_provider_unit_test"),
    );
    expect(
      invokeCalls.map(({ args }) => (args as { input: { capability: string } }).input.capability),
    ).toEqual(["llm", "embedding", "laya", "laya-speech", "tts"]);
  } finally {
    await act(async () => root.unmount());
    environment.restore();
    resetTauriCoreMock();
  }
});

test("image and music generation run from the unit test page with a fixed kind and show the failure code", async () => {
  const environment = installJsdom();
  resetTauriCoreMock();
  const { ProviderUnitTestPage } =
    await import("../src/features/providerUnitTest/ProviderUnitTestPage");
  const { createRoot } = await import("react-dom/client");
  const root = createRoot(document.getElementById("root")!);
  invokeImpl.handler = async (command, args) => {
    if (command === "list_media_generations") return [];
    const input = (args as { input: { runId: string } }).input;
    return {
      runId: input.runId,
      result: null,
      error: {
        kind: "discovery",
        code: "not_found",
        retryable: false,
        mayHaveGenerated: false,
        jobId: null,
      },
    };
  };
  try {
    await act(async () =>
      root.render(
        <ProviderUnitTestPage inputDeviceId="default" echoCancellation onOpenSettings={() => {}} />,
      ),
    );
    for (const [label, kind] of [
      ["画像生成", "image"],
      ["楽曲生成", "music"],
    ]) {
      await act(async () => {
        [...document.querySelectorAll<HTMLButtonElement>(".provider-unit-list button")]
          .find((button) => button.querySelector("strong")?.textContent === label)!
          .click();
      });
      expect(document.querySelector('select[aria-label="作成するもの"]')).toBeNull();
      expect(document.querySelector(".provider-unit-run")).toBeNull();
      const textarea = document.querySelector<HTMLTextAreaElement>("textarea")!;
      await act(async () => {
        Object.getOwnPropertyDescriptor(window.HTMLTextAreaElement.prototype, "value")!.set!.call(
          textarea,
          "テスト",
        );
        textarea.dispatchEvent(new window.Event("input", { bubbles: true }));
      });
      await act(async () =>
        document
          .querySelector("form")!
          .dispatchEvent(new window.Event("submit", { bubbles: true, cancelable: true })),
      );
      expect(document.querySelector('[role="alert"]')?.textContent).toContain("not_found");
      const call = invokeCalls.filter(({ command }) => command === "generate_media").at(-1)!;
      expect((call.args as { input: { kind: string } }).input.kind).toBe(kind);
    }
    expect(invokeCalls.filter(({ command }) => command === "generate_media")).toHaveLength(2);
  } finally {
    await act(async () => root.unmount());
    environment.restore();
    resetTauriCoreMock();
  }
});

test("media tests wait for completion, prevent switching and duplicate submission, then show artifacts", async () => {
  const environment = installJsdom();
  resetTauriCoreMock();
  const previousCreate = URL.createObjectURL;
  const previousRevoke = URL.revokeObjectURL;
  URL.createObjectURL = () => "blob:unit-media";
  URL.revokeObjectURL = () => {};
  const { ProviderUnitTestPage } =
    await import("../src/features/providerUnitTest/ProviderUnitTestPage");
  const { createRoot } = await import("react-dom/client");
  const root = createRoot(document.getElementById("root")!);
  let finish: (() => void) | undefined;
  invokeImpl.handler = async (command, args) => {
    if (command === "list_media_generations") return [];
    if (command === "read_generated_media") return new Uint8Array([1, 2, 3]).buffer;
    if (command !== "generate_media") throw new Error(`Unexpected command: ${command}`);
    const { input, onProgress } = args as {
      input: { runId: string; kind: "image" | "music" };
      onProgress: { onmessage: (value: unknown) => void };
    };
    onProgress.onmessage({
      phase: "loading",
      jobId: input.kind === "music" ? "job-1" : null,
      progress: null,
    });
    return new Promise((resolve) => {
      finish = () =>
        resolve({
          runId: input.runId,
          error: null,
          result: {
            kind: input.kind,
            model: "model",
            jobId: input.kind === "music" ? "job-1" : null,
            artifacts: [
              {
                id: "artifact-1",
                contentUrl: "/content",
                metadataUrl: null,
                mimeType: input.kind === "image" ? "image/png" : "audio/mpeg",
                metadata: {},
              },
            ],
          },
        });
    });
  };
  try {
    await act(async () =>
      root.render(
        <ProviderUnitTestPage inputDeviceId="default" echoCancellation onOpenSettings={() => {}} />,
      ),
    );
    for (const [index, label, element] of [
      [0, "画像生成", "img"],
      [1, "楽曲生成", "audio"],
    ] as const) {
      await act(async () => {
        [...document.querySelectorAll<HTMLButtonElement>(".provider-unit-list button")]
          .find((button) => button.querySelector("strong")?.textContent === label)!
          .click();
      });
      expect(document.querySelector<HTMLTextAreaElement>("textarea")!.value.length).toBeGreaterThan(
        0,
      );
      const submit = () =>
        document
          .querySelector("form")!
          .dispatchEvent(new window.Event("submit", { bubbles: true, cancelable: true }));
      await act(async () => {
        submit();
        submit();
      });
      expect(
        [...document.querySelectorAll<HTMLButtonElement>(".provider-unit-list button")].every(
          (button) => button.disabled,
        ),
      ).toBe(true);
      expect(document.body.textContent).toContain("経過 0 秒");
      if (index === 1) expect(document.body.textContent).toContain("job-1");
      await act(async () => {
        submit();
      });
      expect(invokeCalls.filter(({ command }) => command === "generate_media")).toHaveLength(
        index + 1,
      );
      await act(async () => finish!());
      expect(document.querySelector(element)?.getAttribute("src")).toBe("blob:unit-media");
      expect(
        [...document.querySelectorAll<HTMLButtonElement>(".provider-unit-list button")].every(
          (button) => !button.disabled,
        ),
      ).toBe(true);
    }
    expect(invokeCalls.filter(({ command }) => command === "cancel_media_generation")).toHaveLength(
      0,
    );
    expect(invokeCalls.filter(({ command }) => command === "read_generated_media")).toHaveLength(2);
  } finally {
    await act(async () => root.unmount());
    URL.createObjectURL = previousCreate;
    URL.revokeObjectURL = previousRevoke;
    environment.restore();
    resetTauriCoreMock();
  }
});
