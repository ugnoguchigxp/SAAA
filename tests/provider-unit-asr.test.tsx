import { expect, test } from "bun:test";
import { act } from "react";
import { installJsdom } from "./jsdomGlobals";
import { channels, invokeCalls, invokeImpl, resetTauriCoreMock } from "./tauriCoreMock";
import { ProviderUnitTestPage } from "../src/features/providerUnitTest/ProviderUnitTestPage";

test("ASR uploads recorded PCM, shows backend stages and retains audio for a failed connection retry", async () => {
  const environment = installJsdom();
  resetTauriCoreMock();
  const originals = ["AudioContext", "AudioWorkletNode"].map(
    (name) => [name, Object.getOwnPropertyDescriptor(globalThis, name)] as const,
  );
  const track = { stop() {}, addEventListener() {}, removeEventListener() {} };
  Object.defineProperty(navigator, "mediaDevices", {
    configurable: true,
    value: {
      getUserMedia: async () => ({ getTracks: () => [track], getAudioTracks: () => [track] }),
    },
  });
  class Context {
    sampleRate = 16_000;
    state = "running";
    destination = {};
    audioWorklet = { addModule: async () => {} };
    createMediaStreamSource() {
      return { connect() {}, disconnect() {} };
    }
    async close() {}
  }
  class Node {
    static current: Node;
    port = {
      onmessage: null as ((event: { data: unknown }) => void) | null,
      postMessage: () => this.port.onmessage?.({ data: { type: "flushed" } }),
    };
    constructor() {
      Node.current = this;
    }
    connect() {}
    disconnect() {}
  }
  Object.defineProperty(globalThis, "AudioContext", { configurable: true, value: Context });
  Object.defineProperty(globalThis, "AudioWorkletNode", { configurable: true, value: Node });
  const { createRoot } = await import("react-dom/client");
  const root = createRoot(document.getElementById("root")!);
  const uploads: number[][] = [];
  let rejectRun: (error: Error) => void = () => {};
  let attempts = 0;
  invokeImpl.handler = async (command, args, options) => {
    if (command === "stage_audio_upload") {
      expect(options).toEqual({ headers: { "x-saaa-audio-purpose": "provider-unit-asr" } });
      uploads.push(Array.from(args as Uint8Array));
      return "audio-fixture";
    }
    expect(command).toBe("run_provider_unit_test");
    expect(args).toMatchObject({ input: { capability: "asr", audioUploadId: "audio-fixture" } });
    channels.at(-1)!.onmessage?.({ stage: "semantic_probing" });
    if (++attempts === 1)
      return new Promise((_, reject) => {
        rejectRun = reject;
      });
    channels.at(-1)!.onmessage?.({ stage: "provider_request" });
    return {
      capability: "asr",
      model: "asr-fixture",
      output: "音声認識のテストです。",
      latencyMs: 6,
      audioBase64: null,
    };
  };
  try {
    await act(async () =>
      root.render(
        <ProviderUnitTestPage inputDeviceId="default" echoCancellation onOpenSettings={() => {}} />,
      ),
    );
    const recordButton = () =>
      document.querySelector<HTMLButtonElement>(".provider-unit-input button")!;
    const runButton = () => document.querySelector<HTMLButtonElement>(".provider-unit-run")!;
    await act(async () => recordButton().click());
    Node.current.port.onmessage?.({ data: new Float32Array(3_200).fill(0.25) });
    await act(async () => recordButton().click());
    await act(async () => runButton().click());
    expect(document.body.textContent).toContain("接続先の準備を確認中");
    expect(runButton().disabled).toBe(true);
    await act(async () => rejectRun(new Error("connection unavailable")));
    expect(document.body.textContent).toContain("connection unavailable");
    expect(runButton().disabled).toBe(false);
    await act(async () => runButton().click());
    expect(document.querySelector(".provider-unit-result pre")?.textContent).toBe(
      "音声認識のテストです。",
    );
    expect(uploads).toHaveLength(2);
    expect(uploads[0]).toEqual(uploads[1]);
    expect(uploads[0]?.some((value) => value !== 0)).toBe(true);
    expect(invokeCalls.map(({ command }) => command)).toEqual([
      "stage_audio_upload",
      "run_provider_unit_test",
      "stage_audio_upload",
      "run_provider_unit_test",
    ]);
  } finally {
    await act(async () => root.unmount());
    for (const [name, original] of originals) {
      if (original) Object.defineProperty(globalThis, name, original);
      else Reflect.deleteProperty(globalThis, name);
    }
    environment.restore();
    resetTauriCoreMock();
  }
});
